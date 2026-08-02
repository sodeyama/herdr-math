//! Terminal initialization, raw mode, and capability probes.
//!
//! The [`Terminal`] type owns the terminal-facing state: it writes the mode
//! enable strings that keep the main screen buffer and its scrollback intact
//! (it never enters the alternate screen), restores termios on reset, and
//! probes cell size and pixel-mouse support. All I/O goes through the [`Tty`]
//! trait so every escape byte and probe reply is asserted in unit tests against
//! a fake device; no real terminal is required.

use std::io::{self, Write};
use std::os::fd::AsFd as _;
use std::time::{Duration, Instant};

/// Terminal size, with optional pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub cols: u32,
    pub rows: u32,
    pub width_px: u32,
    pub height_px: u32,
}

impl WindowSize {
    /// Pixel size of one cell, when the terminal reports pixel dimensions and
    /// a nonzero cell count.
    pub fn cell_size(&self) -> Option<(u32, u32)> {
        if self.cols > 0 && self.rows > 0 && self.width_px > 0 && self.height_px > 0 {
            Some((self.width_px / self.cols, self.height_px / self.rows))
        } else {
            None
        }
    }
}

/// Turned-on reporting modes. V2 must keep the main buffer, so it never writes
/// the alternate-screen switch `?1049h`.
const INIT_MODES: &[u8] = b"\x1b[?1003h\x1b[?1006h\x1b[?1016h\x1b[?2004h";
const RESET_MODES: &[u8] = b"\x1b[?2004l\x1b[?1016l\x1b[?1006l\x1b[?1003l";

/// Maximum bytes collected for one report probe before giving up.
const MAX_PROBE_BYTES: usize = 256;

/// The terminal-facing I/O surface. A real implementation drives stdin/stdout
/// and termios; tests use a fake that records writes and queues reads.
pub trait Tty {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
    /// Reads up to `buf.len()` bytes; returns 0 on EOF.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    /// Whether input is available within the timeout (blocking when None).
    fn poll_readable(&mut self, timeout: Option<Duration>) -> io::Result<bool>;
    /// Puts the underlying tty into raw mode, preserving the previous state for
    /// [`Tty::restore`].
    fn set_raw(&mut self) -> io::Result<()>;
    /// Restores the previously saved terminal attributes.
    fn restore(&mut self) -> io::Result<()>;
    /// Current window size from the terminal driver.
    fn window_size(&self) -> io::Result<WindowSize>;
}

/// Terminal state over a concrete [`Tty`].
pub struct Terminal<T: Tty> {
    io: T,
    mouse_pixels: bool,
    cell: Option<(u32, u32)>,
    cell_query_unsupported: bool,
    image_id: u32,
}

impl<T: Tty> Terminal<T> {
    /// Enables raw mode and the reporting modes the renderer needs, then probes
    /// pixel-mouse support. The main screen buffer is preserved.
    pub fn new(mut io: T, image_id: u32) -> io::Result<Self> {
        io.set_raw()?;
        io.write_all(INIT_MODES)?;
        io.flush()?;
        let mut terminal = Self {
            io,
            mouse_pixels: false,
            cell: None,
            cell_query_unsupported: false,
            image_id,
        };
        terminal.mouse_pixels = terminal.probe_mouse_pixels()?;
        Ok(terminal)
    }

    /// Whether mouse reports carry pixel coordinates.
    pub fn reports_pixel_mouse(&self) -> bool {
        self.mouse_pixels
    }

    /// The image id this terminal owns, used for the placeholder grid.
    pub fn image_id(&self) -> u32 {
        self.image_id
    }

    /// Current window size from the terminal driver.
    pub fn size(&self) -> io::Result<WindowSize> {
        self.io.window_size()
    }

    /// Measures the pixel size of one cell, preferring the `CSI 6;h;w t`
    /// report and falling back to the winsize pixel counts.
    pub fn cell_size(&mut self) -> io::Result<Option<(u32, u32)>> {
        if self.cell.is_some() {
            return Ok(self.cell);
        }
        // Inside tmux the `CSI 16t` query is answered by tmux with character
        // counts (rows;cols), not pixels, so it would corrupt the grid; use
        // the winsize pixel fallback there instead.
        let query_usable = !crate::kitty::inside_tmux() && !self.cell_query_unsupported;
        if query_usable {
            self.io.write_all(b"\x1b[16t")?;
            self.io.flush()?;
            if let Some(cell) =
                self.read_report(Duration::from_millis(300), parse_cell_size_report)?
            {
                self.cell = Some(cell);
                return Ok(self.cell);
            }
            self.cell_query_unsupported = true;
        }
        self.cell = self.size()?.cell_size();
        Ok(self.cell)
    }

    /// Probes whether the terminal reports pixel mouse via `DECRQM ?1016`.
    fn probe_mouse_pixels(&mut self) -> io::Result<bool> {
        self.io.write_all(b"\x1b[?1016$p")?;
        self.io.flush()?;
        Ok(self
            .read_report(Duration::from_millis(150), parse_decrqm_1016)?
            .unwrap_or(false))
    }

    /// Probes whether the terminal can display images. A `a=q` query with a
    /// minimal payload is answered with `Gi=<id>;OK` when graphics work;
    /// missing, negative, or truncated replies mean unsupported.
    pub fn probe_graphics_support(&mut self) -> io::Result<bool> {
        const PROBE_ID: u32 = u32::MAX;
        self.io.write_all(
            format!("\x1b_Gi={PROBE_ID},a=q,f=32,s=1,v=1;AAAAAAAAAAAAAAAAAAAAAA==\x1b\\")
                .as_bytes(),
        )?;
        self.io.flush()?;
        Ok(self
            .read_report(Duration::from_millis(300), parse_graphics_probe)?
            .unwrap_or(false))
    }

    /// Reads until `parse` recognizes a complete reply, within the timeout and
    /// a bounded byte count.
    fn read_report<R>(
        &mut self,
        timeout: Duration,
        parse: impl Fn(&[u8]) -> Option<R>,
    ) -> io::Result<Option<R>> {
        let deadline = Instant::now() + timeout;
        let mut buf = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || buf.len() > MAX_PROBE_BYTES {
                return Ok(None);
            }
            if !self.io.poll_readable(Some(remaining))? {
                return Ok(None);
            }
            let mut chunk = [0u8; 64];
            let n = match self.io.read(&mut chunk) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if n == 0 {
                return Ok(None);
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(value) = parse(&buf) {
                return Ok(Some(value));
            }
        }
    }

    /// Restores the terminal: disables the reporting modes and restores the
    /// saved termios state. Safe to call more than once.
    pub fn reset(&mut self) -> io::Result<()> {
        self.io.write_all(RESET_MODES)?;
        self.io.flush()?;
        self.io.restore()
    }
}

impl<T: Tty> Drop for Terminal<T> {
    fn drop(&mut self) {
        let _ = self.reset();
    }
}

fn retry_intr(
    mut call: impl FnMut() -> io::Result<rustix::termios::Termios>,
) -> io::Result<rustix::termios::Termios> {
    loop {
        match call() {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

/// Termios-backed [`Tty`] over stdin/stdout.
#[derive(Default)]
pub struct StdioTty {
    saved: Option<rustix::termios::Termios>,
}

impl Tty for StdioTty {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        io::stdout()
            .lock()
            .write_all(&crate::kitty::wrapped_for_tty(bytes))
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().lock().flush()
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let stdin = io::stdin();
        loop {
            match rustix::io::read(stdin.as_fd(), &mut *buf) {
                Err(rustix::io::Errno::INTR) => continue,
                other => return other.map_err(io::Error::from),
            }
        }
    }

    fn poll_readable(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        use rustix::event::{PollFlags, Timespec};
        let timeout = timeout.map(|duration| Timespec {
            tv_sec: duration.as_secs() as i64,
            tv_nsec: duration.subsec_nanos() as _,
        });
        let stdin = io::stdin();
        let mut fds = [rustix::event::PollFd::new(&stdin, PollFlags::IN)];
        let ready = rustix::event::poll(&mut fds, timeout.as_ref()).map_err(io::Error::from)?;
        Ok(ready > 0 && fds[0].revents().contains(PollFlags::IN))
    }

    fn set_raw(&mut self) -> io::Result<()> {
        let stdin = io::stdin();
        let fd = stdin.as_fd();
        let saved = retry_intr(|| rustix::termios::tcgetattr(fd).map_err(io::Error::from))?;
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(fd, rustix::termios::OptionalActions::Flush, &raw)
            .map_err(io::Error::from)?;
        self.saved = Some(saved);
        Ok(())
    }

    fn restore(&mut self) -> io::Result<()> {
        let Some(saved) = self.saved.take() else {
            return Ok(());
        };
        let stdin = io::stdin();
        rustix::termios::tcsetattr(
            stdin.as_fd(),
            rustix::termios::OptionalActions::Flush,
            &saved,
        )
        .map_err(io::Error::from)
    }

    fn window_size(&self) -> io::Result<WindowSize> {
        let stdin = io::stdin();
        let ws = rustix::termios::tcgetwinsize(stdin.as_fd()).map_err(io::Error::from)?;
        Ok(WindowSize {
            cols: u32::from(ws.ws_col),
            rows: u32::from(ws.ws_row),
            width_px: u32::from(ws.ws_xpixel),
            height_px: u32::from(ws.ws_ypixel),
        })
    }
}

/// Parses the `CSI 6;<height>;<width> t` cell-size report.
fn parse_cell_size_report(buf: &[u8]) -> Option<(u32, u32)> {
    let start = buf.windows(4).position(|w| w == b"\x1b[6;")? + 4;
    let end = start + buf[start..].iter().position(|&b| b == b't')?;
    let mut parts = buf[start..end].split(|&b| b == b';');
    let height: u32 = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let width: u32 = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    if width > 0 && height > 0 {
        Some((width, height))
    } else {
        None
    }
}

/// Parses the `DECRQM ?1016` pixel-mouse probe reply.
pub(crate) fn parse_decrqm_1016(buf: &[u8]) -> Option<bool> {
    let start = buf.windows(8).position(|w| w == b"\x1b[?1016;")? + 8;
    let ps = *buf.get(start)?;
    Some(ps == b'1' || ps == b'3')
}

/// Parses a Kitty `a=q` probe reply: `Gi=<id>;OK` reports support.
pub(crate) fn parse_graphics_probe(buf: &[u8]) -> Option<bool> {
    let needle = b"Gi=4294967295;";
    let pos = buf.windows(needle.len()).position(|w| w == needle)?;
    let rest = &buf[pos + needle.len()..];
    if rest.len() < 2 {
        return None;
    }
    Some(rest.starts_with(b"OK"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory [`Tty`] that queues replies and records writes.
    struct FakeTty {
        writes: Vec<u8>,
        reads: Vec<u8>,
        replies: Vec<(Vec<u8>, Vec<u8>)>,
        raw: usize,
        restored: usize,
        winsize: WindowSize,
    }

    impl FakeTty {
        fn new(replies: &[(&[u8], &[u8])], winsize: WindowSize) -> Self {
            Self {
                writes: Vec::new(),
                reads: Vec::new(),
                replies: replies
                    .iter()
                    .map(|(query, reply)| (query.to_vec(), reply.to_vec()))
                    .collect(),
                raw: 0,
                restored: 0,
                winsize,
            }
        }
    }

    impl Tty for FakeTty {
        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writes.extend_from_slice(bytes);
            let position = self
                .replies
                .iter()
                .position(|(query, _)| bytes.windows(query.len()).any(|w| w == query.as_slice()));
            if let Some(index) = position {
                let (_, reply) = self.replies.remove(index);
                self.reads.extend_from_slice(&reply);
            }
            Ok(())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.reads.len().min(buf.len());
            buf[..n].copy_from_slice(&self.reads[..n]);
            let _ = self.reads.drain(..n);
            Ok(n)
        }
        fn poll_readable(&mut self, _timeout: Option<Duration>) -> io::Result<bool> {
            Ok(!self.reads.is_empty())
        }
        fn set_raw(&mut self) -> io::Result<()> {
            self.raw += 1;
            Ok(())
        }
        fn restore(&mut self) -> io::Result<()> {
            self.restored += 1;
            Ok(())
        }
        fn window_size(&self) -> io::Result<WindowSize> {
            Ok(self.winsize)
        }
    }

    #[test]
    fn init_writes_reporting_modes_without_the_alternate_screen() {
        let tty = FakeTty::new(
            &[],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let term = Terminal::new(tty, 4).unwrap();
        let out = String::from_utf8(term.io.writes.clone()).unwrap();
        assert!(out.contains("\x1b[?1003h"));
        assert!(out.contains("\x1b[?1006h"));
        assert!(out.contains("\x1b[?1016h"));
        assert!(out.contains("\x1b[?2004h"));
        assert!(
            !out.contains("?1049"),
            "must not enter the alternate screen"
        );
        assert!(out.contains("\x1b[?1016$p"), "pixel-mouse probe is written");
        assert!(
            !term.reports_pixel_mouse(),
            "no DECRQM reply means unsupported"
        );
    }

    #[test]
    fn pixel_mouse_probe_accepts_supported_replies() {
        let tty = FakeTty::new(
            &[(b"\x1b[?1016$p".as_slice(), b"\x1b[?1016;1$y")],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let term = Terminal::new(tty, 4).unwrap();
        assert!(term.reports_pixel_mouse());
    }

    #[test]
    fn pixel_mouse_probe_rejects_unsupported_replies() {
        let tty = FakeTty::new(
            &[(b"\x1b[?1016$p".as_slice(), b"\x1b[?1016;2$y")],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let term = Terminal::new(tty, 4).unwrap();
        assert!(!term.reports_pixel_mouse());
    }

    #[test]
    fn cell_size_parses_the_query_report() {
        let tty = FakeTty::new(
            &[(b"\x1b[16t".as_slice(), b"\x1b[6;24;80t")],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let mut term = Terminal::new(tty, 4).unwrap();
        assert_eq!(term.cell_size().unwrap(), Some((80, 24)));
        let out = String::from_utf8(term.io.writes.clone()).unwrap();
        assert!(out.contains("\x1b[16t"), "cell-size query is written");
    }

    #[test]
    fn cell_size_falls_back_to_winsize_when_unanswered() {
        let tty = FakeTty::new(
            &[],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let mut term = Terminal::new(tty, 4).unwrap();
        assert_eq!(term.cell_size().unwrap(), Some((10, 10)));
    }

    #[test]
    fn cell_size_rejects_zero_reports() {
        assert_eq!(parse_cell_size_report(b"\x1b[6;0;0t"), None);
        assert_eq!(
            parse_cell_size_report(b"noise\x1b[6;24;80t"),
            Some((80, 24))
        );
        assert_eq!(parse_cell_size_report(b"\x1b[6;24"), None, "incomplete");
    }

    #[test]
    fn windowsize_cell_size_requires_pixel_and_cell_dimensions() {
        let ws = WindowSize {
            cols: 80,
            rows: 24,
            width_px: 0,
            height_px: 0,
        };
        assert_eq!(ws.cell_size(), None);
        let ws = WindowSize {
            cols: 0,
            rows: 0,
            width_px: 800,
            height_px: 240,
        };
        assert_eq!(ws.cell_size(), None);
    }

    #[test]
    fn reset_disables_modes_and_restores_termios() {
        let tty = FakeTty::new(
            &[],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let mut term = Terminal::new(tty, 4).unwrap();
        term.reset().unwrap();
        let out = String::from_utf8(term.io.writes.clone()).unwrap();
        assert!(out.contains("\x1b[?2004l"));
        assert!(out.contains("\x1b[?1016l"));
        assert!(out.contains("\x1b[?1006l"));
        assert!(out.contains("\x1b[?1003l"));
        assert_eq!(term.io.restored, 1);
    }

    #[test]
    fn decode_of_cell_size_and_pixel_mouse_replies() {
        assert_eq!(parse_decrqm_1016(b"\x1b[?1016;0$y"), Some(false));
        assert_eq!(parse_decrqm_1016(b"\x1b[?1016;1$y"), Some(true));
        assert_eq!(parse_decrqm_1016(b"\x1b[?1016;3$y"), Some(true));
        assert_eq!(parse_decrqm_1016(b"noise\x1b[?1016;3$y\n"), Some(true));
        assert_eq!(parse_decrqm_1016(b"\x1b[?1016;"), None, "partial");
    }

    #[test]
    fn graphics_probe_parses_ok_and_missing_replies() {
        assert_eq!(
            parse_graphics_probe(b"\x1b_Gi=4294967295;OK\x1b\\"),
            Some(true)
        );
        assert_eq!(
            parse_graphics_probe(b"noise\x1b_Gi=4294967295;OK\x1b\\more"),
            Some(true)
        );
        assert_eq!(
            parse_graphics_probe(b"\x1b_Gi=4294967295;"),
            None,
            "partial"
        );
        assert_eq!(
            parse_graphics_probe(b"\x1b_Gi=4294967294;OK\x1b\\"),
            None,
            "wrong id"
        );
    }

    #[test]
    fn graphics_support_probe_is_flaky_positive_on_ok_reply() {
        let tty = FakeTty::new(
            &[(
                b"\x1b_Gi=4294967295".as_slice(),
                b"\x1b_Gi=4294967295;OK\x1b\\",
            )],
            WindowSize {
                cols: 80,
                rows: 24,
                width_px: 800,
                height_px: 240,
            },
        );
        let mut term = Terminal::new(tty, 4).unwrap();
        assert!(term.probe_graphics_support().unwrap());
        let out = String::from_utf8(term.io.writes.clone()).unwrap();
        assert!(out.contains("a=q,f=32,s=1,v=1"));
    }

    #[test]
    fn probe_parsers_never_panic_on_adversarial_input() {
        // Deterministic fuzz over reply bytes for the cell-size, pixel-mouse,
        // and graphics-probe decoders. They must return a value or None, never
        // panic, and never report a zero/inverted cell or spurious support.
        let mut seed = 0xfeed_u64;
        let alphabet = b"\x1b[?;=0123456789$tyOKGi";
        for _ in 0..4096u32 {
            let len = ((seed >> 32) as usize % 40) + 1;
            let mut buf = Vec::new();
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                buf.push(alphabet[(seed >> 56) as usize % alphabet.len()]);
            }
            let cell = std::panic::catch_unwind(|| parse_cell_size_report(&buf));
            assert!(cell.is_ok(), "cell-size parser must not panic on {buf:?}");
            if let Ok(Some((w, h))) = cell {
                assert!(w > 0 && h > 0, "cell size must be positive on {buf:?}");
            }
            let decrqm = std::panic::catch_unwind(|| parse_decrqm_1016(&buf));
            assert!(decrqm.is_ok(), "DECRQM parser must not panic on {buf:?}");
            let probe = std::panic::catch_unwind(|| parse_graphics_probe(&buf));
            assert!(
                probe.is_ok(),
                "graphics probe parser must not panic on {buf:?}"
            );
        }
    }
}

//! Incremental, bounded decoding of raw terminal input.
//!
//! [`InputDecoder`] buffers capped bytes from stdin and replays them one event
//! at a time. It decodes SGR pixel/cell mouse reports, cursor and page keys,
//! plain characters, bracketed-paste spans, focus in/out, and `Ctrl-C`/`q` for
//! the scroll loop. Malformed or truncated input never allocates unbounded
//! memory and the decoder resumes at the next valid event boundary.

use std::collections::VecDeque;

use crate::mouse::{parse_sgr_mouse, Key, MouseEvent};

/// Maximum bytes buffered before the decoder drops the pending prefix.
pub const MAX_PENDING_BYTES: usize = 64 * 1024;

/// A decoded terminal input event for the scroll loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Focus(bool),
}

/// A decoded keyboard event used by the scroll fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Maximum total bytes consumed before `next_event` must make progress.
const MAX_PROGRESS_STEPS: usize = 1024;

/// One parse outcome from a byte buffer.
enum Parsed {
    /// A complete event and the number of bytes it consumed.
    Event(Event, usize),
    /// The sequence is incomplete; more bytes are required.
    Deferred,
    /// The prefix is unparseable garbage; drop this many bytes.
    Skip(usize),
}

/// Bounded incremental decoder over raw stdin bytes.
#[derive(Debug)]
pub struct InputDecoder {
    pending: VecDeque<u8>,
    total_dropped: u64,
}

impl Default for InputDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl InputDecoder {
    pub fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            total_dropped: 0,
        }
    }

    /// Appends raw bytes, evicting the oldest prefix when the cap is exceeded.
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend(bytes.iter().copied());
        if self.pending.len() > MAX_PENDING_BYTES {
            let overflow = self.pending.len() - MAX_PENDING_BYTES;
            self.total_dropped += overflow as u64;
            for _ in 0..overflow {
                self.pending.pop_front();
            }
        }
    }

    /// Total bytes evicted to stay within the buffer cap.
    pub fn dropped(&self) -> u64 {
        self.total_dropped
    }

    /// Whether buffered input can make progress without more bytes.
    pub fn has_event(&self) -> bool {
        self.peek_event().is_some()
    }

    /// Parses and returns the next complete event, or `None` if more bytes are
    /// needed. Unparseable prefixes are skipped to a valid boundary.
    pub fn next_event(&mut self) -> Option<Event> {
        for _ in 0..MAX_PROGRESS_STEPS {
            if self.pending.is_empty() {
                return None;
            }
            let buf: Vec<u8> = self.pending.iter().copied().collect();
            match parse_one(&buf) {
                Parsed::Event(event, used) => {
                    self.drain(used);
                    return Some(event);
                }
                Parsed::Deferred => return None,
                Parsed::Skip(n) => self.drain(n),
            }
        }
        None
    }

    /// Returns the next event without consuming it, for readiness checks.
    fn peek_event(&self) -> Option<Event> {
        let buf: Vec<u8> = self.pending.iter().copied().collect();
        match parse_one(&buf) {
            Parsed::Event(event, _) => Some(event),
            _ => None,
        }
    }

    fn drain(&mut self, n: usize) {
        for _ in 0..n {
            self.pending.pop_front();
        }
    }
}

/// Parses one event from the front of a byte buffer.
fn parse_one(buf: &[u8]) -> Parsed {
    let b0 = buf[0];
    if b0 != 0x1b {
        return parse_plain_bytes(buf);
    }
    let Some(&b1) = buf.get(1) else {
        return Parsed::Deferred;
    };
    match b1 {
        b'[' => parse_csi(buf),
        b'O' => {
            if buf.len() < 3 {
                return Parsed::Deferred;
            }
            let key = match buf[2] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                _ => Key::Unknown,
            };
            Parsed::Event(
                Event::Key(KeyEvent {
                    key,
                    ctrl: false,
                    alt: false,
                }),
                3,
            )
        }
        b'_' | b']' | b'P' | b'X' | b'^' => {
            // String sequences (OSC/DCS/PM/APC): ignore the payload, bounded.
            match string_sequence_end(buf) {
                Some(end) => Parsed::Skip(end),
                None if buf.len() > MAX_PENDING_BYTES => Parsed::Skip(buf.len()),
                None => Parsed::Deferred,
            }
        }
        _ => {
            // A lone ESC is the Escape key; ESC + byte is an Alt-modified key.
            let mut event = byte_key_event(b1);
            event.alt = true;
            event.ctrl = false;
            Parsed::Event(Event::Key(event), 2)
        }
    }
}

/// Parses plain bytes at the front of a buffer (chars, ASCII controls).
fn parse_plain_bytes(buf: &[u8]) -> Parsed {
    let b0 = buf[0];
    if b0 >= 0x80 {
        let len = match b0 {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => return Parsed::Skip(1),
        };
        if buf.len() < len {
            return Parsed::Deferred;
        }
        return match std::str::from_utf8(&buf[..len]) {
            Ok(s) => {
                let c = s.chars().next().unwrap_or('\u{fffd}');
                Parsed::Event(Event::Key(byte_key_event(c as u8)), len)
            }
            Err(_) => Parsed::Skip(1),
        };
    }
    Parsed::Event(Event::Key(byte_key_event(b0)), 1)
}

/// Maps a single byte to a key event (controls and printable ASCII).
fn byte_key_event(b: u8) -> KeyEvent {
    match b {
        0x0d => KeyEvent {
            key: Key::Enter,
            ctrl: false,
            alt: false,
        },
        0x09 => KeyEvent {
            key: Key::Tab,
            ctrl: false,
            alt: false,
        },
        0x7f | 0x08 => KeyEvent {
            key: Key::Backspace,
            ctrl: false,
            alt: false,
        },
        0x1b => KeyEvent {
            key: Key::Escape,
            ctrl: false,
            alt: false,
        },
        c @ 0x01..=0x1a => KeyEvent {
            key: Key::Char((b'a' + c - 1) as char),
            ctrl: true,
            alt: false,
        },
        c @ 0x20..=0x7e => KeyEvent {
            key: Key::Char(c as char),
            ctrl: false,
            alt: false,
        },
        _ => {
            // Printable non-ASCII falls back to a literal char.
            let ch = (b0_or_latin1(b)) as char;
            KeyEvent {
                key: Key::Char(ch),
                ctrl: false,
                alt: false,
            }
        }
    }
}

fn b0_or_latin1(b: u8) -> u8 {
    b
}

/// Parses a CSI sequence (`ESC [ params final`).
fn parse_csi(buf: &[u8]) -> Parsed {
    if buf.starts_with(PASTE_START) {
        let body_start = PASTE_START.len();
        let Some(pos) = find_subsequence(&buf[body_start..], PASTE_END) else {
            if buf.len() > MAX_PENDING_BYTES * 2 {
                return Parsed::Skip(PASTE_START.len());
            }
            return Parsed::Deferred;
        };
        let body = normalize_newlines(
            String::from_utf8_lossy(&buf[body_start..body_start + pos]).into_owned(),
        );
        return Parsed::Event(Event::Paste(body), body_start + pos + PASTE_END.len());
    }

    let mut end = 2;
    let terminator = loop {
        let Some(&b) = buf.get(end) else {
            return Parsed::Deferred;
        };
        end += 1;
        if (0x40..=0x7e).contains(&b) {
            break b;
        }
        if end - 2 > 1024 {
            return Parsed::Skip(2);
        }
    };
    let params = &buf[2..end - 1];
    match terminator {
        b'M' | b'm' => match parse_sgr_mouse(params, terminator == b'M') {
            Some(event) => Parsed::Event(Event::Mouse(event), end),
            None => Parsed::Skip(end),
        },
        b'A' => Parsed::Event(key_ev(Key::Up), end),
        b'B' => Parsed::Event(key_ev(Key::Down), end),
        b'C' => Parsed::Event(key_ev(Key::Right), end),
        b'D' => Parsed::Event(key_ev(Key::Left), end),
        b'H' => Parsed::Event(key_ev(Key::Home), end),
        b'F' => Parsed::Event(key_ev(Key::End), end),
        b'I' => Parsed::Event(Event::Focus(true), end),
        b'O' => Parsed::Event(Event::Focus(false), end),
        b'~' => {
            let number = csi_number(params, 0);
            let mapped = match number {
                Some(2) => Key::Insert,
                Some(3) => Key::Delete,
                Some(5) => Key::PageUp,
                Some(6) => Key::PageDown,
                Some(1 | 7) => Key::Home,
                Some(4 | 8) => Key::End,
                _ => Key::Unknown,
            };
            Parsed::Event(key_ev(mapped), end)
        }
        _ => Parsed::Skip(end),
    }
}

fn key_ev(key: Key) -> Event {
    Event::Key(KeyEvent {
        key,
        ctrl: false,
        alt: false,
    })
}

/// Reads the `n`-th `;`-separated parameter of a CSI body as a number.
fn csi_number(params: &[u8], index: usize) -> Option<u32> {
    let mut split = params.split(|&b| b == b';');
    let field = split.nth(index)?;
    std::str::from_utf8(field)
        .ok()?
        .split(':')
        .next()?
        .parse()
        .ok()
}

/// Finds a byte subsequence, returning the index of its start.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Finds the end of a string sequence (`OSC`/`DCS`/...) terminated by `ESC \`.
fn string_sequence_end(buf: &[u8]) -> Option<usize> {
    if let Some(pos) = find_subsequence(buf, b"\x1b\\") {
        return Some(pos + 2);
    }
    // A bare BEL also terminates an OSC.
    buf.iter().position(|&b| b == 0x07).map(|pos| pos + 1)
}

fn normalize_newlines(mut text: String) -> String {
    text = text.replace("\r\n", "\n").replace('\r', "\n");
    text
}

/// Adds missing keyboard variants used by the scroll loop.
pub mod keys {
    pub use crate::mouse::Key;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::{MouseButton, MouseKind};

    fn events(bytes: &[u8]) -> Vec<Event> {
        let mut decoder = InputDecoder::new();
        decoder.push(bytes);
        let mut out = Vec::new();
        while let Some(event) = decoder.next_event() {
            out.push(event);
        }
        out
    }

    fn key_event(key: Key) -> Event {
        Event::Key(KeyEvent {
            key,
            ctrl: false,
            alt: false,
        })
    }

    #[test]
    fn decodes_named_keys() {
        assert_eq!(events(b"\x1b[A"), vec![key_event(Key::Up)]);
        assert_eq!(events(b"\x1b[B"), vec![key_event(Key::Down)]);
        assert_eq!(events(b"\x1bOA"), vec![key_event(Key::Up)], "SS3 arrows");
        assert_eq!(events(b"\x1bOB"), vec![key_event(Key::Down)], "SS3 arrows");
        assert_eq!(events(b"\x1b[5~"), vec![key_event(Key::PageUp)]);
        assert_eq!(events(b"\x1b[6~"), vec![key_event(Key::PageDown)]);
        assert_eq!(events(b"\x1b[1~"), vec![key_event(Key::Home)]);
        assert_eq!(events(b"\x1b[4~"), vec![key_event(Key::End)]);
    }

    #[test]
    fn decodes_plain_characters_and_controls() {
        assert_eq!(events(b"j"), vec![key_event(Key::Char('j'))]);
        assert_eq!(events(b"G"), vec![key_event(Key::Char('G'))]);
        assert_eq!(events(b"\r"), vec![key_event(Key::Enter)]);
        let ctrl_c = events(b"\x03");
        assert_eq!(
            ctrl_c,
            vec![Event::Key(KeyEvent {
                key: Key::Char('c'),
                ctrl: true,
                alt: false
            })]
        );
    }

    #[test]
    fn decodes_scroll_wheel_mouse() {
        let bytes = b"\x1b[<64;10;20M\x1b[<65;10;20M";
        let decoded = events(bytes);
        assert_eq!(decoded.len(), 2);
        assert!(matches!(
            decoded[0],
            Event::Mouse(MouseEvent {
                kind: MouseKind::ScrollUp,
                x: 10,
                y: 20,
                ..
            })
        ));
        assert!(matches!(
            decoded[1],
            Event::Mouse(MouseEvent {
                kind: MouseKind::ScrollDown,
                x: 10,
                y: 20,
                ..
            })
        ));
    }

    #[test]
    fn decodes_button_and_motion() {
        let decoded = events(b"\x1b[<0;3;4M\x1b[<35;5;6M");
        assert_eq!(decoded.len(), 2);
        assert!(matches!(
            decoded[0],
            Event::Mouse(MouseEvent {
                kind: MouseKind::Down,
                button: MouseButton::Left,
                ..
            })
        ));
        assert!(matches!(
            decoded[1],
            Event::Mouse(MouseEvent {
                kind: MouseKind::Move,
                ..
            })
        ));
    }

    #[test]
    fn decodes_bracketed_paste_as_one_event() {
        let decoded = events(b"\x1b[200~hi\r\nthere\rend\x1b[201~x");
        assert_eq!(decoded.len(), 2);
        assert_eq!(
            decoded[0],
            Event::Paste("hi\nthere\nend".to_string()),
            "CRLF and CR normalize to LF"
        );
        assert_eq!(decoded[1], key_event(Key::Char('x')));
    }

    #[test]
    fn an_unclosed_paste_waits_without_growing_unbounded() {
        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[200~unfinished paste");
        assert!(decoder.next_event().is_none(), "paste is deferred");

        decoder.push(b"\x1b[201~");
        assert_eq!(
            decoder.next_event(),
            Some(Event::Paste("unfinished paste".to_string()))
        );
    }

    #[test]
    fn decodes_focus_events() {
        assert_eq!(events(b"\x1b[I"), vec![Event::Focus(true)]);
        assert_eq!(events(b"\x1b[O"), vec![Event::Focus(false)]);
    }

    #[test]
    fn truncated_sequences_are_deferred_until_complete() {
        let mut decoder = InputDecoder::new();
        decoder.push(b"\x1b[<64;10;20");
        assert!(decoder.next_event().is_none());
        decoder.push(b"M");
        assert!(matches!(decoder.next_event(), Some(Event::Mouse(_))));
    }

    #[test]
    fn garbage_prefix_is_skipped_to_the_next_boundary() {
        let mut decoder = InputDecoder::new();
        decoder.push(b"garbage\x1b[A");
        let mut decoded = Vec::new();
        while let Some(event) = decoder.next_event() {
            decoded.push(event);
        }
        assert!(
            decoded.contains(&key_event(Key::Up)),
            "recovers at a valid CSI"
        );
    }

    #[test]
    fn overflow_evicts_the_oldest_bytes() {
        let mut decoder = InputDecoder::new();
        let big = vec![b'a'; MAX_PENDING_BYTES * 2];
        decoder.push(&big);
        assert!(decoder.dropped() >= MAX_PENDING_BYTES as u64);
        assert!(decoder.pending.len() <= MAX_PENDING_BYTES);
    }

    #[test]
    fn oversize_parameter_runs_are_bounded() {
        let mut decoder = InputDecoder::new();
        decoder.push(&[0x1b, b'[']);
        decoder.push(&[b'1'; 2048]);
        // The CSI never terminates within the 1024-param cap; the decoder drops
        // the leading ESC [ and keeps the digits as plain chars, bounded.
        let mut count = 0;
        while decoder.next_event().is_some() {
            count += 1;
            assert!(count < 4096, "must not loop forever");
        }
        assert!(decoder.pending.len() <= MAX_PENDING_BYTES);
    }

    #[test]
    fn ignore_osc_sequences() {
        let decoded = events(b"\x1b]0;title\x1b\\j");
        assert_eq!(decoded, vec![key_event(Key::Char('j'))], "OSC is skipped");
    }

    #[test]
    fn adversarial_byte_streams_never_panic_or_grow_unbounded() {
        // Deterministic fuzz over the full printable + escape byte range. The
        // decoder must always terminate, stay within the buffer cap, and never
        // emit a raw marker as a character event.
        let mut seed = 0x5eed_u64;
        for iteration in 0..512u64 {
            let mut bytes = Vec::new();
            let len = 1 + (iteration as usize % 64);
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let kind = (seed >> 32) as u8 % 4;
                bytes.push(match kind {
                    0 => (seed >> 24) as u8,
                    1 => 0x1b,
                    2 => b'[',
                    _ => (seed >> 56) as u8,
                });
            }
            let mut decoder = InputDecoder::new();
            decoder.push(&bytes);
            let mut count = 0;
            while let Some(event) = decoder.next_event() {
                count += 1;
                assert!(count < 4096, "must always make progress");
                match event {
                    Event::Key(KeyEvent {
                        key: Key::Char(c), ..
                    }) => {
                        // A lone ESC must never become a plain ESC char here.
                        assert!(c != '\u{1b}');
                    }
                    Event::Paste(_) | Event::Focus(_) | Event::Mouse(_) | Event::Key(_) => {}
                }
            }
            assert!(decoder.pending.len() <= MAX_PENDING_BYTES);
        }
    }

    #[test]
    fn feeding_in_small_chunks_preserves_events() {
        let full = b"\x1b[A\x1b[<64;3;4M\x1b[200~pasted\x1b[201~q";
        let one_shot = events(full).len();
        let mut decoder = InputDecoder::new();
        let mut chunked = 0;
        for &b in full {
            decoder.push(&[b]);
            while let Some(_event) = decoder.next_event() {
                chunked += 1;
            }
        }
        assert_eq!(
            chunked, one_shot,
            "byte-by-byte and whole-buffer decode agree"
        );
    }
}

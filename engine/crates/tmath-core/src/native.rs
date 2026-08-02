//! macOS native input helper integration.
//!
//! The helper (`native-scroll-helper.swift`, compiled by `build.rs` on macOS)
//! reports trackpad precision deltas, zoom, the OS cursor position, and window
//! geometry over a line protocol on its stdout. This module spawns the helper,
//! parses its lines into [`NativeEvent`]s, and fans each event out to
//! subscribers. When the helper is unavailable the module degrades to no
//! events; the caller keeps the mouse-wheel and keyboard fallbacks.

use std::io::BufRead as _;
use std::io::{BufReader, Write as _};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

const KEEPALIVE: Duration = Duration::from_secs(8);

/// A point in macOS screen coordinates.
pub type Point = (f32, f32);

/// A rectangle in macOS screen coordinates.
pub type Rect = (f32, f32, f32, f32);

/// A parsed event delivered by the native helper.
#[derive(Debug, Clone, Copy)]
pub enum NativeEvent {
    Scroll {
        delta_x: f32,
        delta_y: f32,
        precise: bool,
        phase: u32,
        momentum: u32,
        point: Option<Point>,
    },
    Zoom {
        magnification: f32,
        point: Option<Point>,
    },
    Cursor {
        point: Point,
    },
    Window {
        rect: Option<Rect>,
    },
}

enum Msg {
    Scale(f32),
    Event(NativeEvent),
}

impl Msg {
    /// Whether the event should drive an animated frame (cursor and window
    /// moves alone are not enough to redraw).
    fn wakes(&self) -> bool {
        !matches!(
            self,
            Msg::Event(NativeEvent::Cursor { .. } | NativeEvent::Window { .. })
        )
    }
}

struct SharedHelper {
    child: Child,
    stdin: Option<ChildStdin>,
    subscribers: Vec<Sender<Msg>>,
    scale: f32,
    dead: bool,
    wanting: usize,
    armed_at: Option<std::time::Instant>,
}

impl SharedHelper {
    /// Writes the current cursor-tracking request to the helper if the desired
    /// state differs from what the helper is doing, throttled to the keepalive
    /// window.
    fn sync_arming(&mut self) {
        let want = self.wanting > 0;
        let due = match (want, self.armed_at) {
            (true, Some(at)) => at.elapsed() > KEEPALIVE,
            (true, None) => true,
            (false, Some(_)) => true,
            (false, None) => false,
        };
        if !due {
            return;
        }
        self.armed_at = want.then(std::time::Instant::now);
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let line: &[u8] = if want {
            b"positions 1\n"
        } else {
            b"positions 0\n"
        };
        if stdin.write_all(line).and_then(|()| stdin.flush()).is_err() {
            self.stdin = None;
        }
    }
}

static SHARED: Mutex<Option<SharedHelper>> = Mutex::new(None);

fn subscribe() -> Option<Receiver<Msg>> {
    let mut shared = SHARED.lock().unwrap();
    if shared.is_none() {
        let path = std::env::var("NATIVE_SCROLL_HELPER")
            .ok()
            .or_else(|| option_env!("NATIVE_SCROLL_HELPER").map(String::from))?;
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let stdin = child.stdin.take();
        std::thread::spawn(move || {
            read_lines(stdout);
            let mut shared = SHARED.lock().unwrap();
            if let Some(helper) = shared.as_mut() {
                helper.dead = true;
                helper.subscribers.clear();
            }
        });
        *shared = Some(SharedHelper {
            child,
            stdin,
            subscribers: Vec::new(),
            scale: 2.0,
            dead: false,
            wanting: 0,
            armed_at: None,
        });
    }
    let helper = shared.as_mut().unwrap();
    if helper.dead {
        return None;
    }
    let (tx, rx) = channel();
    let _ = tx.send(Msg::Scale(helper.scale));
    helper.subscribers.push(tx);
    Some(rx)
}

fn read_lines(stdout: ChildStdout) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            return;
        };
        let fields: Vec<&str> = line.split_whitespace().collect();
        let msg = match fields.first().copied() {
            Some("scale") => field(&fields, 1).map(Msg::Scale),
            Some("s") => parse_scroll(&fields),
            Some("z") => field(&fields, 1).map(|magnification| {
                Msg::Event(NativeEvent::Zoom {
                    magnification,
                    point: parse_point(&fields, 2),
                })
            }),
            Some("m") => {
                parse_point(&fields, 1).map(|point| Msg::Event(NativeEvent::Cursor { point }))
            }
            Some("w") => Some(Msg::Event(NativeEvent::Window {
                rect: parse_rect(&fields),
            })),
            _ => None,
        };
        let Some(msg) = msg else {
            continue;
        };
        let wakes = msg.wakes();
        let mut shared = SHARED.lock().unwrap();
        let Some(helper) = shared.as_mut() else {
            return;
        };
        if let Msg::Scale(scale) = msg {
            helper.scale = scale;
        }
        helper.subscribers.retain(|tx| {
            tx.send(match msg {
                Msg::Scale(scale) => Msg::Scale(scale),
                Msg::Event(event) => Msg::Event(event),
            })
            .is_ok()
        });
        let _ = wakes;
    }
}

fn field<T: std::str::FromStr>(fields: &[&str], index: usize) -> Option<T> {
    fields.get(index)?.parse().ok()
}

fn parse_point(fields: &[&str], index: usize) -> Option<Point> {
    Some((field(fields, index)?, field(fields, index + 1)?))
}

fn parse_rect(fields: &[&str]) -> Option<Rect> {
    if fields.get(1).copied() == Some("none") {
        return None;
    }
    let (x, y) = parse_point(fields, 1)?;
    let (w, h) = parse_point(fields, 3)?;
    Some((x, y, w, h))
}

fn parse_scroll(fields: &[&str]) -> Option<Msg> {
    Some(Msg::Event(NativeEvent::Scroll {
        delta_y: field(fields, 1)?,
        phase: field(fields, 2).unwrap_or(0),
        momentum: field(fields, 3).unwrap_or(0),
        precise: fields.get(4).copied() == Some("1"),
        delta_x: field(fields, 5).unwrap_or(0.0),
        point: parse_point(fields, 6),
    }))
}

/// A subscribed stream of native input events.
pub struct NativeScroll {
    rx: Receiver<Msg>,
    pub scale: f32,
    dead: bool,
    wants_positions: bool,
}

impl NativeScroll {
    /// Spawns (or reuses) the helper and returns a subscribed stream.
    pub fn spawn() -> Option<Self> {
        let rx = subscribe()?;
        Some(Self {
            rx,
            scale: 2.0,
            dead: false,
            wants_positions: false,
        })
    }

    /// Requests that the helper stream OS cursor positions. Ref-counted across
    /// subscribers via the shared helper.
    pub fn request_positions(&mut self, want: bool) {
        if want == self.wants_positions {
            return;
        }
        self.wants_positions = want;
        let mut shared = SHARED.lock().unwrap();
        let Some(helper) = shared.as_mut() else {
            return;
        };
        if want {
            helper.wanting += 1;
        } else {
            helper.wanting = helper.wanting.saturating_sub(1);
        }
        helper.sync_arming();
    }

    /// Drains all currently buffered events.
    pub fn drain(&mut self) -> Vec<NativeEvent> {
        let mut events = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(Msg::Scale(scale)) => self.scale = scale,
                Ok(Msg::Event(event)) => events.push(event),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.dead = true;
                    break;
                }
            }
        }
        events
    }

    /// Whether the helper process has exited or failed to start.
    pub fn dead(&self) -> bool {
        self.dead
    }
}

impl Drop for NativeScroll {
    fn drop(&mut self) {
        let mut shared = SHARED.lock().unwrap();
        let Some(helper) = shared.as_mut() else {
            return;
        };
        if self.wants_positions {
            helper.wanting = helper.wanting.saturating_sub(1);
            helper.sync_arming();
        }
        helper
            .subscribers
            .retain(|tx| tx.send(Msg::Scale(helper.scale)).is_ok());
        if helper.subscribers.len() <= 1 {
            let _ = helper.child.kill();
            *shared = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scroll(line: &str) -> NativeEvent {
        let fields: Vec<&str> = line.split_whitespace().collect();
        match parse_scroll(&fields) {
            Some(Msg::Event(event)) => event,
            _ => panic!("did not parse: {line}"),
        }
    }

    #[test]
    fn a_scroll_line_carries_the_cursor_when_the_helper_sends_it() {
        let NativeEvent::Scroll {
            delta_y,
            delta_x,
            phase,
            precise,
            point,
            ..
        } = scroll("s 3.5 1 0 1 -2.0 400.5 350.25")
        else {
            panic!("expected a scroll")
        };
        assert_eq!((delta_y, delta_x), (3.5, -2.0));
        assert_eq!((phase, precise), (1, true));
        assert_eq!(point, Some((400.5, 350.25)));
    }

    #[test]
    fn a_scroll_line_without_a_cursor_still_parses() {
        let NativeEvent::Scroll {
            delta_y,
            delta_x,
            point,
            ..
        } = scroll("s 3.5 1 0 1")
        else {
            panic!("expected a scroll")
        };
        assert_eq!((delta_y, delta_x), (3.5, 0.0));
        assert_eq!(point, None);
    }

    #[test]
    fn imprecise_is_anything_but_one() {
        let NativeEvent::Scroll { precise, .. } = scroll("s 5 0 0 0") else {
            panic!("expected a scroll")
        };
        assert!(!precise);
    }

    #[test]
    fn a_window_line_parses_its_rect_or_its_absence() {
        let fields: Vec<&str> = "w 10 20 300 400".split_whitespace().collect();
        assert_eq!(parse_rect(&fields), Some((10.0, 20.0, 300.0, 400.0)));

        let fields: Vec<&str> = "w none".split_whitespace().collect();
        assert_eq!(parse_rect(&fields), None);
    }

    #[test]
    fn a_zoom_line_parses_magnification_and_point() {
        let fields: Vec<&str> = "z 0.1 12.0 34.0".split_whitespace().collect();
        let msg = field::<f32>(&fields, 1).map(|magnification| {
            Msg::Event(NativeEvent::Zoom {
                magnification,
                point: parse_point(&fields, 2),
            })
        });
        match msg {
            Some(Msg::Event(NativeEvent::Zoom {
                magnification,
                point,
            })) => {
                assert_eq!(magnification, 0.1);
                assert_eq!(point, Some((12.0, 34.0)));
            }
            _ => panic!("expected a zoom"),
        }
    }

    #[test]
    fn cursor_and_window_updates_do_not_wake_a_sleeping_engine() {
        use NativeEvent::{Cursor, Window};
        assert!(!Msg::Event(Cursor { point: (1.0, 2.0) }).wakes());
        assert!(!Msg::Event(Window { rect: None }).wakes());
        assert!(Msg::Scale(2.0).wakes());
        assert!(Msg::Event(scroll("s 1 0 0 1")).wakes());
    }
}

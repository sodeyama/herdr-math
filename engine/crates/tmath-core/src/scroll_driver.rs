//! Scroll driver: turns decoded input events into scroll motion.
//!
//! Mouse wheel deltas (cell or pixel totals) and the fallback keys drive a
//! [`ScrollState`] through the [`Smooth`] profile. Units are terminal rows, so a
//! wheel scroll maps to a few rows and `PgUp`/`PgDn` to a page; `j`/`k`/`g`/`G`
//! cover the same rows as the navigational arrows.

use crate::input::{Event, KeyEvent};
use crate::mouse::{Key, MouseKind};
use crate::scroll::{ScrollState, Smooth};

/// Rows scrolled by one wheel notch.
const WHEEL_ROWS: f32 = 3.0;
/// Rows scrolled by an arrow or `j`/`k`.
const LINE_ROWS: f32 = 1.0;
/// Page size used for `PgUp`/`PgDn` and `g`/`G` when the viewport is unknown.
const DEFAULT_PAGE_ROWS: f32 = 20.0;

/// Maps a decoded input event to a scroll delta, or returns `None` when the
/// event does not drive scrolling. `max` is the current scrollable extent used
/// to clamp the target; `page` is the page size in rows when known.
pub fn scroll_delta(event: &Event, page: Option<f32>) -> Option<f32> {
    match event {
        Event::Mouse(mouse) => match mouse.kind {
            MouseKind::ScrollUp => Some(WHEEL_ROWS),
            MouseKind::ScrollDown => Some(-WHEEL_ROWS),
            _ => None,
        },
        Event::Key(KeyEvent {
            key: Key::Up | Key::Char('k'),
            ctrl: false,
            ..
        }) => Some(LINE_ROWS),
        Event::Key(KeyEvent {
            key: Key::Down | Key::Char('j'),
            ctrl: false,
            ..
        }) => Some(-LINE_ROWS),
        Event::Key(KeyEvent {
            key: Key::PageUp | Key::Char('g'),
            ctrl: false,
            ..
        }) => Some(page.unwrap_or(DEFAULT_PAGE_ROWS)),
        Event::Key(KeyEvent {
            key: Key::PageDown | Key::Char('G'),
            ctrl: false,
            ..
        }) => Some(-page.unwrap_or(DEFAULT_PAGE_ROWS)),
        Event::Key(KeyEvent {
            key: Key::Home,
            ctrl: false,
            ..
        }) => Some(f32::MAX),
        Event::Key(KeyEvent {
            key: Key::End,
            ctrl: false,
            ..
        }) => Some(f32::MIN),
        _ => None,
    }
}

/// Whether the event requests an immediate clean exit.
pub fn is_exit_signal(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(KeyEvent {
            key: Key::Char('q'),
            ctrl: false,
            alt: false,
        }) | Event::Key(KeyEvent {
            key: Key::Char('c'),
            ctrl: true,
            alt: false,
        })
    )
}

/// A scroll driver over a fixed maximum extent.
#[derive(Debug)]
pub struct ScrollDriver {
    state: ScrollState,
    smooth: Smooth,
    max: f32,
}

impl ScrollDriver {
    pub fn new(max: f32) -> Self {
        Self {
            state: ScrollState::default(),
            smooth: Smooth::default(),
            max: max.max(0.0),
        }
    }

    /// Feeds one decoded input event into the scroll state.
    pub fn handle(&mut self, event: &Event, page: Option<f32>) -> bool {
        if let Some(delta) = scroll_delta(event, page) {
            self.state.tick(&self.smooth, delta, self.max);
            true
        } else {
            false
        }
    }

    /// Advances one animation frame; returns the current scroll offset.
    pub fn step(&mut self, dt: f32) -> f32 {
        self.state.step(&self.smooth, dt, self.max);
        self.state.position
    }

    /// Current eased scroll offset.
    pub fn position(&self) -> f32 {
        self.state.position
    }

    /// Whether the scroll offset has reached its target.
    pub fn settled(&self) -> bool {
        self.state.settled()
    }

    /// Updates the scrollable extent (used when content grows or the terminal
    /// resizes).
    pub fn set_max(&mut self, max: f32) {
        self.max = max.max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Event, InputDecoder, KeyEvent};
    use crate::mouse::Key;

    fn decode(bytes: &[u8]) -> Event {
        let mut decoder = InputDecoder::new();
        decoder.push(bytes);
        decoder.next_event().expect("one event")
    }

    #[test]
    fn wheel_up_scrolls_forward_and_down_backward() {
        let up = decode(b"\x1b[<64;10;20M");
        let down = decode(b"\x1b[<65;10;20M");
        assert_eq!(scroll_delta(&up, None), Some(3.0));
        assert_eq!(scroll_delta(&down, None), Some(-3.0));
    }

    #[test]
    fn fallback_keys_map_to_rows_and_pages() {
        let key = |k| {
            Event::Key(KeyEvent {
                key: k,
                ctrl: false,
                alt: false,
            })
        };
        assert_eq!(scroll_delta(&key(Key::Up), None), Some(1.0));
        assert_eq!(scroll_delta(&key(Key::Char('j')), None), Some(-1.0));
        assert_eq!(scroll_delta(&key(Key::PageUp), Some(24.0)), Some(24.0));
        assert_eq!(scroll_delta(&key(Key::PageDown), None), Some(-20.0));
        assert_eq!(scroll_delta(&key(Key::Home), None), Some(f32::MAX));
        assert_eq!(scroll_delta(&key(Key::End), None), Some(f32::MIN));
    }

    #[test]
    fn exit_signals_are_q_and_ctrl_c() {
        assert!(is_exit_signal(&Event::Key(KeyEvent {
            key: Key::Char('q'),
            ctrl: false,
            alt: false
        })));
        assert!(is_exit_signal(&decode(b"\x03")));
        assert!(!is_exit_signal(&decode(b"a")));
    }

    #[test]
    fn driver_clamps_and_settles() {
        let mut driver = ScrollDriver::new(50.0);
        driver.handle(&decode(b"\x1b[<64;10;20M"), None);
        driver.step(1.0 / 60.0);
        assert!(driver.position() > 0.0);
        for _ in 0..600 {
            driver.step(1.0 / 60.0);
            if driver.settled() {
                break;
            }
        }
        assert_eq!(driver.position(), 3.0, "settles at the clamped target");
    }

    #[test]
    fn driver_negative_scroll_is_clamped_to_zero() {
        let mut driver = ScrollDriver::new(50.0);
        driver.handle(&decode(b"\x1b[<65;10;20M"), None);
        for _ in 0..600 {
            driver.step(1.0 / 60.0);
            if driver.settled() {
                break;
            }
        }
        assert_eq!(driver.position(), 0.0, "cannot scroll before the top");
    }
}

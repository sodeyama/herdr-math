//! SGR and pixel mouse input decoding.
//!
//! When the terminal runs with SGR mouse (`?1006h`) and optionally pixel mouse
//! (`?1016h`) enabled, presses and motion arrive as `CSI < b ; x ; y M` and
//! releases as `CSI < b ; x ; y m`. This module decodes those reports into a
//! [`MouseEvent`] and converts cell coordinates to pixel coordinates using the
//! measured cell size.

/// Which button is involved in a mouse report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

/// What kind of mouse activity a report describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Up,
    Move,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

/// Keyboard modifier bits carried by an SGR mouse report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// A decoded mouse report with terminal cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseButton,
    pub mods: Mods,
    pub x: u32,
    pub y: u32,
}

/// A complete decoded input event for the terminal input loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Mouse(MouseEvent),
    Key(KeyEvent),
}

/// A decoded keyboard event used by the scroll fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub ctrl: bool,
}

/// Named keys relevant to document scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    Delete,
    Backspace,
    Enter,
    Tab,
    Escape,
    Char(char),
    Unknown,
}

/// Decodes a single SGR mouse report from its parameter body and press flag.
///
/// `params` is the text between `CSI ` and the final byte (`M` for press or
/// motion, `m` for release). Returns `None` for malformed or zero-coordinate
/// reports so the caller treats them as unknown input.
pub fn parse_sgr_mouse(params: &[u8], press: bool) -> Option<MouseEvent> {
    let rest = params.strip_prefix(b"<")?;
    let mut fields = rest.split(|&b| b == b';');
    let mut next_int = || -> Option<u32> { std::str::from_utf8(fields.next()?).ok()?.parse().ok() };
    let b = next_int()?;
    let x = next_int()?;
    let y = next_int()?;
    if x == 0 || y == 0 {
        return None;
    }

    let button = match b & 3 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        _ => MouseButton::None,
    };
    let mods = Mods {
        shift: b & 4 != 0,
        alt: b & 8 != 0,
        ctrl: b & 16 != 0,
    };
    let kind = if b & 64 != 0 {
        match b & 3 {
            0 => MouseKind::ScrollUp,
            1 => MouseKind::ScrollDown,
            2 => MouseKind::ScrollLeft,
            _ => MouseKind::ScrollRight,
        }
    } else if b & 32 != 0 {
        MouseKind::Move
    } else if press {
        MouseKind::Down
    } else {
        MouseKind::Up
    };
    Some(MouseEvent {
        kind,
        button,
        mods,
        x,
        y,
    })
}

/// Parses a complete CSI sequence's parameter body into an input event.
///
/// `terminator` is the final byte of the sequence. Mouse reports use `M` (press
/// or motion) and `m` (release). A `t` reply carries window size. Everything
/// else that matters for scrolling falls through to the fallback keys.
pub fn parse_csi_param(params: &[u8], terminator: u8) -> Option<InputEvent> {
    match terminator {
        b'M' | b'm' => parse_sgr_mouse(params, terminator == b'M').map(InputEvent::Mouse),
        b't' => None,
        b'A' => Some(InputEvent::Key(KeyEvent {
            key: Key::Up,
            ctrl: false,
        })),
        b'B' => Some(InputEvent::Key(KeyEvent {
            key: Key::Down,
            ctrl: false,
        })),
        b'C' => Some(InputEvent::Key(KeyEvent {
            key: Key::Right,
            ctrl: false,
        })),
        b'D' => Some(InputEvent::Key(KeyEvent {
            key: Key::Left,
            ctrl: false,
        })),
        b'H' => Some(InputEvent::Key(KeyEvent {
            key: Key::Home,
            ctrl: false,
        })),
        b'F' => Some(InputEvent::Key(KeyEvent {
            key: Key::End,
            ctrl: false,
        })),
        b'~' => {
            let number = params
                .split(|&b| b == b';')
                .next()
                .and_then(|first| std::str::from_utf8(first).ok())
                .and_then(|first| first.split(':').next())
                .and_then(|first| first.parse::<u32>().ok());
            let key = match number {
                Some(3) => Key::Delete,
                Some(5) => Key::PageUp,
                Some(6) => Key::PageDown,
                _ => Key::Unknown,
            };
            Some(InputEvent::Key(KeyEvent { key, ctrl: false }))
        }
        _ => None,
    }
}

/// Converts cell coordinates to pixel coordinates at the center of the cell.
pub fn cell_to_pixel(x: u32, y: u32, cell_width: u32, cell_height: u32) -> (u32, u32) {
    (
        x.saturating_sub(1).saturating_mul(cell_width) + cell_width / 2,
        y.saturating_sub(1).saturating_mul(cell_height) + cell_height / 2,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses the body of a press report (trailing terminator byte stripped).
    fn mouse(body: &str) -> MouseEvent {
        let bytes = body.as_bytes();
        parse_sgr_mouse(&bytes[..bytes.len() - 1], true).unwrap()
    }

    #[test]
    fn decodes_scroll_up_and_down() {
        assert_eq!(
            mouse("<64;10;20M"),
            MouseEvent {
                kind: MouseKind::ScrollUp,
                button: MouseButton::Left,
                mods: Mods {
                    shift: false,
                    alt: false,
                    ctrl: false
                },
                x: 10,
                y: 20,
            }
        );
        assert_eq!(mouse("<65;10;20M").kind, MouseKind::ScrollDown);
    }

    #[test]
    fn decodes_button_and_modifier_bits() {
        let event = mouse("<2;5;6M");
        assert_eq!(event.button, MouseButton::Right);
        assert_eq!(event.kind, MouseKind::Down);
        let event = mouse("<19;5;6M");
        assert!(event.mods.ctrl);
        assert!(!event.mods.shift);
        assert!(!event.mods.alt);
    }

    #[test]
    fn decodes_motion_and_release() {
        assert_eq!(mouse("<35;3;4M").kind, MouseKind::Move);
        let bytes = b"<0;3;4m";
        assert_eq!(
            parse_sgr_mouse(&bytes[..bytes.len() - 1], false)
                .unwrap()
                .kind,
            MouseKind::Up
        );
    }

    #[test]
    fn rejects_zero_coordinates() {
        assert_eq!(parse_sgr_mouse(b"<0;0;0", true), None);
        assert_eq!(parse_sgr_mouse(b"<0;0;6", true), None);
        assert_eq!(parse_sgr_mouse(b"<0;5;0", true), None);
    }

    #[test]
    fn rejects_malformed_reports() {
        assert_eq!(parse_sgr_mouse(b"<a;b;c", true), None);
        assert_eq!(parse_sgr_mouse(b"64;10;20", true), None);
        assert_eq!(parse_sgr_mouse(b"<64;10", true), None);
    }

    #[test]
    fn csi_framing_dispatches_mouse_and_keys() {
        assert_eq!(
            parse_csi_param(b"<64;10;20", b'M').unwrap(),
            InputEvent::Mouse(mouse("<64;10;20M"))
        );
        assert_eq!(
            parse_csi_param(b"", b'B').unwrap(),
            InputEvent::Key(KeyEvent {
                key: Key::Down,
                ctrl: false
            })
        );
        assert_eq!(
            parse_csi_param(b"5", b'~').unwrap(),
            InputEvent::Key(KeyEvent {
                key: Key::PageUp,
                ctrl: false
            })
        );
    }

    #[test]
    fn cell_to_pixel_returns_center_of_cell() {
        assert_eq!(cell_to_pixel(1, 1, 10, 20), (5, 10));
        assert_eq!(cell_to_pixel(3, 4, 10, 20), (25, 70));
        assert_eq!(cell_to_pixel(0, 0, 10, 20), (5, 10), "clamped below one");
    }
}

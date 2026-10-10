//! X10, UTF-8 and SGR mouse report encoding.

use crate::input::keyboard::Modifiers;
use crate::term::TermMode;

pub const WHEEL_UP: u8 = 64;
pub const WHEEL_DOWN: u8 = 65;
pub const WHEEL_LEFT: u8 = 66;
pub const WHEEL_RIGHT: u8 = 67;

/// Motion offset added to a button code while it is held.
pub const MOTION: u8 = 32;
/// Motion code without any held button.
pub const MOTION_NO_BUTTON: u8 = 35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    pub fn code(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        }
    }
}

/// Encode a mouse report for a viewport cell, or `None` when it can't be represented.
pub fn report(
    button: u8,
    pressed: bool,
    column: usize,
    line: usize,
    mods: Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mut code = button;
    if mods.contains(Modifiers::SHIFT) {
        code += 4;
    }
    if mods.contains(Modifiers::ALT) {
        code += 8;
    }
    if mods.contains(Modifiers::CONTROL) {
        code += 16;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let action = if pressed { 'M' } else { 'm' };
        Some(format!("\x1b[<{code};{};{}{action}", column + 1, line + 1).into_bytes())
    } else {
        // Releases don't identify their button outside of SGR mode.
        let code = if pressed { code } else { 3 + (code & !3) };
        normal_report(code, column, line, mode.contains(TermMode::UTF8_MOUSE))
    }
}

fn normal_report(button: u8, column: usize, line: usize, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };
    if line >= max_point || column >= max_point {
        return None;
    }

    let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

    let mut encode_position = |position: usize| {
        if utf8 && position >= 95 {
            let position = 32 + 1 + position;
            msg.push((0xC0 + position / 64) as u8);
            msg.push((0x80 + (position & 63)) as u8);
        } else {
            msg.push(32 + 1 + position as u8);
        }
    };
    encode_position(column);
    encode_position(line);

    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_reports_press_and_release() {
        let mode = TermMode::SGR_MOUSE;
        assert_eq!(report(0, true, 4, 2, Modifiers::empty(), mode).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(report(0, false, 4, 2, Modifiers::empty(), mode).unwrap(), b"\x1b[<0;5;3m");
        assert_eq!(report(2, true, 0, 0, Modifiers::CONTROL, mode).unwrap(), b"\x1b[<18;1;1M");
    }

    #[test]
    fn normal_reports_are_bounded() {
        let mode = TermMode::empty();
        assert_eq!(report(0, true, 0, 0, Modifiers::empty(), mode).unwrap(), b"\x1b[M !!");
        assert_eq!(report(0, false, 0, 0, Modifiers::empty(), mode).unwrap(), b"\x1b[M#!!");
        assert!(report(0, true, 300, 0, Modifiers::empty(), mode).is_none());
        let utf8 = report(0, true, 100, 0, Modifiers::empty(), TermMode::UTF8_MOUSE).unwrap();
        assert_eq!(utf8, [0x1b, b'[', b'M', 32, 0xC2, 0x85, 33]);
    }
}

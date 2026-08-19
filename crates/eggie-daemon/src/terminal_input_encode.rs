//! Input encoding: paste bracketing, mouse report bytes (SGR/UTF8/legacy), modifier/button codes,
//! and RGB<->u32 color conversion. Stateless pure functions extracted verbatim from `lib.rs`.
//! Callers reach them via the crate-root `use terminal_input_encode::*` re-export.

use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Rgb;
use eggie_protocol::{
    TerminalInputModes, TerminalModifiers, TerminalMouseAction, TerminalMouseButton,
    TerminalMouseEncoding, TerminalMouseEvent, TerminalMousePosition, TerminalMouseTracking,
};

pub(crate) fn rgba_from_rgb(color: Rgb) -> u32 {
    u32::from_be_bytes([color.r, color.g, color.b, 0xff])
}

pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

pub(crate) fn terminal_mouse_mode(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::VI)
}

pub(crate) fn terminal_input_modes(mode: TermMode) -> TerminalInputModes {
    let mouse_tracking = if !terminal_mouse_mode(mode) {
        TerminalMouseTracking::Disabled
    } else if mode.contains(TermMode::MOUSE_MOTION) {
        TerminalMouseTracking::Motion
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        TerminalMouseTracking::Drag
    } else {
        TerminalMouseTracking::Click
    };
    let mouse_encoding = if mode.contains(TermMode::SGR_MOUSE) {
        TerminalMouseEncoding::Sgr
    } else if mode.contains(TermMode::UTF8_MOUSE) {
        TerminalMouseEncoding::Utf8
    } else {
        TerminalMouseEncoding::Legacy
    };
    TerminalInputModes {
        mouse_tracking,
        mouse_encoding,
        focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
        alternate_screen: mode.contains(TermMode::ALT_SCREEN),
        alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        paste_events: mode.contains(TermMode::PASTE_EVENTS),
        kitty_keyboard_flags: ((mode & TermMode::KITTY_KEYBOARD_PROTOCOL).bits() >> 18) as u8,
    }
}

pub(crate) fn terminal_modifier_code(modifiers: TerminalModifiers) -> u8 {
    u8::from(modifiers.shift) * 4 + u8::from(modifiers.alt) * 8 + u8::from(modifiers.control) * 16
}

pub(crate) fn terminal_button_code(button: TerminalMouseButton) -> u8 {
    match button {
        TerminalMouseButton::Left => 0,
        TerminalMouseButton::Middle => 1,
        TerminalMouseButton::Right => 2,
    }
}

pub(crate) fn mouse_report_bytes(
    mode: TermMode,
    display_offset: usize,
    event: TerminalMouseEvent,
) -> Option<Vec<u8>> {
    if !terminal_mouse_mode(mode) {
        return None;
    }

    let (button, released) = match event.action {
        TerminalMouseAction::Press | TerminalMouseAction::Release => {
            if event.modifiers.shift {
                return None;
            }
            let button = terminal_button_code(event.button?);
            (button, event.action == TerminalMouseAction::Release)
        }
        TerminalMouseAction::Move => {
            if event.modifiers.shift
                && matches!(
                    event.button,
                    Some(TerminalMouseButton::Left | TerminalMouseButton::Right)
                )
            {
                return None;
            }
            if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
                return None;
            }
            let button = match event.button {
                Some(button) => terminal_button_code(button),
                None if mode.contains(TermMode::MOUSE_MOTION) => 3,
                None => return None,
            } + 32;
            (button, false)
        }
    };

    mouse_report_from_code(
        mode,
        display_offset,
        event.position,
        button,
        released,
        event.modifiers,
    )
}

pub(crate) fn mouse_report_from_code(
    mode: TermMode,
    display_offset: usize,
    position: TerminalMousePosition,
    button: u8,
    released: bool,
    modifiers: TerminalModifiers,
) -> Option<Vec<u8>> {
    let line = usize::from(position.row).checked_sub(display_offset)?;
    let column = usize::from(position.column);
    let modifier_code = terminal_modifier_code(modifiers);

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if released { 'm' } else { 'M' };
        let (x, y) = if mode.contains(TermMode::SGR_PIXEL_MOUSE) {
            (
                usize::try_from(position.pixel_x).ok()?.saturating_add(1),
                usize::try_from(position.pixel_y).ok()?.saturating_add(1),
            )
        } else {
            (column + 1, line + 1)
        };
        return Some(
            format!("\x1b[<{};{};{}{}", button + modifier_code, x, y, suffix).into_bytes(),
        );
    }

    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let max_point = if utf8 { 2015 } else { 223 };
    if line >= max_point || column >= max_point {
        return None;
    }

    let button = if released { 3 } else { button } + modifier_code;
    let mut report = vec![b'\x1b', b'[', b'M', 32 + button];
    encode_legacy_mouse_coordinate(&mut report, column, utf8);
    encode_legacy_mouse_coordinate(&mut report, line, utf8);
    Some(report)
}

pub(crate) fn encode_legacy_mouse_coordinate(report: &mut Vec<u8>, coordinate: usize, utf8: bool) {
    let coordinate = coordinate + 33;
    if utf8 && coordinate >= 128 {
        report.push((0xc0 + coordinate / 64) as u8);
        report.push((0x80 + (coordinate & 63)) as u8);
    } else {
        report.push(coordinate as u8);
    }
}

pub(crate) fn rgb_from_u32(color: u32) -> Rgb {
    Rgb {
        r: ((color >> 16) & 0xff) as u8,
        g: ((color >> 8) & 0xff) as u8,
        b: (color & 0xff) as u8,
    }
}

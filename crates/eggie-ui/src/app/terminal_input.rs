//! Pure, `EggieApp`-free helpers that encode GPUI keyboard/mouse events into terminal bytes and
//! convert screen coordinates into terminal cell positions. Moved verbatim out of `app.rs`; the
//! logic is unchanged. Consumed by `app.rs` production call sites and its inline `#[cfg(test)]`
//! module through `use terminal_input::*`.

use super::TerminalViewport;
use super::{KITTY_REPORT_ALL_KEYS, KITTY_REPORT_ASSOCIATED_TEXT, KITTY_REPORT_EVENT_TYPES};
use crate::terminal_renderer::TerminalPoint;
use eggie_protocol::{TerminalMousePosition, TerminalSelectionSide, TerminalSnapshot};
use gpui::{KeyDownEvent, Keystroke, Pixels};

#[cfg(test)]
pub(crate) fn terminal_bytes(event: &KeyDownEvent) -> Option<Vec<u8>> {
    terminal_bytes_with_mode(event, 0)
}

pub(crate) fn terminal_bytes_with_mode(event: &KeyDownEvent, keyboard_flags: u8) -> Option<Vec<u8>> {
    if keyboard_flags != 0 {
        return kitty_key_bytes(
            &event.keystroke,
            keyboard_flags,
            if event.is_held { 2 } else { 1 },
        );
    }

    let key = event.keystroke.key.as_str();
    if event.keystroke.modifiers.platform {
        return None;
    }
    let mut bytes = match key {
        "enter" => vec![b'\r'],
        "backspace" => vec![0x7f],
        "tab" => vec![b'\t'],
        "escape" => vec![0x1b],
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        _ if event.keystroke.modifiers.control && key.is_ascii() && key.len() == 1 => {
            vec![key.as_bytes()[0].to_ascii_uppercase() & 0x1f]
        }
        _ if event.keystroke.modifiers.function => return None,
        _ => terminal_key_char_bytes(event.keystroke.key_char.as_deref()?)?,
    };
    if event.keystroke.modifiers.alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

pub(crate) fn kitty_key_bytes(keystroke: &Keystroke, flags: u8, event_type: u8) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let modifiers = 1
        + u8::from(keystroke.modifiers.shift)
        + u8::from(keystroke.modifiers.alt) * 2
        + u8::from(keystroke.modifiers.control) * 4
        + u8::from(keystroke.modifiers.platform) * 8;
    let modifier_field = if flags & KITTY_REPORT_EVENT_TYPES != 0 {
        format!("{modifiers}:{event_type}")
    } else {
        modifiers.to_string()
    };

    let csi_final = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(final_byte) = csi_final {
        return Some(format!("\x1b[1;{modifier_field}{final_byte}").into_bytes());
    }

    let tilde_code = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(code) = tilde_code {
        return Some(format!("\x1b[{code};{modifier_field}~").into_bytes());
    }

    let f_final = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(final_byte) = f_final {
        return Some(format!("\x1b[1;{modifier_field}{final_byte}").into_bytes());
    }

    // AppKit uses the Fn modifier to transform known navigation/function keys.
    // It is not a Kitty protocol modifier of its own, so only discard an
    // otherwise unknown Fn chord after giving transformed keys a chance above.
    if keystroke.modifiers.function {
        return None;
    }

    let primary = match key {
        "enter" => 13,
        "tab" => 9,
        "backspace" => 127,
        "escape" => 27,
        _ => key
            .chars()
            .next()
            .filter(|_| key.chars().count() == 1)
            .map(u32::from)
            .or_else(|| {
                keystroke
                    .key_char
                    .as_deref()
                    .and_then(|text| text.chars().next())
                    .map(u32::from)
            })?,
    };

    let printable_text = keystroke
        .key_char
        .as_deref()
        .filter(|text| !text.is_empty() && !text.chars().any(is_reserved_key_character));
    let must_escape = flags & KITTY_REPORT_ALL_KEYS != 0
        || modifiers != 1
        || matches!(key, "enter" | "tab" | "backspace" | "escape");
    if !must_escape {
        return (event_type != 3)
            .then(|| printable_text.map(|text| text.as_bytes().to_vec()))
            .flatten();
    }

    let associated = (flags & KITTY_REPORT_ASSOCIATED_TEXT != 0 && event_type != 3)
        .then_some(printable_text)
        .flatten()
        // GPUI exposes the physical character for Ctrl chords even though the
        // generated terminal text is a C0 control. Kitty forbids control codes
        // in the associated-text field, so do not report that physical value.
        .filter(|_| !keystroke.modifiers.control)
        .map(|text| {
            text.chars()
                .map(|character| u32::from(character).to_string())
                .collect::<Vec<_>>()
                .join(":")
        });
    let mut sequence = format!("\x1b[{primary};{modifier_field}");
    if let Some(associated) = associated {
        sequence.push(';');
        sequence.push_str(&associated);
    }
    sequence.push('u');
    Some(sequence.into_bytes())
}

pub(crate) fn is_character_palette_shortcut(event: &KeyDownEvent) -> bool {
    let modifiers = event.keystroke.modifiers;
    modifiers.platform && modifiers.control && event.keystroke.key == "space"
}

pub(crate) fn terminal_text_bytes(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

pub(crate) fn terminal_key_char_bytes(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.chars().any(is_reserved_key_character) {
        return None;
    }
    Some(terminal_text_bytes(text))
}

pub(crate) fn is_reserved_key_character(character: char) -> bool {
    let codepoint = character as u32;
    character.is_control()
        || (0xf700..=0xf8ff).contains(&codepoint)
        || (0xfdd0..=0xfdef).contains(&codepoint)
        || codepoint & 0xffff == 0xfffe
        || codepoint & 0xffff == 0xffff
}

pub(crate) fn terminal_point_from_position(
    viewport: TerminalViewport,
    position: gpui::Point<Pixels>,
) -> Option<TerminalPoint> {
    if viewport.rows == 0
        || viewport.columns == 0
        || viewport.cell_width <= 0.
        || viewport.line_height <= 0.
    {
        return None;
    }
    let x = f32::from(position.x - viewport.bounds.origin.x);
    let y = f32::from(position.y - viewport.bounds.origin.y);
    Some(TerminalPoint {
        line: (y / viewport.line_height)
            .floor()
            .clamp(0., (viewport.rows - 1) as f32) as u16,
        column: (x / viewport.cell_width)
            .floor()
            .clamp(0., (viewport.columns - 1) as f32) as u16,
    })
}

/// Which half of its cell a pixel position falls in. The daemon uses this as the selection
/// endpoint's `Side`, so a selection can include or exclude the cell under the cursor depending on
/// whether the pointer is past the cell's midpoint.
pub(crate) fn terminal_selection_side(
    viewport: TerminalViewport,
    position: gpui::Point<Pixels>,
) -> TerminalSelectionSide {
    if viewport.cell_width <= 0. {
        return TerminalSelectionSide::Left;
    }
    let x = f32::from(position.x - viewport.bounds.origin.x);
    let fraction = (x / viewport.cell_width).fract();
    if fraction >= 0.5 {
        TerminalSelectionSide::Right
    } else {
        TerminalSelectionSide::Left
    }
}

/// Convert a viewport point (renderer coordinate) into the protocol cell position sent to the
/// daemon. Both are 0-based viewport line/column; only the type differs across the crate boundary.
pub(crate) fn cell_position(point: TerminalPoint) -> eggie_protocol::TerminalCellPosition {
    eggie_protocol::TerminalCellPosition {
        line: point.line,
        column: point.column,
    }
}

pub(crate) fn terminal_hyperlink_at(snapshot: &TerminalSnapshot, point: TerminalPoint) -> Option<String> {
    snapshot
        .cells
        .iter()
        .find(|cell| cell.line == point.line && cell.column == point.column)
        .and_then(|cell| cell.hyperlink.clone())
}

/// The auto-detected URL range covering `point`, if any. Ranges are per-row, so a hit is a simple
/// line match plus column containment.
pub(crate) fn detected_link_at(
    snapshot: &TerminalSnapshot,
    point: TerminalPoint,
) -> Option<&eggie_protocol::TerminalLinkRange> {
    snapshot.detected_links.iter().find(|link| {
        link.start.line == point.line
            && point.column >= link.start.column
            && point.column <= link.end.column
    })
}

pub(crate) fn url_has_safe_external_scheme(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

pub(crate) fn protocol_mouse_position(
    point: TerminalPoint,
    viewport: TerminalViewport,
    position: gpui::Point<Pixels>,
) -> TerminalMousePosition {
    let x = f32::from(position.x - viewport.bounds.origin.x);
    let y = f32::from(position.y - viewport.bounds.origin.y);
    let pixel_width = viewport.cell_width * viewport.columns as f32;
    let pixel_height = viewport.line_height * viewport.rows as f32;
    TerminalMousePosition {
        column: point.column,
        row: point.line,
        pixel_x: x.floor().clamp(0., (pixel_width - 1.).max(0.)) as u32,
        pixel_y: y.floor().clamp(0., (pixel_height - 1.).max(0.)) as u32,
    }
}

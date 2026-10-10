use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMouseAction {
    Press,
    Release,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMousePosition {
    /// Zero-based viewport column.
    pub column: u16,
    /// Zero-based viewport row.
    pub row: u16,
    /// Zero-based horizontal pixel coordinate within the terminal viewport.
    #[serde(default)]
    pub pixel_x: u32,
    /// Zero-based vertical pixel coordinate within the terminal viewport.
    #[serde(default)]
    pub pixel_y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalMouseEvent {
    pub action: TerminalMouseAction,
    /// The active button for press/release and drag motion. `None` represents unbuttoned motion.
    pub button: Option<TerminalMouseButton>,
    pub position: TerminalMousePosition,
    pub modifiers: TerminalModifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalScrollUnit {
    Pixels,
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalScrollPhase {
    Started,
    Moved,
    Ended,
}

/// Discrete viewport scroll commands mapping directly onto the terminal core's scroll primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalScrollCommand {
    /// Jump to the oldest scrollback line.
    Top,
    /// Jump to the live bottom of the buffer.
    Bottom,
    /// Scroll up one viewport page.
    PageUp,
    /// Scroll down one viewport page.
    PageDown,
    /// Scroll to the previous (older) shell prompt marked via OSC 133. No-op without shell
    /// integration or when already at the oldest recorded prompt.
    PrevPrompt,
    /// Scroll to the next (newer) shell prompt marked via OSC 133.
    NextPrompt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScrollDelta {
    /// Horizontal delta multiplied by [`TERMINAL_SCROLL_DELTA_SCALE`].
    pub x: i32,
    /// Vertical delta multiplied by [`TERMINAL_SCROLL_DELTA_SCALE`].
    pub y: i32,
    pub unit: TerminalScrollUnit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalScrollEvent {
    pub delta: TerminalScrollDelta,
    pub phase: TerminalScrollPhase,
    pub position: TerminalMousePosition,
    pub modifiers: TerminalModifiers,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMouseTracking {
    #[default]
    Disabled,
    Click,
    Drag,
    Motion,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMouseEncoding {
    #[default]
    Legacy,
    Utf8,
    Sgr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputModes {
    pub mouse_tracking: TerminalMouseTracking,
    pub mouse_encoding: TerminalMouseEncoding,
    pub focus_reporting: bool,
    pub alternate_screen: bool,
    pub alternate_scroll: bool,
    /// Kitty OSC 5522 paste-event notifications are enabled through DECSET 5522.
    #[serde(default, skip_serializing_if = "is_false")]
    pub paste_events: bool,
    /// Active Kitty progressive-keyboard-enhancement flags.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub kitty_keyboard_flags: u8,
}

impl TerminalInputModes {
    pub fn captures_mouse(self) -> bool {
        self.mouse_tracking != TerminalMouseTracking::Disabled
    }
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

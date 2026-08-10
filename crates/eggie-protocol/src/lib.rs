use eggie_domain::{ProjectId, SessionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub const PROTOCOL_VERSION: u32 = 25;

/// Fixed-point scale used by [`TerminalScrollDelta`]. Keeping scroll deltas integral preserves
/// sub-pixel trackpad motion without introducing non-reflexive floating-point values into the
/// wire protocol.
pub const TERMINAL_SCROLL_DELTA_SCALE: i32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            columns: 100,
            rows: 30,
            cell_width: 8,
            cell_height: 18,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalProgressState {
    Normal,
    Error,
    Indeterminate,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProgress {
    pub state: TerminalProgressState,
    pub percent: Option<u8>,
    /// Unix time in milliseconds. Used only for user-facing recency text; daemon timeout logic
    /// uses a monotonic clock.
    pub updated_at_unix_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProgressUpdate {
    pub session_id: SessionId,
    pub revision: u64,
    pub progress: Option<TerminalProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProgressTimeouts {
    pub completed_ms: u32,
    pub stale_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalReportedLocation {
    pub user: Option<String>,
    pub host: Option<String>,
    pub path: String,
    pub local: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSemanticPhase {
    None,
    Prompt,
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCommandRecord {
    pub id: u64,
    pub command_line: Option<String>,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalUserVariable {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalShellIntegrationState {
    pub phase: TerminalSemanticPhase,
    pub current_command: Option<TerminalCommandRecord>,
    pub history: Vec<TerminalCommandRecord>,
    pub user_variables: Vec<TerminalUserVariable>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClipboardSelection {
    Clipboard,
    Primary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalClipboardContent {
    pub mime_type: String,
    #[serde(with = "base64_bytes")]
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalNotification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub activation_response_requested: bool,
    pub close_response_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAttentionRequest {
    Once,
    Continuous,
    Fireworks,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFileTransferProtocol {
    Iterm2,
    Kitty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalFileTransferOffer {
    pub request_id: u64,
    pub protocol: TerminalFileTransferProtocol,
    pub suggested_name: String,
    pub size: u64,
    pub directory: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalOscEventPayload {
    Notification {
        notification: TerminalNotification,
    },
    NotificationClose {
        id: String,
    },
    ClipboardWrite {
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
    },
    ClipboardRead {
        request_id: u64,
        selection: TerminalClipboardSelection,
        mime_types: Vec<String>,
    },
    OpenUrl {
        request_id: u64,
        url: String,
    },
    FocusRequest,
    /// The terminal rang the bell (BEL / `\a`). Carries no payload; the UI decides how to react
    /// (visual flash, sound, or nothing) based on the user's bell-mode setting.
    Bell,
    AttentionRequest {
        request: TerminalAttentionRequest,
    },
    FileTransfer {
        offer: TerminalFileTransferOffer,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOscEvent {
    pub revision: u64,
    pub payload: TerminalOscEventPayload,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalOscEventUpdate {
    pub session_id: SessionId,
    pub revision: u64,
    pub events: Vec<TerminalOscEvent>,
}

impl Default for TerminalProgressTimeouts {
    fn default() -> Self {
        Self {
            completed_ms: 5_000,
            stale_ms: 60_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    Handshake {
        protocol_version: u32,
    },
    CreateSession {
        project_id: ProjectId,
        cwd: PathBuf,
        size: TerminalSize,
        appearance: TerminalAppearance,
        /// Scrollback history depth in lines (`0` disables scrollback). The client always sends its
        /// resolved value; `#[serde(default)]` only guards deserializing a request that predates the
        /// field, which the version handshake already rules out in practice.
        #[serde(default)]
        scrollback_limit: usize,
        /// Shell executable to launch. Empty = the daemon falls back to `$SHELL` (then `/bin/zsh`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        shell_program: String,
        /// Custom arguments for the shell. Empty = Eggie's default launch args (`-l`, plus the
        /// mandatory `--posix` for bash integration).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        shell_args: Vec<String>,
    },
    ListSessions,
    InspectSession {
        session_id: SessionId,
    },
    Snapshot {
        session_id: SessionId,
    },
    WaitForSnapshot {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    WaitForProgress {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    WaitForOscEvents {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    GetShellIntegrationState {
        session_id: SessionId,
    },
    CompleteClipboardRead {
        session_id: SessionId,
        request_id: u64,
        contents: Vec<TerminalClipboardContent>,
    },
    NotificationResponse {
        session_id: SessionId,
        notification_id: String,
        activated: bool,
    },
    CompleteFileTransfer {
        session_id: SessionId,
        request_id: u64,
        destination: Option<PathBuf>,
    },
    TerminalImage {
        session_id: SessionId,
        key: TerminalImageKey,
        offset: u32,
        length: u32,
    },
    Input {
        session_id: SessionId,
        bytes: Vec<u8>,
        sequence: u64,
    },
    Paste {
        session_id: SessionId,
        text: String,
        sequence: u64,
    },
    PasteClipboard {
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
    },
    Mouse {
        session_id: SessionId,
        event: TerminalMouseEvent,
    },
    Scroll {
        session_id: SessionId,
        event: TerminalScrollEvent,
    },
    /// Scroll the viewport by a discrete command (page up/down, jump to top/bottom). Unlike
    /// [`ClientRequest::Scroll`] this maps directly onto the terminal core's `Scroll` primitives,
    /// giving exact page and boundary semantics without the wheel accumulator.
    TerminalScrollTo {
        session_id: SessionId,
        command: TerminalScrollCommand,
    },
    /// Begin an interactive selection at a viewport cell. The daemon converts the viewport point to
    /// an absolute grid point (accounting for the current scroll offset) and stores it as the
    /// terminal core's authoritative `Selection`, which stays valid across scrollback growth.
    TerminalSelectionStart {
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
        kind: TerminalSelectionKind,
    },
    /// Extend the active selection's head to a viewport cell (drag). Consecutive updates are merged
    /// on the daemon's input queue so a fast drag does not flood the terminal core.
    TerminalSelectionUpdate {
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    },
    /// Clear the active selection (e.g. a click that did not turn into a drag).
    TerminalSelectionClear {
        session_id: SessionId,
    },
    /// Select the entire buffer including scrollback.
    TerminalSelectAll {
        session_id: SessionId,
    },
    /// Extract the current selection's text (whole scrollback span, soft-wrap unwrapped, trailing
    /// whitespace trimmed) and return it in [`DaemonResponse::SelectionText`].
    TerminalCopySelection {
        session_id: SessionId,
    },
    /// Search the terminal grid (including scrollback) for `query`. The daemon runs the search in
    /// its terminal core, scrolls the viewport so the active match is visible, publishes a fresh
    /// snapshot, and replies with [`DaemonResponse::SearchResult`]. Coordinates in the reply are
    /// viewport-relative, matching the snapshot the client renders against.
    TerminalSearch {
        session_id: SessionId,
        request: TerminalSearchRequest,
    },
    Focus {
        session_id: SessionId,
        focused: bool,
    },
    Resize {
        session_id: SessionId,
        size: TerminalSize,
    },
    SetAppearance {
        session_id: SessionId,
        appearance: TerminalAppearance,
    },
    SetProgressTimeouts {
        session_id: SessionId,
        timeouts: TerminalProgressTimeouts,
    },
    SetOscPolicy {
        session_id: SessionId,
        allow_clipboard_read: bool,
    },
    /// Enable or disable bare-URL auto-detection for a session.
    SetUrlDetection {
        session_id: SessionId,
        detect_urls: bool,
    },
    /// Set the session's default cursor shape. A running program can still override this at runtime
    /// via DECSCUSR (`CSI Ps SP q`); this only changes the shape used when the program hasn't asked.
    SetCursorStyle {
        session_id: SessionId,
        shape: TerminalCursorShape,
    },
    /// Set the session's scrollback history depth at runtime (`0` disables scrollback). Shrinking
    /// below the current history evicts the oldest lines; the change applies to the live terminal.
    SetScrollbackLimit {
        session_id: SessionId,
        limit: usize,
    },
    Terminate {
        session_id: SessionId,
    },
}

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

fn is_false(value: &bool) -> bool {
    !*value
}

impl TerminalInputModes {
    pub fn captures_mouse(self) -> bool {
        self.mouse_tracking != TerminalMouseTracking::Disabled
    }
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub pid: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    /// Process CPU usage in tenths of a percentage point. Optional so a newer
    /// client can keep inspecting sessions owned by an already-running daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage_tenths_percent: Option<u32>,
    /// Resident memory in bytes. See the compatibility note on CPU usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListeningPort {
    pub pid: u32,
    pub protocol: String,
    pub address: String,
    pub port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInspection {
    pub session_id: SessionId,
    pub processes: Vec<ProcessInfo>,
    pub ports: Vec<ListeningPort>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: String,
    pub initial_directory: PathBuf,
    pub current_directory: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_location: Option<TerminalReportedLocation>,
    pub shell_pid: u32,
    pub current_process: ProcessSummary,
    pub status: SessionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Exited { code: Option<i32> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalColor {
    Rgb(u32),
    Indexed(u8),
    Named(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAppearance {
    /// ANSI colors 0 through 15, encoded as `0xRRGGBB`.
    pub palette: [u32; 16],
    /// Default terminal colors, encoded as `0xRRGGBB`.
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub cursor_text: u32,
}

impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            palette: [
                0x1d2027, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
            ],
            foreground: 0xabb2bf,
            background: 0x1d2027,
            cursor: 0xabb2bf,
            cursor_text: 0x1d2027,
        }
    }
}

impl TerminalAppearance {
    pub fn color(&self, index: usize) -> Option<u32> {
        match index {
            0..=15 => Some(self.palette[index]),
            16..=231 => {
                const STEPS: [u32; 6] = [0, 95, 135, 175, 215, 255];
                let value = index - 16;
                Some((STEPS[value / 36] << 16) | (STEPS[(value / 6) % 6] << 8) | STEPS[value % 6])
            }
            232..=255 => {
                let value = 8 + 10 * (index as u32 - 232);
                Some((value << 16) | (value << 8) | value)
            }
            256 | 267 => Some(self.foreground),
            257 => Some(self.background),
            258 => Some(self.cursor),
            259..=266 => Some(dim_rgb(self.palette[index - 259])),
            268 => Some(dim_rgb(self.foreground)),
            _ => None,
        }
    }
}

fn dim_rgb(color: u32) -> u32 {
    let dim = |shift: u32| ((((color >> shift) & 0xff) as f32 * 0.66) as u32) & 0xff;
    (dim(16) << 16) | (dim(8) << 8) | dim(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalColorOverride {
    #[serde(rename = "i")]
    pub index: u16,
    /// Dynamic terminal color encoded as `0xRRGGBBAA`.
    #[serde(rename = "c")]
    pub color: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCell {
    #[serde(rename = "l")]
    pub line: u16,
    #[serde(rename = "c")]
    pub column: u16,
    #[serde(rename = "ch")]
    pub character: char,
    #[serde(rename = "z", default, skip_serializing_if = "Vec::is_empty")]
    pub zerowidth: Vec<char>,
    #[serde(rename = "fg")]
    pub foreground: TerminalColor,
    #[serde(rename = "bg")]
    pub background: TerminalColor,
    #[serde(rename = "u", default, skip_serializing_if = "Option::is_none")]
    pub underline_color: Option<TerminalColor>,
    #[serde(rename = "h", default, skip_serializing_if = "Option::is_none")]
    pub hyperlink: Option<String>,
    #[serde(rename = "f")]
    pub flags: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerminalImageKey {
    #[serde(rename = "i")]
    pub id: u32,
    #[serde(rename = "g")]
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalImageDescriptor {
    #[serde(rename = "k")]
    pub key: TerminalImageKey,
    #[serde(rename = "w")]
    pub width: u32,
    #[serde(rename = "h")]
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalImagePlacement {
    #[serde(rename = "k")]
    pub image: TerminalImageKey,
    #[serde(rename = "p")]
    pub placement_id: u32,
    #[serde(rename = "l")]
    pub line: i32,
    #[serde(rename = "c")]
    pub column: u32,
    #[serde(rename = "x")]
    pub source_x: u32,
    #[serde(rename = "y")]
    pub source_y: u32,
    #[serde(rename = "w")]
    pub source_width: u32,
    #[serde(rename = "h")]
    pub source_height: u32,
    #[serde(rename = "xo", default, skip_serializing_if = "is_zero_u32")]
    pub x_offset: u32,
    #[serde(rename = "yo", default, skip_serializing_if = "is_zero_u32")]
    pub y_offset: u32,
    #[serde(rename = "cs")]
    pub columns: u32,
    #[serde(rename = "rs")]
    pub rows: u32,
    #[serde(rename = "dw")]
    pub destination_width: u32,
    #[serde(rename = "dh")]
    pub destination_height: u32,
    #[serde(rename = "z", default, skip_serializing_if = "is_zero_i32")]
    pub z: i32,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    #[serde(rename = "id")]
    pub session_id: SessionId,
    #[serde(rename = "s")]
    pub size: TerminalSize,
    #[serde(rename = "c")]
    pub cells: Vec<TerminalCell>,
    #[serde(rename = "o", default, skip_serializing_if = "Vec::is_empty")]
    pub color_overrides: Vec<TerminalColorOverride>,
    #[serde(rename = "cl")]
    pub cursor_line: u16,
    #[serde(rename = "cc")]
    pub cursor_column: u16,
    #[serde(rename = "cs")]
    pub cursor_shape: TerminalCursorShape,
    #[serde(rename = "cw")]
    pub cursor_width: u8,
    #[serde(rename = "cb", default, skip_serializing_if = "is_false")]
    pub cursor_blinking: bool,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "r")]
    pub revision: u64,
    #[serde(rename = "is", default, skip_serializing_if = "is_zero_u64")]
    pub last_input_sequence: u64,
    #[serde(rename = "im", default, skip_serializing_if = "is_default_input_modes")]
    pub input_modes: TerminalInputModes,
    #[serde(rename = "gi", default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<TerminalImageDescriptor>,
    #[serde(rename = "gp", default, skip_serializing_if = "Vec::is_empty")]
    pub image_placements: Vec<TerminalImagePlacement>,
    #[serde(rename = "sel", default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TerminalSelectionRange>,
    #[serde(rename = "dl", default, skip_serializing_if = "Vec::is_empty")]
    pub detected_links: Vec<TerminalLinkRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TerminalCellPosition {
    #[serde(rename = "l")]
    pub line: u16,
    #[serde(rename = "c")]
    pub column: u16,
}

/// Direction to advance when moving between terminal search matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSearchDirection {
    /// Toward the end of the buffer (later output).
    #[default]
    Forward,
    /// Toward the start of the buffer (earlier output / scrollback).
    Backward,
}

/// A terminal search query issued by the client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSearchRequest {
    /// The query text. Interpreted literally unless [`Self::regex`] is set.
    pub query: String,
    /// When true, `query` is a regular expression; otherwise it is matched literally.
    #[serde(default)]
    pub regex: bool,
    /// Which way to move from the current active match to select the next one.
    #[serde(default)]
    pub direction: TerminalSearchDirection,
    /// When true this is the first query of a session (typing a new pattern) and the search should
    /// start from the current viewport rather than advancing past the previous active match.
    #[serde(default)]
    pub fresh: bool,
}

/// A single terminal search match, in viewport-relative coordinates. Both endpoints are inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSearchMatch {
    #[serde(rename = "s")]
    pub start: TerminalCellPosition,
    #[serde(rename = "e")]
    pub end: TerminalCellPosition,
}

/// Result of a [`ClientRequest::TerminalSearch`]. When `active` is `None` the query did not match
/// anywhere in the buffer. `matches` lists every match currently visible in the viewport (which the
/// daemon has already scrolled so that `active` is on screen); `total` counts all matches across
/// the whole buffer and `index` is the 0-based position of the active match within that total.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSearchResult {
    #[serde(rename = "a", default, skip_serializing_if = "Option::is_none")]
    pub active: Option<TerminalSearchMatch>,
    #[serde(rename = "m", default, skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<TerminalSearchMatch>,
    #[serde(rename = "i", default)]
    pub index: usize,
    #[serde(rename = "n", default)]
    pub total: usize,
    /// Revision of the snapshot these coordinates apply to. The client should render the highlight
    /// against a snapshot with at least this revision.
    #[serde(rename = "r")]
    pub revision: u64,
}

/// Which edge of a cell a selection endpoint sits on. Mirrors the terminal core's `Side`; the client
/// derives it from the sub-cell x position of the mouse (left half vs right half).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSelectionSide {
    #[default]
    Left,
    Right,
}

/// The semantic kind of an interactive selection, chosen by click count. Mirrors the terminal core's
/// `SelectionType` (minus `Block`, which is reserved for a future rectangular-selection modifier).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSelectionKind {
    /// Character-granularity selection (single click + drag).
    #[default]
    Simple,
    /// Word/semantic selection (double click).
    Semantic,
    /// Whole-line selection (triple click).
    Lines,
    /// Rectangular/block selection.
    Block,
}

/// The active selection projected into the current viewport, in viewport-relative coordinates. Both
/// endpoints are inclusive. The daemon owns the authoritative selection (a terminal-core `Selection`
/// spanning scrollback); this is only its visible projection for the current snapshot. `None` in a
/// snapshot means the selection is empty or entirely scrolled out of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSelectionRange {
    #[serde(rename = "s")]
    pub start: TerminalCellPosition,
    #[serde(rename = "e")]
    pub end: TerminalCellPosition,
    #[serde(rename = "b", default, skip_serializing_if = "is_false")]
    pub is_block: bool,
}

/// A URL auto-detected in the terminal output, in viewport-relative coordinates. Both endpoints are
/// inclusive. A URL spanning multiple rows (soft-wrapped) is emitted as one range per row so the
/// client can draw and hit-test each row independently. Distinct from [`TerminalCell::hyperlink`]
/// (explicit OSC 8 links), which take priority when both cover the same cell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalLinkRange {
    #[serde(rename = "s")]
    pub start: TerminalCellPosition,
    #[serde(rename = "e")]
    pub end: TerminalCellPosition,
    #[serde(rename = "u")]
    pub url: String,
}


#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshotDelta {
    #[serde(rename = "id")]
    pub session_id: SessionId,
    #[serde(rename = "b")]
    pub base_revision: u64,
    #[serde(rename = "s")]
    pub size: TerminalSize,
    #[serde(rename = "c", default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<TerminalCell>,
    #[serde(rename = "x", default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<TerminalCellPosition>,
    #[serde(rename = "o", default, skip_serializing_if = "Vec::is_empty")]
    pub color_overrides: Vec<TerminalColorOverride>,
    #[serde(rename = "cl")]
    pub cursor_line: u16,
    #[serde(rename = "cc")]
    pub cursor_column: u16,
    #[serde(rename = "cs")]
    pub cursor_shape: TerminalCursorShape,
    #[serde(rename = "cw")]
    pub cursor_width: u8,
    #[serde(rename = "cb", default, skip_serializing_if = "is_false")]
    pub cursor_blinking: bool,
    #[serde(rename = "t")]
    pub title: String,
    #[serde(rename = "r")]
    pub revision: u64,
    #[serde(rename = "is", default, skip_serializing_if = "is_zero_u64")]
    pub last_input_sequence: u64,
    #[serde(rename = "im", default, skip_serializing_if = "is_default_input_modes")]
    pub input_modes: TerminalInputModes,
    #[serde(rename = "gi", default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<TerminalImageDescriptor>>,
    #[serde(rename = "gp", default, skip_serializing_if = "Option::is_none")]
    pub image_placements: Option<Vec<TerminalImagePlacement>>,
    #[serde(rename = "sel", default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<TerminalSelectionRange>,
    #[serde(rename = "dl", default, skip_serializing_if = "Vec::is_empty")]
    pub detected_links: Vec<TerminalLinkRange>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_default_input_modes(value: &TerminalInputModes) -> bool {
    *value == TerminalInputModes::default()
}

impl TerminalSnapshot {
    pub fn plain_lines(&self) -> Vec<String> {
        let mut rows = vec![vec![' '; self.size.columns as usize]; self.size.rows as usize];
        for cell in &self.cells {
            if let Some(character) = rows
                .get_mut(cell.line as usize)
                .and_then(|row| row.get_mut(cell.column as usize))
            {
                *character = cell.character;
            }
        }
        rows.into_iter()
            .map(|row| row.into_iter().collect::<String>().trim_end().to_owned())
            .collect()
    }

    pub fn apply_delta(&self, delta: &TerminalSnapshotDelta) -> Option<Self> {
        if self.session_id != delta.session_id
            || self.revision != delta.base_revision
            || self.size != delta.size
        {
            return None;
        }

        let mut replaced = delta
            .cells
            .iter()
            .map(|cell| TerminalCellPosition {
                line: cell.line,
                column: cell.column,
            })
            .chain(delta.cleared.iter().copied())
            .collect::<Vec<_>>();
        replaced.sort_unstable();
        replaced.dedup();
        let mut cells = Vec::with_capacity(
            self.cells
                .len()
                .saturating_add(delta.cells.len())
                .saturating_sub(delta.cleared.len()),
        );
        cells.extend(
            self.cells
                .iter()
                .filter(|cell| {
                    replaced
                        .binary_search(&TerminalCellPosition {
                            line: cell.line,
                            column: cell.column,
                        })
                        .is_err()
                })
                .cloned(),
        );
        cells.extend(delta.cells.iter().cloned());
        cells.sort_unstable_by_key(|cell| (cell.line, cell.column));

        Some(Self {
            session_id: delta.session_id,
            size: delta.size,
            cells,
            color_overrides: delta.color_overrides.clone(),
            cursor_line: delta.cursor_line,
            cursor_column: delta.cursor_column,
            cursor_shape: delta.cursor_shape,
            cursor_width: delta.cursor_width,
            cursor_blinking: delta.cursor_blinking,
            title: delta.title.clone(),
            revision: delta.revision,
            last_input_sequence: delta.last_input_sequence,
            input_modes: delta.input_modes,
            images: delta.images.clone().unwrap_or_else(|| self.images.clone()),
            image_placements: delta
                .image_placements
                .clone()
                .unwrap_or_else(|| self.image_placements.clone()),
            selection: delta.selection,
            detected_links: delta.detected_links.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    HandshakeAccepted {
        protocol_version: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        build_id: String,
    },
    SessionCreated {
        session: SessionSummary,
    },
    Sessions {
        sessions: Vec<SessionSummary>,
    },
    SessionInspection {
        inspection: SessionInspection,
    },
    Snapshot {
        snapshot: Arc<TerminalSnapshot>,
    },
    SnapshotDelta {
        delta: TerminalSnapshotDelta,
    },
    SnapshotUnchanged {
        revision: u64,
    },
    Progress {
        update: TerminalProgressUpdate,
    },
    ProgressUnchanged {
        session_id: SessionId,
        revision: u64,
    },
    OscEvents {
        update: TerminalOscEventUpdate,
    },
    OscEventsUnchanged {
        session_id: SessionId,
        revision: u64,
    },
    ShellIntegrationState {
        session_id: SessionId,
        state: TerminalShellIntegrationState,
    },
    SearchResult {
        session_id: SessionId,
        result: TerminalSearchResult,
    },
    /// Reply to [`ClientRequest::TerminalCopySelection`]. `text` is `None` when there is no active
    /// selection or it is empty.
    SelectionText {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    TerminalImage {
        key: TerminalImageKey,
        width: u32,
        height: u32,
        total_length: u32,
        offset: u32,
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    Ok,
    Error {
        message: String,
    },
}

mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::de::{SeqAccess, Visitor};
    use serde::{Deserializer, Serialize, Serializer};
    use std::fmt;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if serializer.is_human_readable() {
            serializer.serialize_str(&STANDARD.encode(bytes))
        } else {
            serde_bytes::Bytes::new(bytes).serialize(serializer)
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BytesVisitor;

        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("base64 text or a binary byte string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                STANDARD.decode(value).map_err(E::custom)
            }

            fn visit_borrowed_bytes<E>(self, value: &'de [u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(value.to_vec())
            }

            fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(value)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
                while let Some(byte) = sequence.next_element()? {
                    bytes.push(byte);
                }
                Ok(bytes)
            }
        }

        deserializer.deserialize_any(BytesVisitor)
    }
}

pub fn encode_line<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    Ok(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn request_round_trips_as_a_single_json_line() {
        for request in [
            ClientRequest::Input {
                session_id: Uuid::nil(),
                bytes: b"echo hello\r".to_vec(),
                sequence: 7,
            },
            ClientRequest::Paste {
                session_id: Uuid::nil(),
                text: "echo 界\n".to_owned(),
                sequence: 8,
            },
            ClientRequest::PasteClipboard {
                session_id: Uuid::nil(),
                selection: TerminalClipboardSelection::Clipboard,
                contents: vec![TerminalClipboardContent {
                    mime_type: "text/plain;charset=utf-8".to_owned(),
                    data: b"rich paste".to_vec(),
                }],
                sequence: 9,
            },
            ClientRequest::Mouse {
                session_id: Uuid::nil(),
                event: TerminalMouseEvent {
                    action: TerminalMouseAction::Press,
                    button: Some(TerminalMouseButton::Left),
                    position: TerminalMousePosition {
                        column: 4,
                        row: 2,
                        pixel_x: 0,
                        pixel_y: 0,
                    },
                    modifiers: TerminalModifiers {
                        shift: false,
                        alt: true,
                        control: false,
                    },
                },
            },
            ClientRequest::Scroll {
                session_id: Uuid::nil(),
                event: TerminalScrollEvent {
                    delta: TerminalScrollDelta {
                        x: 0,
                        y: TERMINAL_SCROLL_DELTA_SCALE,
                        unit: TerminalScrollUnit::Lines,
                    },
                    phase: TerminalScrollPhase::Moved,
                    position: TerminalMousePosition {
                        column: 4,
                        row: 2,
                        pixel_x: 0,
                        pixel_y: 0,
                    },
                    modifiers: TerminalModifiers::default(),
                },
            },
            ClientRequest::TerminalSearch {
                session_id: Uuid::nil(),
                request: TerminalSearchRequest {
                    query: "需要 find".to_owned(),
                    regex: true,
                    direction: TerminalSearchDirection::Backward,
                    fresh: true,
                },
            },
            ClientRequest::Focus {
                session_id: Uuid::nil(),
                focused: true,
            },
            ClientRequest::WaitForSnapshot {
                session_id: Uuid::nil(),
                after_revision: 42,
                timeout_ms: 50,
            },
            ClientRequest::WaitForProgress {
                session_id: Uuid::nil(),
                after_revision: 7,
                timeout_ms: 50,
            },
            ClientRequest::WaitForOscEvents {
                session_id: Uuid::nil(),
                after_revision: 3,
                timeout_ms: 50,
            },
            ClientRequest::GetShellIntegrationState {
                session_id: Uuid::nil(),
            },
            ClientRequest::CompleteClipboardRead {
                session_id: Uuid::nil(),
                request_id: 11,
                contents: vec![TerminalClipboardContent {
                    mime_type: "text/plain".to_owned(),
                    data: b"clipboard".to_vec(),
                }],
            },
            ClientRequest::NotificationResponse {
                session_id: Uuid::nil(),
                notification_id: "build-finished".to_owned(),
                activated: true,
            },
            ClientRequest::CompleteFileTransfer {
                session_id: Uuid::nil(),
                request_id: 12,
                destination: Some(PathBuf::from("/tmp/download")),
            },
            ClientRequest::SetProgressTimeouts {
                session_id: Uuid::nil(),
                timeouts: TerminalProgressTimeouts {
                    completed_ms: 5_000,
                    stale_ms: 60_000,
                },
            },
            ClientRequest::TerminalImage {
                session_id: Uuid::nil(),
                key: TerminalImageKey {
                    id: 7,
                    generation: 9,
                },
                offset: 4096,
                length: 1024,
            },
            ClientRequest::SetOscPolicy {
                session_id: Uuid::nil(),
                allow_clipboard_read: true,
            },
        ] {
            let encoded = encode_line(&request).unwrap();
            assert_eq!(encoded.last(), Some(&b'\n'));
            let decoded: ClientRequest = serde_json::from_slice(&encoded).unwrap();
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn terminal_search_request_omitted_fields_use_defaults() {
        // A request that specifies only the query must decode, defaulting regex/fresh to false and
        // direction to Forward, so partial or older-client payloads don't hard-fail.
        let decoded: TerminalSearchRequest = serde_json::from_str(r#"{"query":"foo"}"#).unwrap();
        assert_eq!(decoded.query, "foo");
        assert!(!decoded.regex);
        assert!(!decoded.fresh);
        assert_eq!(decoded.direction, TerminalSearchDirection::Forward);
    }

    #[test]
    fn terminal_selection_requests_and_reply_round_trip() {
        for request in [
            ClientRequest::TerminalSelectionStart {
                session_id: Uuid::nil(),
                point: TerminalCellPosition { line: 3, column: 7 },
                side: TerminalSelectionSide::Right,
                kind: TerminalSelectionKind::Semantic,
            },
            ClientRequest::TerminalSelectionUpdate {
                session_id: Uuid::nil(),
                point: TerminalCellPosition {
                    line: 12,
                    column: 0,
                },
                side: TerminalSelectionSide::Left,
            },
            ClientRequest::TerminalSelectionClear {
                session_id: Uuid::nil(),
            },
            ClientRequest::TerminalSelectAll {
                session_id: Uuid::nil(),
            },
            ClientRequest::TerminalCopySelection {
                session_id: Uuid::nil(),
            },
        ] {
            let encoded = encode_line(&request).unwrap();
            assert_eq!(
                serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
                request
            );
            assert_eq!(
                rmp_serde::from_slice::<ClientRequest>(
                    &rmp_serde::to_vec_named(&request).unwrap()
                )
                .unwrap(),
                request
            );
        }

        let reply = DaemonResponse::SelectionText {
            session_id: Uuid::nil(),
            text: Some("hello\nworld".to_owned()),
        };
        let encoded = encode_line(&reply).unwrap();
        assert_eq!(
            serde_json::from_slice::<DaemonResponse>(&encoded).unwrap(),
            reply
        );
    }

    #[test]
    fn terminal_scroll_to_commands_round_trip() {
        for command in [
            TerminalScrollCommand::Top,
            TerminalScrollCommand::Bottom,
            TerminalScrollCommand::PageUp,
            TerminalScrollCommand::PageDown,
            TerminalScrollCommand::PrevPrompt,
            TerminalScrollCommand::NextPrompt,
        ] {
            let request = ClientRequest::TerminalScrollTo {
                session_id: Uuid::nil(),
                command,
            };
            let encoded = encode_line(&request).unwrap();
            assert_eq!(
                serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
                request
            );
            assert_eq!(
                rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                    .unwrap(),
                request
            );
        }
    }

    #[test]
    fn set_cursor_style_request_round_trips() {
        for shape in [
            TerminalCursorShape::Block,
            TerminalCursorShape::Underline,
            TerminalCursorShape::Beam,
            TerminalCursorShape::HollowBlock,
            TerminalCursorShape::Hidden,
        ] {
            let request = ClientRequest::SetCursorStyle {
                session_id: Uuid::nil(),
                shape,
            };
            let encoded = encode_line(&request).unwrap();
            assert_eq!(
                serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
                request
            );
            assert_eq!(
                rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                    .unwrap(),
                request
            );
        }
    }

    #[test]
    fn set_scrollback_limit_request_round_trips() {
        for limit in [0usize, 500, 10_000, 1_000_000] {
            let request = ClientRequest::SetScrollbackLimit {
                session_id: Uuid::nil(),
                limit,
            };
            let encoded = encode_line(&request).unwrap();
            assert_eq!(
                serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
                request
            );
            assert_eq!(
                rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                    .unwrap(),
                request
            );
        }
    }

    #[test]
    fn create_session_carries_scrollback_and_shell_overrides() {
        let request = ClientRequest::CreateSession {
            project_id: Uuid::nil(),
            cwd: PathBuf::from("/tmp"),
            size: TerminalSize::default(),
            appearance: TerminalAppearance::default(),
            scrollback_limit: 500,
            shell_program: "/opt/homebrew/bin/fish".to_owned(),
            shell_args: vec!["-l".to_owned(), "-c".to_owned(), "echo hi".to_owned()],
        };
        let encoded = encode_line(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<ClientRequest>(&encoded).unwrap(),
            request
        );
        assert_eq!(
            rmp_serde::from_slice::<ClientRequest>(&rmp_serde::to_vec_named(&request).unwrap())
                .unwrap(),
            request
        );
    }

    #[test]
    fn create_session_shell_overrides_default_to_empty_when_absent() {
        // A request omitting the new fields (older client shape) decodes with empty/zero defaults.
        let appearance = serde_json::to_string(&TerminalAppearance::default()).unwrap();
        let json = format!(
            r#"{{"type":"create_session","project_id":"00000000-0000-0000-0000-000000000000","cwd":"/tmp","size":{{"columns":80,"rows":24,"cell_width":8,"cell_height":16}},"appearance":{appearance}}}"#
        );
        let decoded: ClientRequest = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientRequest::CreateSession {
                scrollback_limit,
                shell_program,
                shell_args,
                ..
            } => {
                assert_eq!(scrollback_limit, 0);
                assert!(shell_program.is_empty());
                assert!(shell_args.is_empty());
            }
            other => panic!("unexpected decode: {other:?}"),
        }
    }

    #[test]
    fn bell_osc_event_round_trips() {
        let payload = TerminalOscEventPayload::Bell;
        let json = serde_json::to_string(&payload).unwrap();
        assert_eq!(json, r#"{"type":"bell"}"#);
        assert_eq!(
            serde_json::from_str::<TerminalOscEventPayload>(&json).unwrap(),
            payload
        );
        assert_eq!(
            rmp_serde::from_slice::<TerminalOscEventPayload>(
                &rmp_serde::to_vec_named(&payload).unwrap()
            )
            .unwrap(),
            payload
        );
    }

    #[test]
    fn selection_side_and_kind_default_and_omitted_block_flag() {
        assert_eq!(TerminalSelectionSide::default(), TerminalSelectionSide::Left);
        assert_eq!(
            TerminalSelectionKind::default(),
            TerminalSelectionKind::Simple
        );
        // A range without the block flag omits "b" on the wire and decodes back to false.
        let range = TerminalSelectionRange {
            start: TerminalCellPosition { line: 0, column: 0 },
            end: TerminalCellPosition { line: 1, column: 4 },
            is_block: false,
        };
        let json = serde_json::to_string(&range).unwrap();
        assert!(!json.contains("\"b\""));
        assert_eq!(
            serde_json::from_str::<TerminalSelectionRange>(&json).unwrap(),
            range
        );
    }

    #[test]
    fn snapshot_without_selection_key_decodes_to_none() {
        // An older daemon's snapshot has no "sel" key; it must decode with selection == None so a
        // version-skewed client does not hard-fail.
        let json = r#"{"id":"00000000-0000-0000-0000-000000000000","s":{"columns":80,"rows":24,"cell_width":8,"cell_height":16},"c":[],"cl":0,"cc":0,"cs":"block","cw":1,"t":"","r":1}"#;
        let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.selection, None);
        assert!(snapshot.detected_links.is_empty());
    }

    #[test]
    fn detected_links_round_trip_and_default_empty() {
        let link = TerminalLinkRange {
            start: TerminalCellPosition { line: 2, column: 6 },
            end: TerminalCellPosition {
                line: 2,
                column: 24,
            },
            url: "https://example.com/x".to_owned(),
        };
        let encoded = encode_line(&link).unwrap();
        assert_eq!(
            serde_json::from_slice::<TerminalLinkRange>(&encoded).unwrap(),
            link
        );
        // A snapshot without "dl" decodes to an empty link list.
        let json = r#"{"id":"00000000-0000-0000-0000-000000000000","s":{"columns":80,"rows":24,"cell_width":8,"cell_height":16},"c":[],"cl":0,"cc":0,"cs":"block","cw":1,"t":"","r":1}"#;
        let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
        assert!(snapshot.detected_links.is_empty());
    }

    #[test]
    fn terminal_progress_update_round_trips() {
        let response = DaemonResponse::Progress {
            update: TerminalProgressUpdate {
                session_id: Uuid::nil(),
                revision: 9,
                progress: Some(TerminalProgress {
                    state: TerminalProgressState::Paused,
                    percent: Some(73),
                    updated_at_unix_ms: 1_725_000_000_123,
                }),
            },
        };
        let encoded = encode_line(&response).unwrap();
        assert_eq!(
            serde_json::from_slice::<DaemonResponse>(&encoded).unwrap(),
            response
        );
        assert_eq!(
            rmp_serde::from_slice::<DaemonResponse>(&rmp_serde::to_vec_named(&response).unwrap())
                .unwrap(),
            response
        );
    }

    #[test]
    fn terminal_image_chunks_use_compact_base64_json_and_round_trip() {
        let response = DaemonResponse::TerminalImage {
            key: TerminalImageKey {
                id: 7,
                generation: 9,
            },
            width: 2,
            height: 1,
            total_length: 8,
            offset: 0,
            bytes: vec![0, 1, 2, 3, 252, 253, 254, 255],
        };
        let encoded = encode_line(&response).unwrap();
        let json = std::str::from_utf8(&encoded).unwrap();
        assert!(json.contains(r#""bytes":"AAECA/z9/v8=""#));
        assert!(!json.contains(r#""bytes":["#));
        assert_eq!(
            serde_json::from_slice::<DaemonResponse>(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn terminal_image_chunks_are_raw_binary_on_the_daemon_wire() {
        let bytes = vec![0x5a; 1024 * 1024];
        let response = DaemonResponse::TerminalImage {
            key: TerminalImageKey {
                id: 7,
                generation: 9,
            },
            width: 512,
            height: 512,
            total_length: bytes.len() as u32,
            offset: 0,
            bytes,
        };
        let encoded = rmp_serde::to_vec_named(&response).unwrap();
        assert!(
            encoded.len() < 1024 * 1024 + 256,
            "image chunk unexpectedly expanded to {} bytes",
            encoded.len()
        );
        assert_eq!(
            rmp_serde::from_slice::<DaemonResponse>(&encoded).unwrap(),
            response
        );
    }

    #[test]
    fn process_metrics_are_additive_for_running_daemon_compatibility() {
        let process: ProcessInfo =
            serde_json::from_str(r#"{"pid":42,"parent_pid":1,"name":"zsh"}"#).unwrap();

        assert_eq!(process.cpu_usage_tenths_percent, None);
        assert_eq!(process.memory_bytes, None);
    }

    #[test]
    fn handshake_without_a_build_id_is_treated_as_a_stale_daemon() {
        let response: DaemonResponse =
            serde_json::from_str(r#"{"type":"handshake_accepted","protocol_version":9}"#).unwrap();
        assert_eq!(
            response,
            DaemonResponse::HandshakeAccepted {
                protocol_version: 9,
                build_id: String::new(),
            }
        );
    }

    #[test]
    fn render_snapshot_round_trips_without_losing_terminal_semantics() {
        let snapshot = TerminalSnapshot {
            session_id: Uuid::nil(),
            size: TerminalSize {
                columns: 2,
                rows: 1,
                ..TerminalSize::default()
            },
            cells: vec![TerminalCell {
                line: 0,
                column: 0,
                character: 'e',
                zerowidth: vec!['\u{301}'],
                foreground: TerminalColor::Indexed(1),
                background: TerminalColor::Named(257),
                underline_color: Some(TerminalColor::Rgb(0x112233ff)),
                hyperlink: Some("https://example.com".to_owned()),
                flags: 0x4a0a,
            }],
            color_overrides: vec![TerminalColorOverride {
                index: 1,
                color: 0x123456ff,
            }],
            cursor_line: 0,
            cursor_column: 0,
            cursor_shape: TerminalCursorShape::Beam,
            cursor_width: 1,
            cursor_blinking: true,
            title: "semantic snapshot".to_owned(),
            revision: 42,
            last_input_sequence: 9,
            input_modes: TerminalInputModes {
                mouse_tracking: TerminalMouseTracking::Drag,
                mouse_encoding: TerminalMouseEncoding::Sgr,
                focus_reporting: true,
                alternate_screen: true,
                alternate_scroll: true,
                paste_events: false,
                kitty_keyboard_flags: 0,
            },
            images: vec![TerminalImageDescriptor {
                key: TerminalImageKey {
                    id: 7,
                    generation: 9,
                },
                width: 2,
                height: 1,
            }],
            image_placements: vec![TerminalImagePlacement {
                image: TerminalImageKey {
                    id: 7,
                    generation: 9,
                },
                placement_id: 3,
                line: 0,
                column: 1,
                source_x: 0,
                source_y: 0,
                source_width: 2,
                source_height: 1,
                x_offset: 0,
                y_offset: 0,
                columns: 1,
                rows: 1,
                destination_width: 8,
                destination_height: 4,
                z: -1,
            }],
            selection: None,
            detected_links: Vec::new(),
        };
        let encoded = encode_line(&DaemonResponse::Snapshot {
            snapshot: Arc::new(snapshot.clone()),
        })
        .unwrap();
        let decoded: DaemonResponse = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(
            decoded,
            DaemonResponse::Snapshot {
                snapshot: Arc::new(snapshot)
            }
        );
    }

    #[test]
    fn snapshot_cursor_blinking_defaults_to_false_when_absent() {
        // A snapshot serialized before the field existed omits `cb`; it must decode as not-blinking.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000000",
            "s": {"columns": 1, "rows": 1, "cell_width": 8, "cell_height": 18},
            "c": [],
            "cl": 0,
            "cc": 0,
            "cs": "block",
            "cw": 1,
            "t": "",
            "r": 1
        }"#;
        let snapshot: TerminalSnapshot = serde_json::from_str(json).unwrap();
        assert!(!snapshot.cursor_blinking);

        // And when present it round-trips.
        let blinking = TerminalSnapshot {
            cursor_blinking: true,
            ..snapshot
        };
        let encoded = encode_line(&blinking).unwrap();
        assert!(
            serde_json::from_slice::<TerminalSnapshot>(&encoded)
                .unwrap()
                .cursor_blinking
        );
    }

    #[test]
    fn snapshot_delta_replaces_and_clears_cells_without_losing_metadata() {
        let snapshot = TerminalSnapshot {
            session_id: Uuid::nil(),
            size: TerminalSize {
                columns: 3,
                rows: 1,
                ..TerminalSize::default()
            },
            cells: vec![
                TerminalCell {
                    line: 0,
                    column: 0,
                    character: 'a',
                    zerowidth: Vec::new(),
                    foreground: TerminalColor::Named(256),
                    background: TerminalColor::Named(257),
                    underline_color: None,
                    hyperlink: None,
                    flags: 0,
                },
                TerminalCell {
                    line: 0,
                    column: 1,
                    character: 'b',
                    zerowidth: Vec::new(),
                    foreground: TerminalColor::Named(256),
                    background: TerminalColor::Named(257),
                    underline_color: None,
                    hyperlink: None,
                    flags: 0,
                },
            ],
            color_overrides: Vec::new(),
            cursor_line: 0,
            cursor_column: 1,
            cursor_shape: TerminalCursorShape::Block,
            cursor_width: 1,
            cursor_blinking: false,
            title: "before".to_owned(),
            revision: 4,
            last_input_sequence: 1,
            input_modes: TerminalInputModes::default(),
            images: vec![TerminalImageDescriptor {
                key: TerminalImageKey {
                    id: 12,
                    generation: 13,
                },
                width: 64,
                height: 32,
            }],
            image_placements: vec![TerminalImagePlacement {
                image: TerminalImageKey {
                    id: 12,
                    generation: 13,
                },
                placement_id: 4,
                line: -1,
                column: 2,
                source_x: 1,
                source_y: 2,
                source_width: 32,
                source_height: 16,
                x_offset: 3,
                y_offset: 4,
                columns: 4,
                rows: 2,
                destination_width: 32,
                destination_height: 36,
                z: 1,
            }],
            selection: None,
            detected_links: Vec::new(),
        };
        let replacement = TerminalCell {
            line: 0,
            column: 1,
            character: 'x',
            zerowidth: vec!['\u{301}'],
            foreground: TerminalColor::Indexed(2),
            background: TerminalColor::Named(257),
            underline_color: None,
            hyperlink: Some("https://example.com/replaced".to_owned()),
            flags: 4,
        };
        let delta = TerminalSnapshotDelta {
            session_id: Uuid::nil(),
            base_revision: 4,
            size: snapshot.size,
            cells: vec![replacement.clone()],
            cleared: vec![TerminalCellPosition { line: 0, column: 0 }],
            color_overrides: vec![TerminalColorOverride {
                index: 2,
                color: 0xabcd_efff,
            }],
            cursor_line: 0,
            cursor_column: 2,
            cursor_shape: TerminalCursorShape::Beam,
            cursor_width: 1,
            cursor_blinking: false,
            title: "after".to_owned(),
            revision: 5,
            last_input_sequence: 2,
            input_modes: TerminalInputModes {
                focus_reporting: true,
                ..TerminalInputModes::default()
            },
            images: Some(Vec::new()),
            image_placements: Some(Vec::new()),
            selection: None,
            detected_links: Vec::new(),
        };

        let updated = snapshot.apply_delta(&delta).unwrap();
        assert_eq!(updated.cells, vec![replacement]);
        assert_eq!(updated.title, "after");
        assert_eq!(updated.revision, 5);
        assert_eq!(updated.last_input_sequence, 2);
        assert!(updated.input_modes.focus_reporting);
        assert_eq!(updated.color_overrides, delta.color_overrides);
        assert_eq!(updated.images, delta.images.clone().unwrap());
        assert_eq!(
            updated.image_placements,
            delta.image_placements.clone().unwrap()
        );
        let mut wrong_base = delta;
        wrong_base.base_revision = 3;
        assert!(snapshot.apply_delta(&wrong_base).is_none());
    }

    #[test]
    fn default_appearance_resolves_the_complete_alacritty_palette() {
        let appearance = TerminalAppearance::default();
        assert_eq!(appearance.color(0), Some(appearance.palette[0]));
        assert_eq!(appearance.color(16), Some(0x000000));
        assert_eq!(appearance.color(231), Some(0xffffff));
        assert_eq!(appearance.color(232), Some(0x080808));
        assert_eq!(appearance.color(255), Some(0xeeeeee));
        assert_eq!(appearance.color(256), Some(appearance.foreground));
        assert_eq!(appearance.color(257), Some(appearance.background));
        assert_eq!(appearance.color(258), Some(appearance.cursor));
        assert_eq!(appearance.color(268), Some(dim_rgb(appearance.foreground)));
        assert_eq!(dim_rgb(0x5f5f5f), 0x3e3e3e);
        assert_eq!(appearance.color(269), None);
    }
}

use super::*;

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

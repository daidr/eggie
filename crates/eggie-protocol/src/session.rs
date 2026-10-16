use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSummary {
    pub pid: u32,
    pub name: String,
    /// Executable and arguments, used to distinguish commands such as `git diff`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line: Option<Vec<String>>,
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
    pub port_mappings: Option<PortMappingsSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalProgramBlockKind {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalProgramStatus {
    pub state: TerminalProgramState,
    pub kind: Option<TerminalProgramBlockKind>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub progress: Option<u8>,
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
    /// OSC 133 command currently running, when shell integration reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_command: Option<String>,
    /// Most relevant OSC 7501 record for this terminal, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program_status: Option<TerminalProgramStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteSessionInfo>,
    pub status: SessionStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteSessionPhase {
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Exited,
}

impl RemoteSessionPhase {
    pub fn from_process(status: &SessionStatus, ready: bool) -> Self {
        match status {
            SessionStatus::Running if ready => Self::Connected,
            SessionStatus::Running => Self::Connecting,
            SessionStatus::Exited { code: Some(0) } if ready => Self::Exited,
            SessionStatus::Exited {
                code: Some(255) | None,
            } if ready => Self::Disconnected,
            SessionStatus::Exited { .. } => Self::Failed,
        }
    }

    pub fn needs_recovery(self) -> bool {
        matches!(self, Self::Disconnected | Self::Failed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSessionInfo {
    pub target: eggie_domain::RemoteTarget,
    pub phase: RemoteSessionPhase,
    pub initial_directory: Option<String>,
}

impl SessionSummary {
    pub fn remote_directory(&self) -> Option<&str> {
        self.remote.as_ref()?;
        self.reported_location
            .as_ref()
            .filter(|location| !location.local && !location.path.is_empty())
            .map(|location| location.path.as_str())
            .or_else(|| self.remote.as_ref()?.initial_directory.as_deref())
    }

    pub fn needs_remote_recovery(&self) -> bool {
        self.remote
            .as_ref()
            .is_some_and(|remote| remote.phase.needs_recovery())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Exited { code: Option<i32> },
}

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
    pub status: SessionStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Exited { code: Option<i32> },
}

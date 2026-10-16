//! Shared local-control contract. CLI and MCP use the same methods and validation.
use super::*;
use eggie_domain::{Direction, GroupId, ProfileId, WindowId};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum AutomationRequest {
    #[serde(rename = "projects.list")]
    ProjectsList {},
    #[serde(rename = "projects.create")]
    ProjectsCreate { name: String, root: Option<PathBuf> },
    #[serde(rename = "projects.update")]
    ProjectsUpdate {
        project_id: ProjectId,
        name: Option<String>,
        root: Option<PathBuf>,
        #[serde(default)]
        clear_root: bool,
    },
    #[serde(rename = "projects.delete")]
    ProjectsDelete { project_id: ProjectId },
    #[serde(rename = "windows.list")]
    WindowsList {},
    #[serde(rename = "profiles.list")]
    ProfilesList {},
    #[serde(rename = "tabs.list")]
    TabsList { project_id: Option<ProjectId> },
    #[serde(rename = "tabs.create")]
    TabsCreate {
        project_id: Option<ProjectId>,
        window_id: Option<WindowId>,
        profile_id: Option<ProfileId>,
        cwd: Option<PathBuf>,
        title: Option<String>,
    },
    #[serde(rename = "tabs.focus")]
    TabsFocus { session_id: SessionId },
    #[serde(rename = "tabs.rename")]
    TabsRename {
        session_id: SessionId,
        title: String,
    },
    #[serde(rename = "tabs.close")]
    TabsClose {
        session_id: SessionId,
        #[serde(default)]
        force: bool,
    },
    #[serde(rename = "tabs.move")]
    TabsMove {
        session_id: SessionId,
        window_id: WindowId,
        group_id: Option<GroupId>,
        index: Option<usize>,
    },
    #[serde(rename = "panes.split")]
    PanesSplit {
        session_id: SessionId,
        direction: Direction,
        profile_id: Option<ProfileId>,
    },
    #[serde(rename = "terminal.read")]
    TerminalRead {
        session_id: SessionId,
        #[serde(default = "default_tail")]
        tail_lines: u32,
    },
    #[serde(rename = "terminal.send")]
    TerminalSend {
        session_id: SessionId,
        text: String,
        #[serde(default)]
        submit: bool,
    },
    #[serde(rename = "commands.list")]
    CommandsList { session_id: SessionId },
    #[serde(rename = "commands.output")]
    CommandsOutput {
        session_id: SessionId,
        command_id: u64,
        #[serde(default = "default_output_limit")]
        max_bytes: u32,
    },
    #[serde(rename = "commands.locate")]
    CommandsLocate {
        session_id: SessionId,
        command_id: u64,
    },
}
fn default_tail() -> u32 {
    200
}
fn default_output_limit() -> u32 {
    1024 * 1024
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AutomationResponse {
    Ok { result: Value },
    Error { code: String, message: String },
}
impl AutomationResponse {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub struct AutomationMethod {
    pub name: &'static str,
    pub description: &'static str,
    pub read_only: bool,
    pub destructive: bool,
    pub schema: Value,
}

/// Keep method discovery identical in `eggie api --help` and MCP tools/list.
pub fn automation_methods() -> Vec<AutomationMethod> {
    let uuid = json!({"type":"string", "format":"uuid"});
    let text = json!({"type":"string"});
    let flag = json!({"type":"boolean", "default":false});
    let mut methods = Vec::new();
    macro_rules! method {
        ($name:literal, $description:literal, $read:literal, $destructive:literal, $properties:expr, $required:expr) => {
            methods.push(AutomationMethod { name: $name, description: $description, read_only: $read, destructive: $destructive,
                schema: json!({"type":"object", "properties":$properties, "required":$required, "additionalProperties":false}) });
        }
    }
    method!(
        "projects.list",
        "List projects with stable IDs and roots",
        true,
        false,
        json!({}),
        json!([])
    );
    method!(
        "projects.create",
        "Create a project; root must be an existing absolute local directory",
        false,
        false,
        json!({"name":text,"root":text}),
        json!(["name"])
    );
    method!(
        "projects.update",
        "Rename a project or change its root; clear_root removes the root",
        false,
        false,
        json!({"project_id":uuid,"name":text,"root":text,"clear_root":flag}),
        json!(["project_id"])
    );
    method!(
        "projects.delete",
        "Delete an empty project; close its terminal tabs first",
        false,
        true,
        json!({"project_id":uuid}),
        json!(["project_id"])
    );
    method!(
        "windows.list",
        "List windows, pane layouts and active project IDs",
        true,
        false,
        json!({}),
        json!([])
    );
    method!(
        "profiles.list",
        "List profile IDs, names and remote targets; environment values are omitted",
        true,
        false,
        json!({}),
        json!([])
    );
    method!(
        "tabs.list",
        "List terminal tabs, including background tabs, with session and placement IDs",
        true,
        false,
        json!({"project_id":uuid}),
        json!([])
    );
    method!(
        "tabs.create",
        "Create a terminal in an existing window; omit IDs to use the active window/project",
        false,
        false,
        json!({"project_id":uuid,"window_id":uuid,"profile_id":uuid,"cwd":text,"title":text}),
        json!([])
    );
    method!(
        "tabs.focus",
        "Focus a terminal tab; restore a background tab into the active window",
        false,
        false,
        json!({"session_id":uuid}),
        json!(["session_id"])
    );
    method!(
        "tabs.rename",
        "Set a tab title; an empty title restores the shell title",
        false,
        false,
        json!({"session_id":uuid,"title":text}),
        json!(["session_id", "title"])
    );
    method!(
        "tabs.close",
        "Close a terminal; force is required for a running foreground command",
        false,
        true,
        json!({"session_id":uuid,"force":flag}),
        json!(["session_id"])
    );
    method!(
        "tabs.move",
        "Move/reorder a tab in a window, preserving its process and project",
        false,
        false,
        json!({"session_id":uuid,"window_id":uuid,"group_id":uuid,"index":{"type":"integer","minimum":0}}),
        json!(["session_id", "window_id"])
    );
    method!(
        "panes.split",
        "Create a terminal beside an existing tab in a new pane",
        false,
        false,
        json!({"session_id":uuid,"profile_id":uuid,"direction":{"enum":["left","right","up","down"]}}),
        json!(["session_id", "direction"])
    );
    method!(
        "terminal.read",
        "Read terminal text from its existing scrollback, bounded to 1 MiB",
        true,
        false,
        json!({"session_id":uuid,"tail_lines":{"type":"integer","minimum":1,"maximum":10000,"default":200}}),
        json!(["session_id"])
    );
    method!(
        "terminal.send",
        "Paste literal text; only submit=true sends Enter. Does not interpret key escape sequences",
        false,
        true,
        json!({"session_id":uuid,"text":text,"submit":flag}),
        json!(["session_id", "text"])
    );
    method!(
        "commands.list",
        "Read shell-integrated command history and the running command",
        true,
        false,
        json!({"session_id":uuid}),
        json!(["session_id"])
    );
    method!(
        "commands.output",
        "Read one command's output; fails once its scrollback was evicted",
        true,
        false,
        json!({"session_id":uuid,"command_id":{"type":"integer","minimum":1},"max_bytes":{"type":"integer","minimum":1,"maximum":1048576,"default":1048576}}),
        json!(["session_id", "command_id"])
    );
    method!(
        "commands.locate",
        "Scroll a terminal viewport to a command's output",
        false,
        false,
        json!({"session_id":uuid,"command_id":{"type":"integer","minimum":1}}),
        json!(["session_id", "command_id"])
    );
    methods
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requests_reject_typos_and_preserve_safe_defaults() {
        let id = SessionId::new_v4();
        let request: AutomationRequest = serde_json::from_value(
            json!({"method":"terminal.send","params":{"session_id":id,"text":"hi"}}),
        )
        .unwrap();
        assert!(matches!(
            request,
            AutomationRequest::TerminalSend { submit: false, .. }
        ));
        assert!(serde_json::from_value::<AutomationRequest>(json!({"method":"terminal.send","params":{"session_id":id,"text":"hi","sumbit":true}})).is_err());
        for method in automation_methods() {
            assert_eq!(method.schema["additionalProperties"], false);
        }
    }
}

//! Serializable configuration contracts shared by clients and the daemon.
pub mod font;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable action names accepted by the configuration service. The UI has the
/// constructors for these actions; the service only needs their identities.
pub const BUILTIN_ACTIONS: &[&str] = &[
    "terminal_switcher",
    "window_switcher",
    "undo_window_operation",
    "pane.split",
    "tab.new",
    "open_settings",
    "new_window",
    "quit",
    "terminal_copy",
    "terminal_paste",
    "terminal_select_all",
    "terminal_find",
    "terminal_suggest",
    "find_next",
    "find_previous",
    "new_tab",
    "close_tab",
    "close_window",
    "next_tab",
    "prev_tab",
    "split_right",
    "split_down",
    "clear_screen",
    "scroll_top",
    "scroll_bottom",
    "page_up",
    "page_down",
    "font_increase",
    "font_decrease",
    "font_reset",
    "jump_prev_prompt",
    "jump_next_prompt",
    "minimize_window",
    "toggle_full_screen",
    "command_palette",
    "toggle_left_sidebar",
    "toggle_right_sidebar",
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RegisteredConfig {
    pub ui_light_theme: Option<String>,
    pub ui_dark_theme: Option<String>,
    pub terminal_light_theme: Option<String>,
    pub terminal_dark_theme: Option<String>,
    pub terminal_font: Option<FontSpec>,
    pub bindings: Vec<BindingOp>,
    pub actions: Vec<ActionSpec>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontSpec {
    pub family: String,
    pub size: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BindingOp {
    Bind {
        key: String,
        action: String,
        args: serde_json::Value,
    },
    Unbind {
        key: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub name: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub revision: u64,
    /// Advances on both successful reloads and failed attempts. The effective
    /// configuration revision stays unchanged when validation fails.
    pub status_revision: u64,
    pub path: PathBuf,
    pub exists: bool,
    pub error: Option<String>,
    pub registered: RegisteredConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PreferencesSnapshot {
    pub revision: u64,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionContext {
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
    pub window_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedAction {
    pub name: String,
    pub args: serde_json::Value,
}

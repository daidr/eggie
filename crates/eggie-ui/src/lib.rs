mod actions;
mod app;
mod command_history;
mod command_palette;
use eggie_ui_kit::continuous_corners;
mod gui_ipc;
mod i18n;
use eggie_terminal_view::input_latency;
use eggie_ui_kit::icons;
mod keybindings;
mod markdown;
mod native_menu;
mod notes_store;
use eggie_ui_kit::popup_window;
mod process_icons;
mod profile_editor;
mod profile_store;
mod project_drag;
mod project_store;
pub mod services;
mod settings;
mod settings_window;
mod snippet_store;
mod system_menu;
use eggie_terminal_view::terminal_renderer;
mod text_input;
mod update_controller;
mod update_window;
mod workspace;

/// Launch the GPUI application with an established daemon connection.
pub fn launch(project_root: std::path::PathBuf, client: eggie_ipc::DaemonClient) {
    app::EggieApp::launch(project_root, client);
}
pub use gui_ipc::try_wake_existing;

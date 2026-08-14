mod app;
mod cli;
mod command_history;
mod command_palette;
mod gui_ipc;
mod i18n;
mod icons;
mod input_latency;
mod keybindings;
mod markdown;
#[cfg(target_os = "macos")]
mod metal_terminal;
mod native_menu;
mod notes_store;
mod project_store;
mod snippet_store;
pub mod services;
mod settings;
mod settings_window;
mod system_menu;
mod terminal_renderer;
mod terminal_sprites;
mod text_input;
mod update_controller;
mod update_window;

use anyhow::{Context, Result};
use eggie_daemon::{DaemonClient, is_daemon_invocation, run_daemon};

const BUILD_ID: &str = env!("EGGIE_BUILD_ID");

fn main() -> Result<()> {
    let arguments = std::env::args().collect::<Vec<_>>();
    if let Some(socket_path) = is_daemon_invocation(&arguments) {
        return run_daemon(&socket_path, BUILD_ID);
    }

    // 内置 CLI(`eggie +action` / `--version` / `-h`)。命中则执行并退出,
    // 不进入 GUI 启动路径。
    if let Some(exit_code) = cli::try_run_cli(&arguments) {
        std::process::exit(exit_code);
    }

    let project_root = std::env::current_dir().context("failed to determine current directory")?;
    // Single instance: if a GUI is already running, ask it to open a window for this directory and
    // exit, instead of starting a second GUI process. Falls through to launching our own GUI when
    // there is no running instance to wake.
    if gui_ipc::try_wake_existing(project_root.clone()) {
        return Ok(());
    }
    let client = DaemonClient::connect_default(BUILD_ID)?;
    app::EggieApp::launch(project_root, client);
    Ok(())
}

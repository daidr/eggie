mod app;
mod icons;
mod input_latency;
#[cfg(target_os = "macos")]
mod metal_terminal;
mod native_menu;
pub mod services;
mod settings;
mod settings_window;
mod terminal_renderer;
mod terminal_sprites;

use anyhow::{Context, Result};
use eggie_daemon::{DaemonClient, is_daemon_invocation, run_daemon};

const BUILD_ID: &str = env!("EGGIE_BUILD_ID");

fn main() -> Result<()> {
    let arguments = std::env::args().collect::<Vec<_>>();
    if let Some(socket_path) = is_daemon_invocation(&arguments) {
        return run_daemon(&socket_path, BUILD_ID);
    }

    let project_root = std::env::current_dir().context("failed to determine current directory")?;
    let client = DaemonClient::connect_default(BUILD_ID)?;
    app::EggieApp::launch(project_root, client);
    Ok(())
}

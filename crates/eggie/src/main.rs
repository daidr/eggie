mod cli;
use anyhow::{Context, Result};
use eggie_build_info::BUILD_ID;

fn main() -> Result<()> {
    let arguments = std::env::args().collect::<Vec<_>>();
    if let Some(socket_path) = eggie_ipc::is_daemon_invocation(&arguments) {
        return eggie_daemon::run_daemon(&socket_path, BUILD_ID);
    }
    if let Some(exit_code) = cli::try_run_cli(&arguments) {
        std::process::exit(exit_code);
    }
    let project_root = std::env::current_dir().context("failed to determine current directory")?;
    if eggie_ui::try_wake_existing(project_root.clone()) {
        return Ok(());
    }
    let client = eggie_ipc::DaemonClient::connect_default(BUILD_ID)?;
    eggie_ui::launch(project_root, client);
    Ok(())
}

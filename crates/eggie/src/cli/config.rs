use std::path::PathBuf;

use eggie_ipc::DaemonClient;
use eggie_protocol::{ClientRequest, DaemonResponse};

pub(super) fn run(flags: &[String]) -> i32 {
    match flags {
        [command] if command == "check" => check(None),
        [command, path] if command == "check" => check(Some(PathBuf::from(path))),
        [command] if command == "reload" => reload(),
        _ => {
            eprintln!("usage: eggie config check [path] | eggie config reload");
            2
        }
    }
}

fn check(path: Option<PathBuf>) -> i32 {
    let path = path.unwrap_or_else(eggie_storage::config_path);
    match eggie_lua::LuaConfig::load(&path) {
        Ok(runtime) => {
            let snapshot = runtime.snapshot(0);
            println!("path: {}", path.display());
            println!("exists: {}", if snapshot.exists { "yes" } else { "no" });
            println!("bindings: {}", snapshot.registered.bindings.len());
            println!("actions: {}", snapshot.registered.actions.len());
            0
        }
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

fn reload() -> i32 {
    let result = DaemonClient::connect_default(eggie_build_info::BUILD_ID)
        .and_then(|client| client.request(ClientRequest::ReloadConfig));
    match result {
        Ok(DaemonResponse::Config { snapshot }) => {
            println!(
                "reloaded {} (revision {})",
                snapshot.path.display(),
                snapshot.revision
            );
            0
        }
        Ok(DaemonResponse::Error { message }) => {
            eprintln!("{message}");
            1
        }
        Ok(response) => {
            eprintln!("unexpected config reload response: {response:?}");
            1
        }
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use eggie_domain::SessionId;
use eggie_ipc::DaemonClient;
use eggie_protocol::{ClientRequest, DaemonResponse, EventRecord};
use mlua::{Function, Lua, LuaSerdeExt, Table, Value};

pub(super) fn run(flags: &[String]) -> i32 {
    match run_inner(flags) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    }
}

fn run_inner(flags: &[String]) -> Result<()> {
    let Some(script) = flags.first() else {
        bail!("usage: eggie do <script.lua> [--session <id>] [args...]");
    };
    let path = PathBuf::from(script);
    let mut session_id = None;
    let mut args = Vec::new();
    let mut index = 1;
    while index < flags.len() {
        if flags[index] == "--session" {
            let value = flags.get(index + 1).context("--session needs an id")?;
            session_id = Some(value.parse::<SessionId>().context("invalid session id")?);
            index += 2;
        } else {
            args.push(flags[index].clone());
            index += 1;
        }
    }
    let source =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let client = DaemonClient::connect_default(eggie_build_info::BUILD_ID)?;
    let sequence = match client.request(ClientRequest::GetEventCursor)? {
        DaemonResponse::EventsUnchanged { sequence } => sequence,
        response => bail!("unexpected event cursor response: {response:?}"),
    };
    let cursor = Arc::new(Mutex::new(sequence));
    let lua = unsafe { Lua::unsafe_new() };
    let api = lua.create_table()?;
    let handlers = lua.create_table()?;
    lua.globals().set("__eggie_event_handlers", handlers)?;
    lua.globals().set("__eggie_script_loading", true)?;
    api.set("args", lua.to_value(&args)?)?;
    if let Some(id) = session_id {
        api.set("session_id", id.to_string())?;
    }

    api.set(
        "on",
        lua.create_function(|lua, (name, callback): (String, Function)| {
            if !lua.globals().get::<bool>("__eggie_script_loading")? {
                return Err(mlua::Error::external(
                    "eggie.on is only available while the script loads",
                ));
            }
            let handlers: Table = lua.globals().get("__eggie_event_handlers")?;
            let callbacks: Table = match handlers.get::<Value>(name.as_str())? {
                Value::Table(table) => table,
                _ => {
                    let table = lua.create_table()?;
                    handlers.set(name.as_str(), table.clone())?;
                    table
                }
            };
            callbacks.push(callback)?;
            Ok(())
        })?,
    )?;

    let wait_client = client.clone();
    let wait_cursor = cursor.clone();
    api.set(
        "wait",
        lua.create_function(move |lua, (name, timeout): (String, Option<f64>)| {
            if timeout
                .is_some_and(|seconds| !seconds.is_finite() || !(0. ..=86_400.).contains(&seconds))
            {
                return Err(mlua::Error::external(
                    "wait timeout must be between 0 and 86400 seconds",
                ));
            }
            let deadline =
                timeout.map(|seconds| Instant::now() + Duration::from_secs_f64(seconds.max(0.)));
            loop {
                let remaining = deadline.map_or(Duration::from_secs(30), |end| {
                    end.saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(30))
                });
                if remaining.is_zero() {
                    return Ok((Value::Nil, Value::String(lua.create_string("timeout")?)));
                }
                let response = wait_client
                    .request(ClientRequest::WaitForEvents {
                        after_sequence: *wait_cursor.lock().unwrap(),
                        session_id,
                        timeout_ms: remaining.as_millis() as u32,
                    })
                    .map_err(mlua::Error::external)?;
                match response {
                    DaemonResponse::Events { update } => {
                        if update.gap {
                            return Err(mlua::Error::external(
                                "event history gap; call eggie.sessions() to resynchronize",
                            ));
                        }
                        *wait_cursor.lock().unwrap() = update.sequence;
                        if let Some(event) =
                            update.events.into_iter().find(|event| event.name == name)
                        {
                            return Ok((
                                lua_event(lua, &event)?,
                                lua.to_value(&serde_json::json!({"session_id": event.session_id}))?,
                            ));
                        }
                    }
                    DaemonResponse::EventsUnchanged { .. } => {}
                    DaemonResponse::Error { message } => {
                        return Err(mlua::Error::external(message));
                    }
                    other => {
                        return Err(mlua::Error::external(format!(
                            "unexpected event response: {other:?}"
                        )));
                    }
                }
            }
        })?,
    )?;
    api.set(
        "sleep",
        lua.create_function(|_, seconds: f64| {
            if !seconds.is_finite() || !(0. ..=86_400.).contains(&seconds) {
                return Err(mlua::Error::external(
                    "sleep duration must be between 0 and 86400 seconds",
                ));
            }
            std::thread::sleep(Duration::from_secs_f64(seconds));
            Ok(())
        })?,
    )?;
    api.set(
        "stop",
        lua.create_function(|_, code: Option<i32>| -> mlua::Result<()> {
            std::process::exit(code.unwrap_or(0));
        })?,
    )?;
    let invoke_client = client.clone();
    api.set(
        "invoke",
        lua.create_function(move |lua, (name, args): (String, Option<Value>)| {
            let args = match args {
                Some(value) => lua.from_value(value)?,
                None => serde_json::Value::Null,
            };
            match invoke_client
                .request(ClientRequest::InvokeConfigAction {
                    revision: 0,
                    name,
                    context: eggie_config::ActionContext {
                        session_id: session_id.map(|id| id.to_string()),
                        ..Default::default()
                    },
                    args,
                })
                .map_err(mlua::Error::external)?
            {
                DaemonResponse::ConfigActionResult { actions } if actions.is_empty() => Ok(()),
                DaemonResponse::ConfigActionResult { .. } => {
                    Err(mlua::Error::external("action requires a GUI client"))
                }
                DaemonResponse::Error { message } => Err(mlua::Error::external(message)),
                response => Err(mlua::Error::external(format!(
                    "unexpected action response: {response:?}"
                ))),
            }
        })?,
    )?;
    let sessions_client = client.clone();
    api.set(
        "sessions",
        lua.create_function(move |lua, ()| {
            match sessions_client
                .request(ClientRequest::ListSessions)
                .map_err(mlua::Error::external)?
            {
                DaemonResponse::Sessions { sessions } => lua.to_value(&sessions),
                DaemonResponse::Error { message } => Err(mlua::Error::external(message)),
                response => Err(mlua::Error::external(format!(
                    "unexpected sessions response: {response:?}"
                ))),
            }
        })?,
    )?;
    lua.globals().set("eggie", api)?;
    if let Some(parent) = path.parent() {
        let package: Table = lua.globals().get("package")?;
        let original: String = package.get("path")?;
        let directory = if parent.as_os_str().is_empty() {
            std::borrow::Cow::Borrowed(".")
        } else {
            parent.to_string_lossy()
        };
        package.set(
            "path",
            format!("{directory}/?.lua;{directory}/?/init.lua;{original}"),
        )?;
    }
    lua.load(&source)
        .set_name(path.to_string_lossy().as_ref())
        .exec()
        .with_context(|| format!("running {}", path.display()))?;
    lua.globals().set("__eggie_script_loading", false)?;

    let handlers: Table = lua.globals().get("__eggie_event_handlers")?;
    if handlers.is_empty() {
        return Ok(());
    }
    loop {
        match client.request(ClientRequest::WaitForEvents {
            after_sequence: *cursor.lock().unwrap(),
            session_id,
            timeout_ms: 30_000,
        })? {
            DaemonResponse::Events { update } => {
                if update.gap {
                    eprintln!(
                        "Eggie event history has a gap before sequence {}",
                        update.sequence
                    );
                    let gap = EventRecord {
                        sequence: update.sequence,
                        timestamp_ms: 0,
                        name: "event.gap".to_owned(),
                        session_id: None,
                        payload: serde_json::json!({ "sequence": update.sequence }),
                    };
                    dispatch_event(&lua, &handlers, &gap)?;
                }
                *cursor.lock().unwrap() = update.sequence;
                for event in update.events {
                    dispatch_event(&lua, &handlers, &event)?;
                }
            }
            DaemonResponse::EventsUnchanged { .. } => {}
            DaemonResponse::Error { message } => bail!("{message}"),
            response => bail!("unexpected event response: {response:?}"),
        }
    }
}

fn dispatch_event(lua: &Lua, handlers: &Table, event: &EventRecord) -> Result<()> {
    let Value::Table(callbacks) = handlers.get::<Value>(event.name.as_str())? else {
        return Ok(());
    };
    for callback in callbacks.sequence_values::<Function>() {
        let callback = callback?;
        callback
            .call::<()>((
                lua.to_value(&serde_json::json!({ "session_id": event.session_id }))?,
                lua_event(lua, event)?,
            ))
            .with_context(|| format!("handling {} at sequence {}", event.name, event.sequence))?;
    }
    Ok(())
}

fn lua_event(lua: &Lua, event: &EventRecord) -> mlua::Result<Value> {
    let mut value = serde_json::to_value(event).map_err(mlua::Error::external)?;
    if let Some(payload) = event.payload.as_object() {
        if let Some(fields) = value.as_object_mut() {
            for (key, field) in payload {
                fields.insert(key.clone(), field.clone());
            }
        }
    }
    lua.to_value(&value)
}

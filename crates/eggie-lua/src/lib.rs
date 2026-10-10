//! Lua configuration execution. Serializable configuration lives in eggie-config.
use anyhow::{Context, Result, bail};
use eggie_config::*;
use mlua::{Function, Lua, LuaSerdeExt, Table, Value};
use parking_lot::Mutex;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
/// A loaded configuration owns its callbacks. Construct a replacement VM and
/// validate it before swapping the current instance.
pub struct LuaConfig {
    lua: Lua,
    registered: RegisteredConfig,
    path: PathBuf,
    exists: bool,
}

impl LuaConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        Self::from_source(path, &source, path.is_file())
    }

    pub fn from_source(path: &Path, source: &str, exists: bool) -> Result<Self> {
        // Local configuration is trusted user code, as in Rex. Running it on the
        // config executor keeps arbitrary Lua work off the GUI render thread.
        let lua = unsafe { Lua::unsafe_new() };
        let builder = Arc::new(Mutex::new(RegisteredConfig::default()));
        let actions = lua.create_table()?;
        lua.globals().set("__eggie_actions", actions)?;
        lua.globals().set("__eggie_config_loading", true)?;
        install_api(&lua, builder.clone())?;
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
        lua.load(source)
            .set_name(path.to_string_lossy().as_ref())
            .exec()
            .with_context(|| format!("evaluating {}", path.display()))?;
        lua.globals().set("__eggie_config_loading", false)?;
        let registered = builder.lock().clone();
        validate(&registered)?;
        Ok(Self {
            lua,
            registered,
            path: path.to_path_buf(),
            exists,
        })
    }

    pub fn snapshot(&self, revision: u64) -> ConfigSnapshot {
        ConfigSnapshot {
            revision,
            status_revision: revision,
            path: self.path.clone(),
            exists: self.exists,
            error: None,
            registered: self.registered.clone(),
        }
    }

    /// Only client operations are queued. The caller publishes them after the
    /// callback succeeds; an error discards the entire queue.
    pub fn invoke(
        &mut self,
        name: &str,
        context: &ActionContext,
        args: &serde_json::Value,
    ) -> Result<Vec<QueuedAction>> {
        if BUILTIN_ACTIONS.contains(&name) {
            if context.window_id.is_none() {
                bail!("client action needs a GUI context");
            }
            validate_builtin_action(name, args)?;
            return Ok(vec![QueuedAction {
                name: name.to_owned(),
                args: args.clone(),
            }]);
        }
        let actions: Table = self.lua.globals().get("__eggie_actions")?;
        let callback: Function = actions
            .get(name)
            .with_context(|| format!("unknown action: {name}"))?;
        let queued = Arc::new(Mutex::new(Vec::new()));
        let destination = queued.clone();
        let has_client = context.window_id.is_some();
        let client: Table = self.lua.globals().get::<Table>("eggie")?.get("client")?;
        client.set(
            "queue",
            self.lua.create_function(
                move |lua, (action, arguments): (String, Option<Value>)| {
                    if !has_client {
                        return Err(mlua::Error::external("client action needs a GUI context"));
                    }
                    if !BUILTIN_ACTIONS.contains(&action.as_str()) {
                        return Err(mlua::Error::external(format!(
                            "unknown client action: {action}"
                        )));
                    }
                    let args = match arguments {
                        Some(value) => lua.from_value(value)?,
                        None => serde_json::Value::Null,
                    };
                    validate_builtin_action(&action, &args).map_err(mlua::Error::external)?;
                    destination.lock().push(QueuedAction { name: action, args });
                    Ok(())
                },
            )?,
        )?;
        let lua_context = self.lua.to_value(context)?;
        let lua_args = self.lua.to_value(args)?;
        let result = callback.call::<()>((lua_context, lua_args));
        client.set("queue", Value::Nil)?;
        result.with_context(|| format!("running Lua action {name}"))?;
        let result = queued.lock().clone();
        Ok(result)
    }
}

fn install_api(lua: &Lua, builder: Arc<Mutex<RegisteredConfig>>) -> Result<()> {
    let api = lua.create_table()?;
    let ui = lua.create_table()?;
    let target = builder.clone();
    ui.set(
        "theme",
        lua.create_function(move |lua, (half, name): (String, String)| {
            ensure_load_phase(lua)?;
            validate_theme(&half, &name).map_err(mlua::Error::external)?;
            let mut config = target.lock();
            match half.as_str() {
                "light" => config.ui_light_theme = Some(name),
                "dark" => config.ui_dark_theme = Some(name),
                _ => unreachable!(),
            }
            Ok(())
        })?,
    )?;
    api.set("ui", ui)?;

    let terminal = lua.create_table()?;
    let target = builder.clone();
    terminal.set(
        "theme",
        lua.create_function(move |lua, (half, name): (String, String)| {
            ensure_load_phase(lua)?;
            validate_theme(&half, &name).map_err(mlua::Error::external)?;
            let mut config = target.lock();
            match half.as_str() {
                "light" => config.terminal_light_theme = Some(name),
                "dark" => config.terminal_dark_theme = Some(name),
                _ => unreachable!(),
            }
            Ok(())
        })?,
    )?;
    let target = builder.clone();
    terminal.set(
        "font",
        lua.create_function(move |lua, options: Table| {
            ensure_load_phase(lua)?;
            let family: String = options.get("family")?;
            let size: f32 = options.get("size")?;
            if family.trim().is_empty() || !size.is_finite() || !(8.0..=32.0).contains(&size) {
                return Err(mlua::Error::external(
                    "font needs a family and a size from 8 to 32",
                ));
            }
            target.lock().terminal_font = Some(FontSpec { family, size });
            Ok(())
        })?,
    )?;
    api.set("terminal", terminal)?;

    let target = builder.clone();
    api.set(
        "bind",
        lua.create_function(
            move |lua, (key, action, args): (String, String, Option<Value>)| {
                ensure_load_phase(lua)?;
                let key = normalize_key(&key).map_err(mlua::Error::external)?;
                let args = match args {
                    Some(value) => lua.from_value(value)?,
                    None => serde_json::Value::Null,
                };
                target
                    .lock()
                    .bindings
                    .push(BindingOp::Bind { key, action, args });
                Ok(())
            },
        )?,
    )?;
    let target = builder.clone();
    api.set(
        "unbind",
        lua.create_function(move |lua, key: String| {
            ensure_load_phase(lua)?;
            let key = normalize_key(&key).map_err(mlua::Error::external)?;
            target.lock().bindings.push(BindingOp::Unbind { key });
            Ok(())
        })?,
    )?;

    let target = builder;
    api.set(
        "action",
        lua.create_function(move |lua, options: Table| {
            ensure_load_phase(lua)?;
            let name: String = options.get("name")?;
            let title: String = options.get("title")?;
            let run: Function = options.get("run")?;
            if !valid_action_name(&name) || title.trim().is_empty() {
                return Err(mlua::Error::external(
                    "action needs a lowercase name and a title",
                ));
            }
            let actions: Table = lua.globals().get("__eggie_actions")?;
            if actions.contains_key(name.as_str())? || BUILTIN_ACTIONS.contains(&name.as_str()) {
                return Err(mlua::Error::external(format!("duplicate action: {name}")));
            }
            actions.set(name.as_str(), run)?;
            target.lock().actions.push(ActionSpec { name, title });
            Ok(())
        })?,
    )?;
    api.set("client", lua.create_table()?)?;
    lua.globals().set("eggie", api)?;
    Ok(())
}

fn validate(registered: &RegisteredConfig) -> Result<()> {
    let names: HashSet<_> = registered
        .actions
        .iter()
        .map(|action| action.name.as_str())
        .collect();
    for binding in &registered.bindings {
        if let BindingOp::Bind { action, .. } = binding
            && !BUILTIN_ACTIONS.contains(&action.as_str())
            && !names.contains(action.as_str())
        {
            bail!("unknown action: {action}");
        }
        if let BindingOp::Bind { action, args, .. } = binding
            && BUILTIN_ACTIONS.contains(&action.as_str())
        {
            validate_builtin_action(action, args)?;
        }
    }
    Ok(())
}

fn validate_builtin_action(name: &str, args: &serde_json::Value) -> Result<()> {
    if name == "pane.split" {
        let Some(direction) = args.get("direction").and_then(serde_json::Value::as_str) else {
            bail!("pane.split needs a direction");
        };
        if !matches!(direction, "right" | "left" | "up" | "down")
            || args.as_object().is_none_or(|object| object.len() != 1)
        {
            bail!("pane.split direction must be right, left, up, or down");
        }
    } else if !args.is_null() {
        bail!("built-in action {name} does not take arguments");
    }
    Ok(())
}

fn ensure_load_phase(lua: &Lua) -> mlua::Result<()> {
    if lua.globals().get::<bool>("__eggie_config_loading")? {
        Ok(())
    } else {
        Err(mlua::Error::external(
            "configuration registration is only available while init.lua loads",
        ))
    }
}

fn validate_theme(half: &str, name: &str) -> Result<()> {
    let dark = match half {
        "dark" => true,
        "light" => false,
        _ => bail!("theme half must be 'light' or 'dark'"),
    };
    if !eggie_themes::theme_catalog()
        .theme_by_name(name)
        .is_some_and(|theme| theme.is_dark() == dark)
    {
        bail!("unknown {half} theme: {name}");
    }
    Ok(())
}

fn normalize_key(key: &str) -> Result<String> {
    let mut strokes = Vec::new();
    for stroke in key.split('>') {
        let stroke = stroke.trim();
        let converted = if stroke == "+" || stroke.contains("-+") {
            stroke.to_owned()
        } else if let Some(modifiers) = stroke.strip_suffix("++") {
            format!("{}-+", modifiers.replace('+', "-"))
        } else {
            stroke.replace('+', "-")
        };
        let mut remaining = converted.as_str();
        if remaining.is_empty() {
            bail!("empty key in binding: {key}");
        }
        let mut modifiers = HashSet::new();
        while let Some((modifier, rest)) = remaining.split_once('-') {
            if matches!(modifier, "cmd" | "ctrl" | "alt" | "shift" | "fn") {
                if !modifiers.insert(modifier) {
                    bail!("duplicate modifier in binding: {key}");
                }
                remaining = rest;
            } else {
                break;
            }
        }
        if remaining.is_empty()
            || remaining.chars().any(char::is_whitespace)
            || (remaining.contains('-') && remaining != "-")
        {
            bail!("invalid key in binding: {key}");
        }
        let mut canonical = String::new();
        for modifier in ["fn", "ctrl", "alt", "cmd", "shift"] {
            if modifiers.contains(modifier) {
                canonical.push_str(modifier);
                canonical.push('-');
            }
        }
        canonical.push_str(remaining);
        strokes.push(canonical);
    }
    Ok(strokes.join(">"))
}

fn valid_action_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'.'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_and_deferred_action() {
        let mut loaded = LuaConfig::from_source(
            Path::new("/tmp/init.lua"),
            r#"
            eggie.ui.theme("dark", "Builtin Dark")
            eggie.bind("cmd-shift-s", "scratch")
            eggie.action {
                name = "scratch", title = "Scratch",
                run = function(ctx, args)
                    eggie.client.queue("new_tab")
                end,
            }
        "#,
            true,
        )
        .unwrap();
        assert_eq!(
            loaded.snapshot(7).registered.ui_dark_theme.as_deref(),
            Some("Builtin Dark")
        );
        let actions = loaded
            .invoke(
                "scratch",
                &ActionContext {
                    cwd: Some("/tmp".into()),
                    window_id: Some("window".into()),
                    ..Default::default()
                },
                &serde_json::Value::Null,
            )
            .unwrap();
        assert_eq!(actions[0].name, "new_tab");
        assert!(actions[0].args.is_null());
    }

    #[test]
    fn invalid_bind_fails_as_one_transaction() {
        let result = LuaConfig::from_source(
            Path::new("init.lua"),
            "eggie.bind('cmd-z', 'missing')",
            true,
        );
        assert!(result.is_err());
    }

    #[test]
    fn callback_error_discards_queued_client_actions() {
        let mut loaded = LuaConfig::from_source(
            Path::new("init.lua"),
            r#"
            eggie.action {
                name = "broken", title = "Broken",
                run = function()
                    eggie.client.queue("tab.new")
                    error("failed after queueing")
                end,
            }
        "#,
            true,
        )
        .unwrap();
        let result = loaded.invoke(
            "broken",
            &ActionContext {
                window_id: Some("window".into()),
                ..Default::default()
            },
            &serde_json::Value::Null,
        );
        assert!(result.is_err());
    }

    #[test]
    fn registration_is_closed_after_loading() {
        let mut loaded = LuaConfig::from_source(
            Path::new("init.lua"),
            r#"
            eggie.action {
                name = "late", title = "Late",
                run = function() eggie.bind("cmd+x", "new_tab") end,
            }
        "#,
            true,
        )
        .unwrap();
        let result = loaded.invoke(
            "late",
            &ActionContext {
                window_id: Some("window".into()),
                ..Default::default()
            },
            &serde_json::Value::Null,
        );
        assert!(result.is_err());
    }

    #[test]
    fn key_sequences_and_builtin_arguments_are_validated() {
        let loaded = LuaConfig::from_source(
            Path::new("init.lua"),
            "eggie.bind('cmd+k>cmd+t', 'pane.split', { direction = 'down' })",
            true,
        )
        .unwrap();
        assert_eq!(
            loaded.snapshot(1).registered.bindings,
            vec![BindingOp::Bind {
                key: "cmd-k>cmd-t".into(),
                action: "pane.split".into(),
                args: serde_json::json!({ "direction": "down" }),
            }]
        );
        assert!(
            LuaConfig::from_source(
                Path::new("init.lua"),
                "eggie.bind('cmd+k', 'pane.split', { direction = 'sideways' })",
                true
            )
            .is_err()
        );
        assert_eq!(normalize_key("shift+cmd+d").unwrap(), "cmd-shift-d");
        assert_eq!(normalize_key("cmd++").unwrap(), "cmd-+");
        assert!(normalize_key("cmd+cmd+d").is_err());
    }
}

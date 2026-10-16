//! Pure CLI discovery shared by the command dispatcher and in-app completion.
use crate::automation_methods;
use eggie_completion::{CompletionContext, CompletionItem, CompletionKind, PromptInput, rank};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliAction {
    Help,
    Version,
    ListColors,
    ListThemes,
}
impl CliAction {
    pub const ALL: &'static [Self] = &[
        Self::Help,
        Self::Version,
        Self::ListColors,
        Self::ListThemes,
    ];
    pub fn from_plus(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.name() == name)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Version => "version",
            Self::ListColors => "list-colors",
            Self::ListThemes => "list-themes",
        }
    }
    pub fn summary(self) -> &'static str {
        match self {
            Self::Help => "显示帮助",
            Self::Version => "显示版本与构建信息",
            Self::ListColors => "列出命名颜色",
            Self::ListThemes => "列出主题",
        }
    }
}

/// Values are supplied by the GUI that owns these resources, never by a shell subprocess.
#[derive(Default)]
pub struct CliCompletionResources {
    values: Vec<CliCompletionValue>,
}
struct CliCompletionValue {
    field: &'static str,
    value: Value,
    label: String,
    scope: Option<(&'static str, Value)>,
}
impl CliCompletionResources {
    pub fn push(&mut self, field: &'static str, value: impl Into<Value>, label: impl Into<String>) {
        self.values.push(CliCompletionValue {
            field,
            value: value.into(),
            label: label.into(),
            scope: None,
        });
    }
    pub fn push_scoped(
        &mut self,
        field: &'static str,
        value: impl Into<Value>,
        label: impl Into<String>,
        scope_field: &'static str,
        scope: impl Into<Value>,
    ) {
        self.values.push(CliCompletionValue {
            field,
            value: value.into(),
            label: label.into(),
            scope: Some((scope_field, scope.into())),
        });
    }
    fn values<'a>(
        &'a self,
        field: &'a str,
        fields: &'a Map<String, Value>,
    ) -> impl Iterator<Item = (&'a Value, &'a str)> {
        self.values
            .iter()
            .filter(move |v| {
                v.field == field
                    && v.scope.as_ref().is_none_or(|(key, value)| {
                        fields.get(*key).is_none_or(|selected| selected == value)
                    })
            })
            .map(|v| (&v.value, v.label.as_str()))
    }
}

/// None delegates to the existing providers (notably files for `do` and `config check`).
pub fn eggie_completions(
    input: &PromptInput,
    resources: &CliCompletionResources,
) -> Option<Vec<CompletionItem>> {
    let ctx = CompletionContext::parse(input)?;
    let args = eggie_args(&ctx)?;
    let methods = automation_methods();
    let mut items = Vec::new();
    let mut add =
        |value: &str, description: &str, kind| items.push(ctx.item(value, description, kind));
    let help = [("--help", "显示帮助"), ("-h", "显示帮助")];
    match args.as_slice() {
        [] => {
            for action in CliAction::ALL {
                add(
                    &format!("+{}", action.name()),
                    action.summary(),
                    CompletionKind::Command,
                );
            }
            for (name, description) in [
                ("api", "调用本地控制接口"),
                ("config", "检查或重载配置"),
                ("do", "运行 Lua 脚本"),
                ("mcp", "启动 MCP 服务"),
            ] {
                add(name, description, CompletionKind::Command);
            }
            let mut groups = std::collections::BTreeSet::new();
            for method in &methods {
                groups.insert(method.name.split_once('.').unwrap().0);
            }
            for name in groups {
                add(name, group_description(name), CompletionKind::Command);
            }
            for (name, description) in help.into_iter().chain([("--version", "显示版本")]) {
                add(name, description, CompletionKind::Option);
            }
        }
        ["config"] => {
            add("check", "检查配置文件", CompletionKind::Command);
            add("reload", "重载配置", CompletionKind::Command);
        }
        ["config", "check"] => return None,
        ["config", ..] => {}
        ["do"] => return None,
        ["do", _, rest @ ..] => {
            if rest.last() == Some(&"--session") {
                for (value, label) in resources.values("session_id", &Map::new()) {
                    if let Some(value) = value.as_str() {
                        add(value, label, CompletionKind::Argument);
                    }
                }
            } else if !rest.contains(&"--session")
                && (ctx.query.is_empty() || ctx.query.starts_with('-'))
            {
                add("--session", "指定终端", CompletionKind::Option);
            } else {
                return None;
            }
        }
        ["mcp", rest @ ..] => {
            for (name, description) in help
                .into_iter()
                .chain([("--allow-write", "允许修改项目和终端")])
            {
                if !rest.contains(&name) {
                    add(name, description, CompletionKind::Option);
                }
            }
        }
        [action, rest @ ..] if action.starts_with('+') => {
            let action = CliAction::from_plus(&action[1..])?;
            let extra: &[(&str, &str)] = match action {
                CliAction::ListThemes => &[
                    ("--dark", "深色主题"),
                    ("--light", "浅色主题"),
                    ("--plain", "纯文本输出"),
                ],
                CliAction::ListColors => &[("--plain", "纯文本输出")],
                _ => &[],
            };
            for &(name, description) in extra.iter().chain(help.iter()) {
                if !rest.contains(&name) {
                    add(name, description, CompletionKind::Option);
                }
            }
        }
        [group]
            if *group == "api"
                || methods
                    .iter()
                    .any(|m| m.name.starts_with(&format!("{group}."))) =>
        {
            for (name, description) in help {
                add(name, description, CompletionKind::Option);
            }
            for method in &methods {
                if *group == "api" {
                    add(method.name, method.description, CompletionKind::Command);
                } else if let Some(name) = method.name.strip_prefix(&format!("{group}.")) {
                    add(name, method.description, CompletionKind::Command);
                }
            }
        }
        [group, operation] => {
            let name = if *group == "api" {
                operation.to_string()
            } else {
                format!("{group}.{operation}")
            };
            let method = methods.iter().find(|m| m.name == name)?;
            return Some(json_completions(&ctx, &method.schema, resources));
        }
        _ => {}
    }
    Some(rank(items, &ctx.query))
}
pub fn is_eggie_command(input: &PromptInput) -> bool {
    CompletionContext::parse(input).is_some_and(|ctx| eggie_args(&ctx).is_some())
}
fn eggie_args(ctx: &CompletionContext) -> Option<Vec<&str>> {
    let mut words = ctx.words.iter().map(String::as_str).peekable();
    while words
        .peek()
        .is_some_and(|w| *w == "command" || *w == "env" || assignment(w))
    {
        words.next();
    }
    if std::path::Path::new(words.next()?).file_name()?.to_str()? != "eggie" {
        return None;
    }
    Some(words.collect())
}

fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name
                .chars()
                .enumerate()
                .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit())
    })
}
fn group_description(name: &str) -> &'static str {
    match name {
        "projects" => "管理项目",
        "tabs" => "管理标签页",
        "panes" => "拆分窗格",
        "terminal" => "读取终端或发送输入",
        "commands" => "读取命令历史和输出",
        "windows" => "查看窗口",
        "profiles" => "查看配置方案",
        _ => "",
    }
}

// JSON is one shell argument; complete the object without discarding fields already entered.
fn json_completions(
    ctx: &CompletionContext,
    schema: &Value,
    resources: &CliCompletionResources,
) -> Vec<CompletionItem> {
    if ctx.query.starts_with('-') {
        return (ctx.query == "-")
            .then(|| ctx.item("-", "从标准输入读取 JSON 参数", CompletionKind::Argument))
            .into_iter()
            .collect();
    }
    let Some((fields, slot)) = json_slot(&ctx.query) else {
        return Vec::new();
    };
    let properties = schema["properties"].as_object().unwrap();
    if fields.keys().any(|key| !properties.contains_key(key)) {
        return Vec::new();
    }
    let default = |key: &str, property: &Value| -> Value {
        resources
            .values(key, &fields)
            .next()
            .map(|(v, _)| v.clone())
            .unwrap_or_else(|| {
                if let Some(v) = property.get("default") {
                    v.clone()
                } else if let Some(v) = property["enum"].as_array().and_then(|a| a.first()) {
                    v.clone()
                } else {
                    match property["type"].as_str() {
                        Some("boolean") => json!(false),
                        Some("integer") => property.get("minimum").cloned().unwrap_or(json!(0)),
                        _ => json!(""),
                    }
                }
            })
    };
    let mut base = fields.clone();
    for key in schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        base.entry(key.to_owned())
            .or_insert_with(|| default(key, &properties[key]));
    }
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut emit = |object: Map<String, Value>, label: &str| {
        let text = Value::Object(object).to_string();
        if seen.insert(text.clone()) {
            items.push(ctx.item(&text, label, CompletionKind::Argument));
        }
        items.len() >= eggie_completion::MAX_CANDIDATES
    };
    if ctx.query.is_empty() {
        let summary = base
            .iter()
            .filter_map(|(key, value)| {
                resources
                    .values(key, &base)
                    .find(|(v, _)| *v == value)
                    .map(|(_, label)| label)
            })
            .collect::<Vec<_>>()
            .join(" · ");
        emit(
            base.clone(),
            if summary.is_empty() {
                "JSON 参数"
            } else {
                &summary
            },
        );
    }
    for (key, property) in properties {
        let prefix = match &slot {
            JsonSlot::Key(prefix) if !fields.contains_key(key) && key.starts_with(prefix) => "",
            JsonSlot::Value(field, prefix) if field == key => prefix,
            _ => continue,
        };
        let mut values: Vec<(Value, String)> = resources
            .values(key, &base)
            .take(eggie_completion::MAX_CANDIDATES)
            .map(|(v, label)| (v.clone(), label.into()))
            .collect();
        if let Some(options) = property["enum"].as_array() {
            values.extend(
                options
                    .iter()
                    .map(|v| (v.clone(), v.as_str().unwrap_or_default().into())),
            );
        }
        if property["type"] == "boolean" {
            values.extend([(json!(true), "true".into()), (json!(false), "false".into())]);
        }
        if values.is_empty() {
            values.push((default(key, property), key.clone()));
        }
        for (value, label) in values {
            let text = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            if !text.starts_with(prefix) {
                continue;
            }
            let mut object = base.clone();
            object.insert(key.clone(), value);
            if emit(object, &format!("{key} · {label}")) {
                return items;
            }
        }
    }
    items
}

enum JsonSlot {
    Key(String),
    Value(String, String),
}
fn partial_string(text: &str) -> Option<String> {
    serde_json::from_str::<String>(&format!("{text}\"")).ok()
}
fn json_slot(query: &str) -> Option<(Map<String, Value>, JsonSlot)> {
    if query.trim().is_empty() {
        return Some((Map::new(), JsonSlot::Key(String::new())));
    }
    let mut text = query.trim_start().strip_prefix('{')?.trim_start();
    let mut fields = Map::new();
    loop {
        if text.is_empty() || text == "}" {
            return Some((fields, JsonSlot::Key(String::new())));
        }
        let mut key_stream = serde_json::Deserializer::from_str(text).into_iter::<String>();
        let key = match key_stream.next()? {
            Ok(key) => key,
            Err(error) if error.is_eof() => {
                return Some((fields, JsonSlot::Key(partial_string(text)?)));
            }
            _ => return None,
        };
        text = text[key_stream.byte_offset()..].trim_start();
        if text.is_empty() {
            return Some((fields, JsonSlot::Key(key)));
        }
        text = text.strip_prefix(':')?.trim_start();
        if text.is_empty() {
            return Some((fields, JsonSlot::Value(key, String::new())));
        }
        let mut value_stream = serde_json::Deserializer::from_str(text).into_iter::<Value>();
        match value_stream.next()? {
            Ok(value) => {
                if value.is_number() && text[value_stream.byte_offset()..].is_empty() {
                    return Some((fields, JsonSlot::Value(key, text.into())));
                }
                fields.insert(key, value);
                text = text[value_stream.byte_offset()..].trim_start();
            }
            Err(error) if error.is_eof() => {
                return Some((
                    fields,
                    JsonSlot::Value(
                        key,
                        if text.starts_with('"') {
                            partial_string(text)?
                        } else {
                            text.into()
                        },
                    ),
                ));
            }
            _ => return None,
        }
        if text.is_empty() || text == "}" {
            return Some((fields, JsonSlot::Key(String::new())));
        }
        text = text.strip_prefix(',')?.trim_start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    const OTHER: &str = "00000000-0000-4000-8000-000000000002";
    fn input(text: &str) -> PromptInput {
        PromptInput {
            text: text.into(),
            cursor: text.len(),
            shell: "zsh".into(),
            native: true,
            ..Default::default()
        }
    }
    fn resources() -> CliCompletionResources {
        let mut r = CliCompletionResources::default();
        for key in ["session_id", "project_id", "window_id", "profile_id"] {
            r.push(key, ID, "Eggie · 中文标题");
            r.push(key, OTHER, "其他终端");
        }
        r.push_scoped("group_id", ID, "窗格", "window_id", ID);
        r.push_scoped("group_id", OTHER, "其他窗格", "window_id", OTHER);
        r.push_scoped("command_id", 7, "echo seven", "session_id", ID);
        r.push_scoped("command_id", 9, "echo nine", "session_id", OTHER);
        r
    }
    fn complete(text: &str) -> Vec<CompletionItem> {
        eggie_completions(&input(text), &resources()).expect("Eggie context")
    }
    fn labels(text: &str) -> Vec<String> {
        complete(text).into_iter().map(|i| i.label).collect()
    }
    #[test]
    fn command_catalog_matches_dispatcher_and_api() {
        let root = labels("eggie ");
        for a in CliAction::ALL {
            assert!(root.contains(&format!("+{}", a.name())));
        }
        for m in automation_methods() {
            let (group, operation) = m.name.split_once('.').unwrap();
            assert!(root.contains(&group.into()));
            assert!(labels(&format!("eggie {group} ")).contains(&operation.into()));
            assert!(labels("eggie api ").contains(&m.name.into()));
            let params: Value =
                serde_json::from_str(&labels(&format!("eggie {group} {operation} "))[0]).unwrap();
            serde_json::from_value::<crate::AutomationRequest>(
                json!({"method":m.name,"params":params}),
            )
            .unwrap();
        }
    }
    #[test]
    fn options_are_contextual_and_used_flags_are_omitted() {
        assert!(labels("eggie +list-themes --").contains(&"--dark".into()));
        assert!(!labels("eggie +list-themes --dark --").contains(&"--dark".into()));
        assert!(!labels("eggie +list-colors --").contains(&"--dark".into()));
        assert!(labels("eggie mcp --").contains(&"--allow-write".into()));
        assert!(!labels("eggie mcp --allow-write --").contains(&"--allow-write".into()));
        assert_eq!(labels("eggie config re"), ["reload"]);
        assert!(labels("eggie config reload ").is_empty());
    }
    #[test]
    fn file_arguments_keep_the_existing_filesystem_provider() {
        for text in [
            "eggie do ",
            "eggie do ./sc",
            "eggie config check ",
            "eggie config check '~/文",
            "eggie do script.lua --session x argument",
        ] {
            assert!(
                eggie_completions(&input(text), &resources()).is_none(),
                "{text}"
            );
        }
        assert_eq!(labels("eggie do script.lua --"), ["--session"]);
        assert_eq!(labels("eggie do script.lua --session "), [ID, OTHER]);
    }
    #[test]
    fn executable_paths_environment_and_command_boundaries() {
        for text in [
            "/Applications/Eggie.app/Contents/MacOS/eggie tab",
            "FOO=bar eggie tab",
            "env FOO=bar command eggie tab",
            "echo hi | eggie tab",
            "pwd; eggie tab",
        ] {
            assert_eq!(labels(text), ["tabs"]);
        }
        for text in [
            "echo eggie tab",
            "ssh host eggie tab",
            "eggie api tabs.list '{}' | cat ",
            "git ",
            "noteggie ",
        ] {
            assert!(eggie_completions(&input(text), &resources()).is_none());
        }
    }
    #[test]
    fn dynamic_objects_have_names_and_safe_single_argument_edits() {
        let items = complete("eggie tabs focus ");
        assert_eq!(items.len(), 2);
        assert!(items[0].description.contains("中文标题"));
        for item in items {
            let text = format!("eggie tabs focus {} ", item.insert_text);
            let parsed = CompletionContext::parse(&input(&text)).unwrap();
            let params: Value = serde_json::from_str(parsed.words.last().unwrap()).unwrap();
            assert!(params["session_id"] == ID || params["session_id"] == OTHER);
            assert!(
                item.replacement(&input("eggie tabs focus "))
                    .unwrap()
                    .starts_with(b"\x18\x1e")
            );
        }
    }
    #[test]
    fn partial_json_keys_values_and_existing_fields() {
        let items = complete(
            "eggie tabs rename '{\"title\":\"中文 O’Reilly\",\"session_id\":\"00000000-0000-4000-8000-000000000002",
        );
        assert_eq!(items.len(), 1);
        let params: Value = serde_json::from_str(&items[0].label).unwrap();
        assert_eq!(params, json!({"title":"中文 O’Reilly","session_id":OTHER}));
        assert!(items[0].insert_text.starts_with('\''));
        assert!(
            complete("eggie tabs list '{\"project_")
                .iter()
                .all(|i| i.description.starts_with("project_id"))
        );
        assert!(complete("eggie tabs focus '{\"session_id\":\"bogus").is_empty());
        assert!(complete("eggie tabs focus '{\"typo\":1,").is_empty());
    }
    #[test]
    fn enum_boolean_and_scoped_resource_values() {
        let directions: Vec<_> = complete("eggie panes split '{\"direction\":\"")
            .iter()
            .map(|i| serde_json::from_str::<Value>(&i.label).unwrap()["direction"].clone())
            .collect();
        assert_eq!(directions.len(), 4);
        let flags = labels("eggie tabs close '{\"force\":t");
        assert_eq!(flags.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(&flags[0]).unwrap()["force"],
            true
        );
        let text = format!("eggie commands output '{{\"session_id\":\"{OTHER}\",\"command_id\":");
        assert!(
            complete(&text)
                .iter()
                .all(|i| serde_json::from_str::<Value>(&i.label).unwrap()["command_id"] == 9)
        );
        let text = format!("eggie tabs move '{{\"window_id\":\"{OTHER}\",\"group_id\":\"");
        assert!(
            complete(&text)
                .iter()
                .all(|i| serde_json::from_str::<Value>(&i.label).unwrap()["group_id"] == OTHER)
        );
    }
    #[test]
    fn mid_word_replaces_only_the_current_word() {
        let mut p = input("eggie tabs focXX tail");
        p.cursor = "eggie tabs foc".len();
        let items = eggie_completions(&p, &resources()).unwrap();
        assert_eq!(items[0].label, "focus");
        assert_eq!(
            &p.text[items[0].replace_start..items[0].replace_end],
            "focXX"
        );
        assert_eq!(&p.text[items[0].replace_end..], " tail");
    }
    #[test]
    fn missing_resources_and_invalid_input_do_not_fail() {
        assert!(
            !eggie_completions(&input("eggie "), &CliCompletionResources::default())
                .unwrap()
                .is_empty()
        );
        for text in [
            "eggie api terminal.send '{",
            "eggie tabs focus '{broken",
            "eggie \x1b",
        ] {
            let _ = eggie_completions(&input(text), &CliCompletionResources::default());
        }
    }
}

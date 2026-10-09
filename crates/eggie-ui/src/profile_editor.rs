//! Profile 编辑器的纯逻辑层:文本编辑面 ↔ [`Profile`] 字段之间的转换。
//!
//! 刻意与渲染分离。设置窗口里每个 profile 字段都由一个 [`crate::text_input::TextInput`] 承载,
//! 而 `Profile` 里存的是结构化值(`BTreeMap`、`Option<PathBuf>`、`Option<u16>`、`RemoteTarget`)。
//! 两个方向的转换都放在这里,于是「用户输入什么会变成什么」可以脱离 GPUI 单测 —— 其中 env 那一对
//! 尤其需要:变量名的合法性正是[远程 bootstrap 注入漏洞](eggie_domain::is_valid_env_key)的根源,
//! 而编辑器是它的第一道关。

use std::collections::BTreeMap;

use eggie_domain::{Profile, RemoteTarget, is_valid_env_key};

/// 把 env 编辑框的文本解析成 `Profile::env`,并报告被丢弃的行数。
///
/// 每行一条 `NAME=value`。空行与仅含空白的行是排版,不计入丢弃数;`#` 开头的行按注释跳过,同样不计。
/// 其余任何不合法的行(无 `=`、名字非法)都计入 `rejected`,由 UI 显示出来 —— 静默丢弃会让一个手误
/// 变成「为什么我的变量没生效」这种极难排查的问题。
///
/// 名字用 [`is_valid_env_key`] 校验而非转义:名字落在 `NAME=value` 的**左侧**,那不是可加引号的位置。
/// 这里拒绝掉,daemon 侧仍会再校验一次(profiles.json 可以被手工编辑,编辑器不是唯一入口)。
///
/// value **不** trim:尾随空格在 shell 里是有意义的值,而用户既然写了就当作有意。名字两侧的空白则
/// 允许(`FOO = bar` 里的 `FOO `),因为名字里根本不可能合法地含空白 —— 那必然是排版而非本意。
pub(crate) fn parse_env(text: &str) -> (BTreeMap<String, String>, usize) {
    let mut env = BTreeMap::new();
    let mut rejected = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            rejected += 1;
            continue;
        };
        let key = key.trim();
        if !is_valid_env_key(key) {
            rejected += 1;
            continue;
        }
        env.insert(key.to_owned(), value.to_owned());
    }
    (env, rejected)
}

/// 把 `Profile::env` 渲染回编辑框文本。[`parse_env`] 的逆向。
///
/// `BTreeMap` 保证顺序稳定,所以重新打开设置窗口时行序不会跳动(这也是 `Profile::env` 选 `BTreeMap`
/// 而非 `HashMap` 的原因)。
///
/// **跳过 value 含换行的条目** —— 每行一条的格式无法表达它们。见 [`unrepresentable_env`]。
pub(crate) fn format_env(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .filter(|(_, value)| is_representable(value))
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// value 能否放进「每行一条」的编辑框。
///
/// 含 `\n` / `\r` 的 value 不能:它会被读成两行,前半截被当作 value、后半截当作一条非法的行 ——
/// 实测 `SCRIPT=line1\nline2` 会变成 `SCRIPT=line1` 外加一句「已忽略 1 行」的误导性警告。
fn is_representable(value: &str) -> bool {
    !value.contains('\n') && !value.contains('\r')
}

/// `env` 中无法在编辑框里表示的条目(value 含换行)。
///
/// 这类 value 在 POSIX 下完全合法(本地经 `execve` 传递,远程被单引号包裹),只是无法用这个
/// 行式编辑器编辑。所以:不显示、不改写、原样保留 —— 回写时由调用方合并回去,于是编辑其它变量
/// 不会毁掉它。要改它只能编辑 `profiles.json`。
///
/// 选择「保留」而非「转义」是刻意的:任何 `\n` 转义方案都会改变用户**键入**的反斜杠的含义,
/// 让 `C:\new` 这类路径变成含换行的值 —— 用一个更常见的坑换掉一个只有手工编辑才能碰到的坑。
pub(crate) fn unrepresentable_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .filter(|(_, value)| !is_representable(value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// 把 shell 参数编辑框的一行文本切成 argv。
///
/// v1 不做引号解析:按空白切分。含空格的单个参数无法表达 —— 这与设置窗口里全局 shell 参数的既有
/// 行为一致(见 `settings_window::on_shell_args_event`),两处保持同一套规则比各自聪明更重要。
pub(crate) fn parse_args(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// 从三个远程字段构造 [`RemoteTarget`];host 为空表示「这是本地 profile」,返回 `None`。
///
/// 端口解析失败(非数字、越界)按未设置处理,而不是把整个远程目标丢掉:用户正在输入 `2` 想打 `22` 的
/// 中间态不应该让主机名凭空消失。真正会导致目标被拒的是 host/user 本身不合法,那由
/// [`RemoteTarget::is_usable`] 判定,并由 UI 显式提示。
pub(crate) fn parse_remote(host: &str, user: &str, port: &str) -> Option<RemoteTarget> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let user = user.trim();
    Some(RemoteTarget {
        host: host.to_owned(),
        user: (!user.is_empty()).then(|| user.to_owned()),
        port: port.trim().parse::<u16>().ok().filter(|port| *port != 0),
    })
}

/// 一个 profile 的全部文本字段快照,用于把 store 里的值播种进各个输入框。
pub(crate) struct ProfileFields {
    pub(crate) name: String,
    pub(crate) shell_program: String,
    pub(crate) shell_args: String,
    pub(crate) cwd: String,
    pub(crate) env: String,
    pub(crate) remote_host: String,
    pub(crate) remote_user: String,
    pub(crate) remote_port: String,
}

impl ProfileFields {
    pub(crate) fn from_profile(profile: &Profile) -> Self {
        let remote = profile.remote.as_ref();
        Self {
            name: profile.name.clone(),
            shell_program: profile.shell_program.clone(),
            shell_args: profile.shell_args.join(" "),
            cwd: profile
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned())
                .unwrap_or_default(),
            env: format_env(&profile.env),
            remote_host: remote.map(|remote| remote.host.clone()).unwrap_or_default(),
            remote_user: remote
                .and_then(|remote| remote.user.clone())
                .unwrap_or_default(),
            remote_port: remote
                .and_then(|remote| remote.port)
                .map(|port| port.to_string())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_parsing_rejects_unusable_names_and_counts_them() {
        let (env, rejected) = parse_env(
            "AWS_PROFILE=prod\n\
             _private=1\n\
             \n\
             # a comment\n\
             no-equals-sign\n\
             1STARTS_WITH_DIGIT=x\n\
             HAS SPACE=x\n\
             =just-a-value\n",
        );
        assert_eq!(env.len(), 2, "only the two well-formed lines survive");
        assert_eq!(env.get("AWS_PROFILE").map(String::as_str), Some("prod"));
        assert_eq!(env.get("_private").map(String::as_str), Some("1"));
        // 空行与注释是排版,不算错误;其余四行都要被计入,否则手误会静默消失。
        assert_eq!(rejected, 4, "malformed lines are counted, blanks and comments are not");
    }

    #[test]
    fn env_parsing_refuses_the_injection_shapes() {
        // 这些名字在远程 bootstrap 里会脱离赋值、变成独立命令(上一步实测可执行任意命令)。
        // 编辑器是第一道关,必须在这里就拒掉,而不是留给 daemon。
        let (env, rejected) = parse_env("X;id=v\nX$(id)=v\nX`id`=v\n");
        assert!(env.is_empty(), "no injection-shaped name may become a variable");
        assert_eq!(rejected, 3);

        // 含换行的名字无法从「每行一条」的文本里产生 —— 换行本身就是行分隔符,所以
        // `X\nid > /tmp/pwned` 会被读成两行,各自单独判定。这里确认它不会意外拼出一个变量。
        let (env, _) = parse_env("X\nid > /tmp/pwned\n");
        assert!(env.is_empty(), "neither half of a split line forms a variable");
    }

    #[test]
    fn env_values_keep_everything_after_the_first_equals() {
        let (env, rejected) = parse_env("URL=https://example.com/?a=1&b=2\nEMPTY=\nSPACED= two words \n");
        assert_eq!(rejected, 0);
        // 只在**首个** `=` 处切分:value 里的 `=` 属于 value。
        assert_eq!(
            env.get("URL").map(String::as_str),
            Some("https://example.com/?a=1&b=2")
        );
        // 空 value 是合法的(`FOO=` 在 shell 里表示设为空串,与未设置不同)。
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        // value 不 trim:shell 里尾随空格是有意义的值。
        assert_eq!(env.get("SPACED").map(String::as_str), Some(" two words "));
    }

    #[test]
    fn env_round_trips_through_the_text_editor() {
        let mut original = BTreeMap::new();
        original.insert("B_SECOND".to_owned(), "2".to_owned());
        original.insert("A_FIRST".to_owned(), "value with spaces".to_owned());
        let text = format_env(&original);
        // BTreeMap 的顺序稳定,所以重开设置窗口时行序不会跳动。
        assert_eq!(text, "A_FIRST=value with spaces\nB_SECOND=2");
        let (parsed, rejected) = parse_env(&text);
        assert_eq!(parsed, original, "a round trip must be lossless");
        assert_eq!(rejected, 0);
    }

    #[test]
    fn an_empty_host_means_a_local_profile() {
        assert!(parse_remote("", "deploy", "22").is_none(), "no host = local");
        assert!(parse_remote("   ", "", "").is_none(), "whitespace-only host = local");
    }

    #[test]
    fn remote_fields_parse_into_a_target() {
        let target = parse_remote(" prod.example.com ", " deploy ", " 2222 ")
            .expect("a host makes it remote");
        assert_eq!(target.host, "prod.example.com", "surrounding whitespace is layout");
        assert_eq!(target.user.as_deref(), Some("deploy"));
        assert_eq!(target.port, Some(2222));
        assert!(target.is_usable());

        // 空 user/port 表示「交给 ssh 决定」,不是空串。
        let bare = parse_remote("host", "", "").expect("host only");
        assert_eq!(bare.user, None);
        assert_eq!(bare.port, None);
    }

    #[test]
    fn a_half_typed_port_keeps_the_host() {
        // 用户想打 22,已经打了 2 —— 中间态不应让主机名消失。同理非数字、越界、以及 0
        // (0 号端口不是可连接的目标)都按「未设置」处理,而不是丢掉整个目标。
        for port in ["", "  ", "not-a-number", "70000", "-1", "0"] {
            let target = parse_remote("host", "", port)
                .unwrap_or_else(|| panic!("host must survive port {port:?}"));
            assert_eq!(target.host, "host");
            assert_eq!(target.port, None, "port {port:?} falls back to the ssh default");
        }
    }

    #[test]
    fn an_ssh_option_shaped_target_is_built_but_reports_itself_unusable() {
        // 编辑器仍然照实构造 —— 用户正在输入,不该被半路改写成别的主机。是否可用由
        // `is_usable` 判定,UI 据此提示,启动路径据此丢弃。这样「所见」与「所得」一致。
        let target = parse_remote("-oProxyCommand=touch /tmp/pwned", "", "")
            .expect("a non-empty host still builds");
        assert!(
            !target.is_usable(),
            "a host that ssh would parse as an option must report itself unusable"
        );
    }

    #[test]
    fn the_text_round_trip_is_lossy_for_input_the_parser_refuses() {
        // This is *why* the settings pane must compare before writing (see `on_profile_input_event`):
        // seeding the editor and parsing straight back is not an identity when the stored data contains
        // something the parser rejects. A hand-edited `profiles.json` can, so if the pane wrote back
        // unconditionally, merely opening it would delete the bad line from disk — taking the evidence
        // of the user's typo with it, instead of showing the warning the pane renders.
        let mut profile = Profile::new("P".to_owned());
        profile.env.insert("BAD KEY".to_owned(), "v".to_owned());
        profile.env.insert("GOOD".to_owned(), "v".to_owned());

        let seeded = ProfileFields::from_profile(&profile).env;
        assert_eq!(seeded, "BAD KEY=v\nGOOD=v", "the editor shows the stored text verbatim");
        let (parsed, rejected) = parse_env(&seeded);
        assert_eq!(rejected, 1, "the pane can tell the user a line was ignored");
        assert_ne!(parsed, profile.env, "so the round trip must not be treated as an identity");
        assert!(!parsed.contains_key("BAD KEY"));

        // Same shape for an unusable port: shown as typed, parsed back to None.
        let mut remote_profile = Profile::new("R".to_owned());
        remote_profile.remote = Some(RemoteTarget {
            host: "h".to_owned(),
            user: None,
            port: Some(2222),
        });
        let fields = ProfileFields::from_profile(&remote_profile);
        assert_eq!(fields.remote_port, "2222", "a valid port does round-trip");
    }

    #[test]
    fn a_well_formed_profile_round_trips_so_no_write_is_triggered() {
        // The complement of the test above: for data the parser accepts, seed → parse must be an exact
        // identity. Otherwise the pane's "only write when it differs" guard would fire on every open
        // anyway, and the guard would be useless.
        let mut profile = Profile::new("Prod".to_owned());
        profile.shell_program = "/bin/bash".to_owned();
        profile.shell_args = vec!["-l".to_owned()];
        profile.cwd = Some(std::path::PathBuf::from("/srv/app"));
        profile.env.insert("AWS_PROFILE".to_owned(), "prod".to_owned());
        profile.remote = Some(RemoteTarget {
            host: "prod.example.com".to_owned(),
            user: Some("deploy".to_owned()),
            port: Some(2222),
        });

        let fields = ProfileFields::from_profile(&profile);
        let (env, rejected) = parse_env(&fields.env);
        assert_eq!(rejected, 0);
        assert_eq!(env, profile.env);
        assert_eq!(parse_args(&fields.shell_args), profile.shell_args);
        assert_eq!(
            parse_remote(&fields.remote_host, &fields.remote_user, &fields.remote_port),
            profile.remote,
        );
        assert_eq!(fields.shell_program.trim(), profile.shell_program);
        assert_eq!(
            (!fields.cwd.trim().is_empty()).then(|| std::path::PathBuf::from(fields.cwd.trim())),
            profile.cwd,
        );
    }

    #[test]
    fn the_seeded_text_guard_blocks_only_the_seed_echo() {
        // Models the settings pane's write-through guard (`on_profile_input_event`), which cannot be
        // driven headlessly because it needs a GPUI `Window`. The property under test is why the guard
        // compares *text* rather than the parsed profile: the seed echo must be blocked even when it
        // parses to something different from what is stored, and a real keystroke must still pass.
        let mut profile = Profile::new("Prod".to_owned());
        profile.env.insert("AWS_PROFILE".to_owned(), "prod".to_owned());
        // The line the parser refuses. Only reachable by hand-editing profiles.json — which is exactly
        // why the pane must not "clean" it silently.
        profile.env.insert("BAD KEY".to_owned(), "keep me".to_owned());

        let seeded = ProfileFields::from_profile(&profile).env;
        // 1. The echo GPUI delivers after seeding. Re-derive the text the way the pane does, rather
        // than comparing a value to its own clone — that would pass against any code at all.
        let echoed = ProfileFields::from_profile(&profile).env;
        assert_eq!(seeded, echoed, "seeding twice yields identical text, so the echo is a no-op");
        // Had the guard compared parsed-vs-stored instead, this is what it would have seen — different,
        // hence a write, hence "BAD KEY" deleted from disk. Verified live before the fix.
        let (parsed, _) = parse_env(&seeded);
        assert_ne!(parsed, profile.env, "parsed-vs-stored would have looked like a real edit");

        // 2. A real keystroke: the text differs from the seed, so the write must go through.
        let typed = format!("{seeded}\nNEW_VAR=1");
        assert_ne!(typed, seeded, "a keystroke changes the text");
        let (typed_env, rejected) = parse_env(&typed);
        assert_eq!(typed_env.get("NEW_VAR").map(String::as_str), Some("1"));
        assert_eq!(rejected, 1, "the bad line is still reported, not silently dropped");
        // The bad line is still lost from the *parsed* result — unavoidable, since the user is now
        // editing this text. What matters is that it took a deliberate edit, not merely opening the pane.
        assert!(!typed_env.contains_key("BAD KEY"));
    }

    #[test]
    fn a_value_containing_a_newline_is_hidden_rather_than_truncated() {
        // A line-oriented editor cannot represent a newline in a *value*: seeding it would produce two
        // lines, so parsing back gave `SCRIPT=line1` plus a misleading "1 line ignored" warning —
        // truncating a valid value and blaming the user for a typo they did not make. Such values are
        // legal POSIX (passed via `execve` locally, single-quoted remotely) and reachable by
        // hand-editing profiles.json, so they are withheld from the editor and preserved instead.
        let mut env = BTreeMap::new();
        env.insert("SCRIPT".to_owned(), "line1\nline2".to_owned());
        env.insert("CRLF".to_owned(), "a\r\nb".to_owned());
        env.insert("Z_OK".to_owned(), "ok".to_owned());

        let text = format_env(&env);
        assert_eq!(text, "Z_OK=ok", "only representable entries are shown");
        let (parsed, rejected) = parse_env(&text);
        assert_eq!(rejected, 0, "no spurious warning: nothing malformed is shown");
        assert_eq!(parsed.len(), 1);

        // The hidden ones are reported separately so the write-through can merge them back.
        let hidden = unrepresentable_env(&env);
        assert_eq!(hidden.len(), 2, "both the LF and CRLF values are withheld");
        assert_eq!(hidden.get("SCRIPT").map(String::as_str), Some("line1\nline2"));
        assert_eq!(hidden.get("CRLF").map(String::as_str), Some("a\r\nb"));
        assert!(!hidden.contains_key("Z_OK"));

        // Merging the two back reconstructs the original exactly, which is what keeps an edit to some
        // *other* variable from deleting them.
        let mut merged = parsed;
        for (key, value) in hidden {
            merged.entry(key).or_insert(value);
        }
        assert_eq!(merged, env, "hide + merge must be lossless");
    }

    #[test]
    fn a_backslash_in_a_value_is_left_alone() {
        // Why hiding beats escaping: any `\n` escape scheme would reinterpret a backslash the user
        // actually typed, turning a Windows-style path into a value containing a real newline. That
        // would trade a bug only hand-editing can reach for one ordinary typing hits.
        let (env, rejected) = parse_env(r"WINPATH=C:\new\table");
        assert_eq!(rejected, 0);
        assert_eq!(env.get("WINPATH").map(String::as_str), Some(r"C:\new\table"));
        // And it survives a round trip untouched.
        assert_eq!(format_env(&env), r"WINPATH=C:\new\table");
    }

    #[test]
    fn a_stale_input_must_not_overwrite_a_field_it_did_not_change() {
        // The pane writes per field, diffing each input against its own seed. That is what stops a
        // *stale* input from reverting another surface's edit, and this models the bug it fixes:
        //
        // Reproduced live before the fix — Advanced → Shell set `shell_program` to `/bin/bash`, then
        // typing one character into the Profiles pane's *Name* field wrote `shell_program: ""` back over
        // it, because that pane's shell input still held the value from when it was seeded (the inputs
        // are seeded once per selection, never on someone else's write).
        let stored = Profile::new("Default".to_owned());
        let seeded = ProfileFields::from_profile(&stored);
        let baseline = [
            seeded.name.clone(),
            seeded.shell_program.clone(),
            seeded.shell_args.clone(),
            seeded.cwd.clone(),
            seeded.env.clone(),
            seeded.remote_host.clone(),
            seeded.remote_user.clone(),
            seeded.remote_port.clone(),
        ];
        assert_eq!(baseline[1], "", "the seeded shell input starts empty");

        // Another surface writes shell_program. The Profiles inputs are NOT re-seeded.
        let mut after_other_surface = stored.clone();
        after_other_surface.shell_program = "/bin/bash".to_owned();

        // The user now types into Name only: one field moves, the other seven still hold their seeds.
        let mut current = baseline.clone();
        current[0] = "Renamed".to_owned();

        // Apply the pane's rule.
        let mut edited = after_other_surface.clone();
        if baseline[0] != current[0] {
            edited.name = current[0].trim().to_owned();
        }
        if baseline[1] != current[1] {
            edited.shell_program = current[1].trim().to_owned();
        }

        assert_eq!(edited.name, "Renamed", "the edited field is applied");
        assert_eq!(
            edited.shell_program, "/bin/bash",
            "an untouched field keeps the other surface's value instead of the stale input's"
        );
        // A whole-struct write would have produced the seeded empty string here — that was the bug.
        assert_ne!(edited.shell_program, baseline[1]);
    }

    #[test]
    fn editing_one_variable_preserves_the_entries_the_editor_cannot_show() {
        // Replicates the pane's env write-through (`on_profile_input_event`, the `changed(4)` branch):
        // parse the visible text, then merge back what was withheld. Without the merge, adding one
        // variable would delete every newline-valued entry the editor never displayed.
        let mut stored = Profile::new("P".to_owned());
        stored.env.insert("VISIBLE".to_owned(), "1".to_owned());
        stored.env.insert("SCRIPT".to_owned(), "line1\nline2".to_owned());

        // The editor shows only VISIBLE, so the user's text starts there.
        let seeded = ProfileFields::from_profile(&stored).env;
        assert_eq!(seeded, "VISIBLE=1", "the newline entry is not shown");

        // The user adds a variable.
        let typed = format!("{seeded}\nNEW_VAR=2");
        let (mut env, rejected) = parse_env(&typed);
        assert_eq!(rejected, 0);
        for (key, value) in unrepresentable_env(&stored.env) {
            env.entry(key).or_insert(value);
        }

        assert_eq!(env.get("NEW_VAR").map(String::as_str), Some("2"), "the edit lands");
        assert_eq!(env.get("VISIBLE").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("SCRIPT").map(String::as_str),
            Some("line1\nline2"),
            "the hidden entry survives an edit to a different variable"
        );
        assert_eq!(env.len(), 3);
    }

    #[test]
    fn a_visible_line_wins_over_a_hidden_entry_with_the_same_name() {
        // If the user types a name that also exists as a hidden entry, the visible line must win:
        // that is the one they can see, and `or_insert` must not resurrect the invisible value.
        let mut stored = Profile::new("P".to_owned());
        stored.env.insert("SCRIPT".to_owned(), "old\nmultiline".to_owned());

        let (mut env, _) = parse_env("SCRIPT=now-single-line");
        for (key, value) in unrepresentable_env(&stored.env) {
            env.entry(key).or_insert(value);
        }
        assert_eq!(
            env.get("SCRIPT").map(String::as_str),
            Some("now-single-line"),
            "what the user typed wins over the withheld value"
        );
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn args_split_on_whitespace() {
        assert_eq!(parse_args("  -l   --login "), vec!["-l", "--login"]);
        assert!(parse_args("   ").is_empty(), "blank means Eggie's default args");
    }

    #[test]
    fn fields_seed_from_a_profile_and_show_unset_values_as_empty() {
        let mut profile = Profile::new("Prod".to_owned());
        profile.shell_args = vec!["-l".to_owned()];
        profile.env.insert("K".to_owned(), "v".to_owned());
        profile.remote = Some(RemoteTarget {
            host: "h".to_owned(),
            user: None,
            port: Some(2222),
        });

        let fields = ProfileFields::from_profile(&profile);
        assert_eq!(fields.name, "Prod");
        assert_eq!(fields.shell_args, "-l");
        assert_eq!(fields.env, "K=v");
        assert_eq!(fields.remote_host, "h");
        // 未设置的值渲染成空字符串,而不是 "None" 之类的占位符 —— 输入框显示的就是可编辑的内容。
        assert_eq!(fields.remote_user, "");
        assert_eq!(fields.remote_port, "2222");
        assert_eq!(fields.cwd, "");
        assert_eq!(fields.shell_program, "");

        // 本地 profile 的三个远程字段全为空。
        let local = ProfileFields::from_profile(&Profile::new("Local".to_owned()));
        assert_eq!(local.remote_host, "");
        assert_eq!(local.remote_port, "");
    }
}

//! Eggie 的内置命令行子系统,对齐 ghostty 的 `ghostty +action` 机制。
//!
//! 用户无需打开 GUI 即可查询终端信息:`eggie +version`、`eggie +help`、
//! `eggie +list-colors`、`eggie +list-themes`,并支持惯用的 `--version`、
//! `-h`/`--help` 别名。这里只做 argv → action 的分派与调度;各命令的具体
//! 实现分散在同目录的子模块里。

mod colors;
mod help;
mod list_colors;
mod list_themes;
mod tui;
mod version;

/// 可用的 CLI action。新增命令时在此追加,并同步维护 `ALL`/`from_plus`/`name`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Help,
    Version,
    ListColors,
    ListThemes,
}

impl Action {
    /// `+help` 里遍历展示的全部 action,顺序即展示顺序。
    const ALL: &'static [Action] = &[
        Action::Help,
        Action::Version,
        Action::ListColors,
        Action::ListThemes,
    ];

    /// 把 `+<name>` 里的 name 解析成 action;未知则 `None`。
    fn from_plus(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }

    /// action 的规范名(即 `+<name>` 里的 name)。
    fn name(self) -> &'static str {
        match self {
            Action::Help => "help",
            Action::Version => "version",
            Action::ListColors => "list-colors",
            Action::ListThemes => "list-themes",
        }
    }

    /// 一句话中文说明,用于 `+help` 的命令列表。
    fn summary(self) -> &'static str {
        match self {
            Action::Help => "显示本帮助信息",
            Action::Version => "显示版本与构建信息",
            Action::ListColors => "列出内置的 X11 命名颜色",
            Action::ListThemes => "列出可用主题(终端下可交互预览)",
        }
    }

    /// 执行该 action,返回进程退出码。`flags` 是 action 之外的剩余参数,
    /// 由各命令自行解析。
    fn run(self, flags: &[String]) -> i32 {
        match self {
            Action::Help => help::run(flags),
            Action::Version => version::run(flags),
            Action::ListColors => list_colors::run(flags),
            Action::ListThemes => list_themes::run(flags),
        }
    }
}

/// argv 解析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Invocation {
    /// 没有任何 CLI action,应当继续走 GUI 启动路径。
    None,
    /// 命中某个 action,附带交给它的剩余参数。
    Run(Action, Vec<String>),
    /// 参数有误。
    Error(CliError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CliError {
    /// 一次指定了多个 `+action`。
    MultipleActions,
    /// 未知的 `+action`。
    InvalidAction(String),
}

/// CLI 入口。在 daemon 自调用检测之后、GUI 启动之前调用。
///
/// 返回 `None` 表示没有 CLI action(继续走 GUI);`Some(code)` 表示 CLI 已经
/// 处理(含出错),调用方应以该退出码结束进程。
pub(crate) fn try_run_cli(arguments: &[String]) -> Option<i32> {
    match parse_invocation(arguments) {
        Invocation::None => None,
        Invocation::Run(action, flags) => Some(action.run(&flags)),
        Invocation::Error(error) => {
            match error {
                CliError::MultipleActions => {
                    eprintln!("错误:一次只能指定一个 +action。");
                }
                CliError::InvalidAction(name) => {
                    eprintln!("错误:未知的 action `+{name}`。运行 `eggie +help` 查看可用命令。");
                }
            }
            Some(1)
        }
    }
}

/// 纯函数式的 argv 解析,便于单测。语义对齐 ghostty 的 detect + special-case:
///
/// - `+<name>`:命中记为待执行 action(重复 → `MultipleActions`,未知 → `InvalidAction`)。
/// - `--version`:最高优先级,强制 Version(忽略其余)。
/// - `--help` / `-h`:记为兜底 Help,仅在没有其它 action 时生效。
/// - 其余参数:原样收集,作为 flags 传给命中的 action 自行解析。
///
/// 裁决顺序:`--version` > 命中的 `+action` > 兜底 Help > None。
/// 因此 `eggie +list-themes --help` → ListThemes(由该命令展示自身帮助),
/// 而单独的 `eggie -h` → Help。
fn parse_invocation(arguments: &[String]) -> Invocation {
    let mut pending: Option<Action> = None;
    let mut force_version = false;
    let mut fallback_help = false;
    let mut flags: Vec<String> = Vec::new();

    for arg in arguments.iter().skip(1) {
        if let Some(name) = arg.strip_prefix('+') {
            match Action::from_plus(name) {
                Some(action) => {
                    if pending.is_some() {
                        return Invocation::Error(CliError::MultipleActions);
                    }
                    pending = Some(action);
                }
                None => {
                    return Invocation::Error(CliError::InvalidAction(name.to_owned()));
                }
            }
        } else if arg == "--version" {
            force_version = true;
        } else if arg == "--help" || arg == "-h" {
            // 既作无-action 时的兜底 Help,又原样透传给命中的 action —— 这样
            // `+list-themes --help` 能让 ListThemes 自己展示帮助。
            fallback_help = true;
            flags.push(arg.clone());
        } else {
            flags.push(arg.clone());
        }
    }

    if force_version {
        return Invocation::Run(Action::Version, flags);
    }
    if let Some(action) = pending {
        return Invocation::Run(action, flags);
    }
    if fallback_help {
        return Invocation::Run(Action::Help, flags);
    }
    Invocation::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        // argv[0] 是程序名,解析从 [1..] 开始。
        std::iter::once("eggie")
            .chain(parts.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn plus_actions_map_to_their_action() {
        assert_eq!(
            parse_invocation(&argv(&["+help"])),
            Invocation::Run(Action::Help, vec![])
        );
        assert_eq!(
            parse_invocation(&argv(&["+version"])),
            Invocation::Run(Action::Version, vec![])
        );
        assert_eq!(
            parse_invocation(&argv(&["+list-colors"])),
            Invocation::Run(Action::ListColors, vec![])
        );
        assert_eq!(
            parse_invocation(&argv(&["+list-themes"])),
            Invocation::Run(Action::ListThemes, vec![])
        );
    }

    #[test]
    fn version_and_help_aliases() {
        assert_eq!(
            parse_invocation(&argv(&["--version"])),
            Invocation::Run(Action::Version, vec![])
        );
        assert_eq!(
            parse_invocation(&argv(&["-h"])),
            Invocation::Run(Action::Help, vec!["-h".to_owned()])
        );
        assert_eq!(
            parse_invocation(&argv(&["--help"])),
            Invocation::Run(Action::Help, vec!["--help".to_owned()])
        );
    }

    #[test]
    fn version_flag_outranks_other_actions() {
        // --version 最高优先,即便同时给了别的 action。
        assert_eq!(
            parse_invocation(&argv(&["+list-themes", "--version"])),
            Invocation::Run(Action::Version, vec![])
        );
    }

    #[test]
    fn action_specific_help_stays_with_the_action() {
        // +list-themes --help 归 ListThemes(由命令自己展示帮助),--help 只作兜底。
        assert_eq!(
            parse_invocation(&argv(&["+list-themes", "--help"])),
            Invocation::Run(Action::ListThemes, vec!["--help".to_owned()])
        );
    }

    #[test]
    fn extra_flags_pass_through_to_the_action() {
        assert_eq!(
            parse_invocation(&argv(&["+list-themes", "--dark"])),
            Invocation::Run(Action::ListThemes, vec!["--dark".to_owned()])
        );
    }

    #[test]
    fn multiple_actions_is_an_error() {
        assert_eq!(
            parse_invocation(&argv(&["+help", "+version"])),
            Invocation::Error(CliError::MultipleActions)
        );
    }

    #[test]
    fn unknown_action_is_an_error() {
        assert_eq!(
            parse_invocation(&argv(&["+bogus"])),
            Invocation::Error(CliError::InvalidAction("bogus".to_owned()))
        );
    }

    #[test]
    fn no_action_returns_none() {
        assert_eq!(parse_invocation(&argv(&[])), Invocation::None);
        // 裸参数(不带 + 也非已知别名)不构成 action。
        assert_eq!(parse_invocation(&argv(&["foo", "bar"])), Invocation::None);
    }

    #[test]
    fn from_plus_and_name_round_trip() {
        for action in Action::ALL.iter().copied() {
            assert_eq!(Action::from_plus(action.name()), Some(action));
        }
        assert_eq!(Action::from_plus("nope"), None);
    }
}

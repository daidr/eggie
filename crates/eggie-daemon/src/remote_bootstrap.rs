//! 远程 shell 集成的自举(bootstrap)。
//!
//! 一个带 [`RemoteTarget`](eggie_domain::RemoteTarget) 的 profile 打开的不是本地 shell,而是 `ssh`。
//! 要让远程会话也有 OSC 133/OSC 7 集成,集成脚本必须先到达远端 —— 而这里最难的不是传文件,是**认证**:
//! 走 `scp`/`sftp` 再开一条 `ssh` 意味着两次连接、两次密码/2FA。
//!
//! 所以本模块把安装脚本 base64 编码后**内联进同一条交互式 ssh 的远程命令**:
//!
//! ```text
//! ssh -t host 'eval "$(printf %s <BASE64> | base64 -d)"'
//! ```
//!
//! 远程命令先幂等地安装(版本相同则秒过),再 `exec` 用户真正的 shell。一条连接、一次认证、
//! 原子完成;安装失败则直接 `exec` shell 降级为无集成的普通会话 —— ssh 本身能用是底线。
//!
//! 自举脚本必须是 **POSIX sh**:远端的登录 shell 未知(可能是 zsh/bash/fish/ksh),而 `ssh` 执行
//! 远程命令时用的是用户的登录 shell,所以脚本只能用所有它们都认的语法。fish 不认 POSIX 的
//! `$(...)`/`export`,因此外层命令刻意只用 `sh -c` 包一层(见 [`remote_command`])。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use eggie_domain::RemoteTarget;

/// 远端安装目录(相对 `$HOME`)。带版本号,便于将来换机制时新旧并存、老目录可留待清理。
const REMOTE_ROOT: &str = ".eggie/shell-integration-v1";

/// 写在远端的版本标记。内容一致时自举脚本跳过全部写入 —— 这是幂等性的判据,
/// 也让每次连接的开销降到一次 `cat` 比较。
const VERSION_MARKER: &str = "version";

/// Upper bound on a ControlPath, just under the 104-byte `sun_path` limit. ssh rejects a path that
/// long outright, so a longer home directory disables multiplexing rather than breaking the session.
const CONTROL_PATH_MAX: usize = 100;

/// Construct the ControlPath for a session's ssh master: `$HOME/.eggie/ctl/<short id>`.
///
/// The short id is the first 16 hex chars of the session UUID, so the path is 44 bytes for a typical
/// home directory — well under the limit. Returns `None` when the home directory would push the path
/// over the limit, so the caller can launch without multiplexing instead of letting ssh reject the
/// connection.
pub(crate) fn control_path(home: &Path, short_id: &str) -> Option<PathBuf> {
    let path = home.join(".eggie").join("ctl").join(short_id);
    (path.as_os_str().len() < CONTROL_PATH_MAX).then_some(path)
}

/// 一次远程会话所需的全部信息。
pub(crate) struct RemoteLaunch {
    /// 传给 `ssh` 的完整参数(不含 `ssh` 本身)。
    pub args: Vec<String>,
    /// 要执行的程序 —— 恒为 `ssh`,单独给出以便调用方直接交给 PTY。
    pub program: String,
}

/// 构造远程会话的 `ssh` 调用。
///
/// - `-t` 强制分配 tty:没有它,远端 shell 不是交互式的,OSC 序列与作业控制都不会工作。
/// - `install` 为 `false` 时不注入自举(用户拒绝在该主机安装),只 `exec` 远端 shell;
///   会话照常可用,只是没有集成。
/// - `env` 是 profile 的环境变量。**不走 `ssh -o SendEnv`**:那要求远端 `sshd` 的
///   `AcceptEnv` 放行对应变量,默认几乎都不放行,静默丢失。改为在自举脚本里 `export`,
///   对任何 sshd 配置都有效。
pub(crate) fn remote_launch(
    target: &RemoteTarget,
    scripts: &RemoteScripts,
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
    install: bool,
    control_path: Option<&Path>,
) -> RemoteLaunch {
    let mut args = vec!["-t".to_owned()];
    if let Some(port) = target.port {
        args.push("-p".to_owned());
        args.push(port.to_string());
    }
    // ControlMaster multiplexes later commands (tunnels, process checks) over this one authenticated
    // connection. The options go after -t/-p but before --, so the -- guard still sits immediately
    // before the destination.
    if let Some(path) = control_path {
        args.push("-o".to_owned());
        args.push("ControlMaster=auto".to_owned());
        args.push("-o".to_owned());
        args.push(format!("ControlPath={}", path.display()));
    }
    // `--` before the destination is a hard requirement, not tidiness: without it a host string
    // beginning with `-` is parsed by ssh as an OPTION. `-oProxyCommand=<cmd>` in particular makes ssh
    // run `<cmd>` on the *local* machine at connect time — arbitrary local code execution from a
    // profile field, before any consent prompt could apply. Verified against the real ssh binary.
    args.push("--".to_owned());
    args.push(target.destination());
    args.push(remote_command(scripts, env, cwd, install));
    RemoteLaunch {
        args,
        program: "ssh".to_owned(),
    }
}

/// 要在远端执行的单条命令。
///
/// 外层固定是 `sh -c '...'`:`ssh` 会用**用户的登录 shell**执行远程命令,而自举脚本是 POSIX sh。
/// 若远端登录 shell 是 fish,`$(...)`、`export VAR=x` 这些都会语法错误 —— 而 fish 用户恰恰是
/// 最需要这套集成的人群之一。`sh -c` 把语法方言钉死,代价只是多一层进程。
fn remote_command(
    scripts: &RemoteScripts,
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
    install: bool,
) -> String {
    let payload = if install {
        bootstrap_script(scripts, env, cwd)
    } else {
        // 不安装:仍然要处理 env 与 cwd,并 exec 用户的 shell。
        degraded_script(env, cwd)
    };
    let encoded = BASE64.encode(payload.as_bytes());
    // `printf %s` 而非 `echo`:后者对反斜杠的处理各家不同。base64 字母表里没有 shell 元字符,
    // 所以这里的单引号包裹是安全的(不存在需要转义的引号)。
    //
    // 兜底本身也不能写成 `exec A || exec B`:exec 失败会直接终止 shell,不会走到 `||`(实测
    // `sh -c 'exec /nonexistent || echo x'` 以 126 退出且无输出)。所以先 `[ -x ]` 验证再 exec,
    // 最后无条件落到 `/bin/sh`。
    //
    // 兜底的 `exec` 必须在 eval **之外**:极简远端(busybox / 精简容器 / 老 Solaris)可能没有 `base64`
    // 或不认 `-d`,此时命令替换结果为空,`eval ""` 静默无操作 —— 若兜底写在 payload 里,它会跟着一起
    // 消失,会话直接以 0 退出、看起来像干净断连(实测)。放在外面则退化为一个无集成的普通 shell,
    // 也就是本模块承诺的底线。eval 成功时脚本自己会 exec,永远走不到这一行。
    format!(
        "sh -c 'eval \"$(printf %s {encoded} | base64 -d 2>/dev/null)\"; \
         if [ -x \"${{SHELL:-}}\" ]; then exec \"$SHELL\" -l; fi; exec /bin/sh -l'"
    )
}

/// 要安装到远端的脚本内容。由 daemon 用它已内嵌的字节填充,避免本模块重复 `include_bytes!`。
pub(crate) struct RemoteScripts {
    pub zsh_zshenv: &'static [u8],
    pub zsh_integration: &'static [u8],
    pub bash: &'static [u8],
    pub fish: &'static [u8],
    /// 版本标记内容;与远端已存在的一致时自举跳过写入。
    pub version: String,
}

/// 生成自举脚本(POSIX sh)。
///
/// 执行顺序刻意如下:
/// 1. 先算出 `$SHELL`(下面每条失败路径都要用它 `exec` 降级);
/// 2. 版本标记一致则跳过安装(常见路径,只花一次 `cat`);
/// 3. 否则写入脚本 —— 任一步失败即放弃安装,但**不放弃会话**;
/// 4. 按远端 shell 选注入机制,设置对应环境变量;
/// 5. `export` profile 的环境变量、`cd` 到 profile 的目录;
/// 6. `exec` 远端 shell。
///
/// 每一步都不用 `set -e`:那会让任何一个小失败直接杀掉会话。改为逐步判断,失败即降级。
fn bootstrap_script(
    scripts: &RemoteScripts,
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
) -> String {
    let mut script = String::new();
    script.push_str(&format!(
        r#"
# --- Eggie remote shell integration bootstrap (POSIX sh) ---
# Resolve the login shell first: every failure path below execs it to degrade gracefully.
_eggie_shell="${{SHELL:-/bin/sh}}"
_eggie_root="$HOME/{REMOTE_ROOT}"
_eggie_version='{version}'

# Idempotence: identical version marker means the scripts are already in place.
_eggie_installed=0
if [ -r "$_eggie_root/{VERSION_MARKER}" ] \
    && [ "$(cat "$_eggie_root/{VERSION_MARKER}" 2>/dev/null)" = "$_eggie_version" ]; then
    _eggie_installed=1
fi
"#,
        version = scripts.version,
    ));

    // 写入部分:每个文件用 base64 传递,避免在 heredoc 里为引号/反斜杠/UTF-8 做转义。
    script.push_str(&format!(
        r#"
if [ "$_eggie_installed" -eq 0 ]; then
    if mkdir -p "$_eggie_root/zsh" "$_eggie_root/bash" "$_eggie_root/fish/vendor_conf.d" 2>/dev/null; then
        chmod 700 "$_eggie_root" 2>/dev/null
        _eggie_write() {{
            # $1 = destination, $2 = base64 payload. Write to a temp file then move, so a
            # half-written script is never sourced by a concurrently starting shell.
            #
            # The temp name carries $$ because it is NOT per-connection otherwise: two Eggie builds
            # (a debug and a release, or one mid-upgrade) connecting to the same host write different
            # bytes through the same fixed path, and the interleaving produces a genuinely torn file
            # that every later zsh login then sources. Same-version races are harmless either way.
            printf %s "$2" | base64 -d > "$1.$$.tmp" 2>/dev/null \
                && mv -f "$1.$$.tmp" "$1" 2>/dev/null
        }}
        if _eggie_write "$_eggie_root/zsh/.zshenv" '{zshenv}' \
            && _eggie_write "$_eggie_root/zsh/eggie-integration" '{zsh_integration}' \
            && _eggie_write "$_eggie_root/bash/eggie.bash" '{bash}' \
            && _eggie_write "$_eggie_root/fish/vendor_conf.d/eggie.fish" '{fish}'; then
            # The marker is written LAST: if any script write failed, the marker is absent and the
            # next connection retries rather than trusting a half-installed tree.
            printf '%s\n' "$_eggie_version" > "$_eggie_root/{VERSION_MARKER}" 2>/dev/null \
                && _eggie_installed=1
        fi
    fi
fi
"#,
        zshenv = BASE64.encode(scripts.zsh_zshenv),
        zsh_integration = BASE64.encode(scripts.zsh_integration),
        bash = BASE64.encode(scripts.bash),
        fish = BASE64.encode(scripts.fish),
    ));

    // 注入部分:按远端 shell 的 basename 选机制,与本地 `build_shell_env` 的分发保持一致。
    script.push_str(
        r#"
if [ "$_eggie_installed" -eq 1 ]; then
    _eggie_name="${_eggie_shell##*/}"
    # Login-shell flag, overridden per shell where integration needs a different mode.
    _eggie_flag=-l
    # Mirrors the local dispatch in `build_shell_env`: same mechanism per shell, same variables.
    case "$_eggie_name" in
        zsh)
            # Preserve the user's own ZDOTDIR so our .zshenv can chain to it, exactly as locally.
            if [ -n "${ZDOTDIR+x}" ]; then
                EGGIE_ZDOTDIR_ORIG="$ZDOTDIR"
                export EGGIE_ZDOTDIR_ORIG
            fi
            ZDOTDIR="$_eggie_root/zsh"
            export ZDOTDIR
            ;;
        bash)
            # bash reads $ENV *only in POSIX mode*, and `-l --posix` does not count — measured:
            # `--posix` alone sources it, `-l`, `-l --posix` and `-i` do not. So the exec flag has to
            # change for bash, exactly as the local side forces `--posix` instead of `-l`. Without
            # this the whole bash branch is dead weight: the variables are set and never read.
            if [ -n "${ENV+x}" ]; then
                EGGIE_BASH_ENV="$ENV"
                export EGGIE_BASH_ENV
            fi
            ENV="$_eggie_root/bash/eggie.bash"
            export ENV
            EGGIE_BASH_INJECT=1
            export EGGIE_BASH_INJECT
            _eggie_flag=--posix
            ;;
        fish)
            # fish sources vendor_conf.d from every XDG_DATA_DIRS entry.
            if [ -n "${XDG_DATA_DIRS+x}" ]; then
                XDG_DATA_DIRS="$_eggie_root:$XDG_DATA_DIRS"
            else
                XDG_DATA_DIRS="$_eggie_root:/usr/local/share:/usr/share"
            fi
            export XDG_DATA_DIRS
            ;;
    esac
    # Terminal identity, matching the local environment so the same integration branches are taken.
    EGGIE_SHELL_FEATURES=''
    export EGGIE_SHELL_FEATURES
fi
"#,
    );

    script.push_str(&launch_tail(env, cwd));
    script
}

/// 不安装集成时的脚本:只处理 profile 的 env/cwd,然后 `exec` 远端 shell。
fn degraded_script(env: &BTreeMap<String, String>, cwd: Option<&str>) -> String {
    let mut script = String::from(
        r#"
# --- Eggie remote session (integration not installed) ---
_eggie_shell="${SHELL:-/bin/sh}"
"#,
    );
    script.push_str(&launch_tail(env, cwd));
    script
}

/// 两条路径共用的尾段:导出 profile 环境变量、切目录、exec shell。
///
/// `env` 走这里而不是 `ssh -o SendEnv`,因为后者需要远端 sshd 的 `AcceptEnv` 放行,默认不放行。
/// 变量名已在客户端校验过(见 `is_valid_env_key`),值用单引号包裹并转义内部单引号。
fn launch_tail(env: &BTreeMap<String, String>, cwd: Option<&str>) -> String {
    let mut script = String::new();
    for (key, value) in env {
        // 跳过 Eggie 自己拥有的变量:与本地一侧同样的规则,防止 profile 破坏集成。
        if is_eggie_owned(key) {
            continue;
        }
        // Key 不能像 value 那样靠加引号救 —— 它出现在赋值的**左侧**,不是可加引号的位置。所以只能
        // 拒绝:含换行、`;`、`$(...)` 的 key 会直接变成脚本里的一行命令(实测可执行任意命令)。
        // daemon 侧独立校验,不信任客户端已经过滤过。
        if !is_shell_name(key) {
            eprintln!("remote bootstrap: refusing environment variable name {key:?}");
            continue;
        }
        script.push_str(&format!(
            "{key}={value}\nexport {key}\n",
            value = single_quote(value)
        ));
    }
    if let Some(cwd) = cwd {
        // `cd` 失败不算致命:远端目录可能已不存在,那就在 home 起 shell。
        script.push_str(&format!("cd {} 2>/dev/null || true\n", single_quote(cwd)));
    }
    // `exec` 失败会**直接杀掉 shell**,不会走到 `||` 分支 —— 实测 `sh -c 'exec /nonexistent || echo x'`
    // 以 126 退出且不打印任何东西。所以不能用 `exec A || exec B` 做兜底:必须在 exec 之前先验证。
    //
    // 顺序:$SHELL 可执行则带 `-l` 登录式启动(与本地默认参数一致);否则回落到 `sh`。最后一行的
    // `exec /bin/sh` 是绝对底线 —— 走到那里说明 $SHELL 不可用,但会话仍然给用户一个可用的 shell。
    script.push_str(
        r#"# Clean up our own scratch variables so they do not leak into the user's shell.
unset _eggie_installed _eggie_name _eggie_root _eggie_version 2>/dev/null
if [ -x "$_eggie_shell" ]; then
    _eggie_exec="$_eggie_shell"
    _eggie_arg="${_eggie_flag:--l}"
    unset _eggie_shell _eggie_flag
    exec "$_eggie_exec" "$_eggie_arg"
fi
unset _eggie_shell _eggie_flag
exec /bin/sh -l
"#,
    );
    script
}

/// 与 daemon 本地一侧同名判断保持一致:Eggie 拥有的变量不可由 profile 设置。
///
/// 刻意重复一份而不跨模块复用:本地版还包含 `TERM`/`TERMINFO` 等只对本地 PTY 有意义的项,
/// 而远端由 ssh 自己协商 TERM。两处的共同点是 `EGGIE_*` 前缀,那才是安全边界。
fn is_eggie_owned(key: &str) -> bool {
    key.starts_with("EGGIE_")
        || key.starts_with("_eggie")
        || matches!(key, "ZDOTDIR" | "ENV" | "XDG_DATA_DIRS")
}

/// 一个字符串是否是合法的 shell 变量名。
///
/// 转发到 [`eggie_domain::is_valid_env_key`] —— 规则的唯一真源在那里,因为 profile 编辑器、客户端的
/// spawn 配置、这里的远程 bootstrap 三处都需要**同一条**规则,而正是这条规则在各处分头维护时漂移,
/// 才让 bootstrap 一度可注入。daemon 是独立进程,仍然自己校验一次(不信任客户端已过滤),只是不再
/// 自己定义规则。
fn is_shell_name(key: &str) -> bool {
    eggie_domain::is_valid_env_key(key)
}

/// 把任意字符串包成 POSIX sh 的单引号字面量。
///
/// 单引号内除 `'` 本身外一切都是字面量,所以只需把每个 `'` 换成 `'\''`(闭合、转义、重开)。
/// 这对换行、`$`、反引号、UTF-8 都成立。
fn single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scripts() -> RemoteScripts {
        RemoteScripts {
            zsh_zshenv: b"# zshenv\n",
            zsh_integration: b"# integration\n",
            bash: b"# bash\n",
            fish: b"# fish\n",
            version: "1.2.3".to_owned(),
        }
    }

    fn target() -> RemoteTarget {
        RemoteTarget::new("host.example.com".to_owned())
    }

    #[test]
    fn ssh_invocation_forces_a_tty_and_carries_the_destination() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), None, true, None);
        assert_eq!(launch.program, "ssh");
        assert_eq!(launch.args[0], "-t", "without -t the remote shell is not interactive");
        assert!(launch.args.contains(&"host.example.com".to_owned()));
    }

    #[test]
    fn a_control_path_sits_under_the_home_directory() {
        let path = control_path(Path::new("/Users/alice"), "0123456789abcdef")
            .expect("a typical home directory is well under the limit");
        assert_eq!(
            path,
            PathBuf::from("/Users/alice/.eggie/ctl/0123456789abcdef")
        );
    }

    #[test]
    fn a_long_home_directory_disables_multiplexing_instead_of_breaking_the_session() {
        // A home directory that would push the ControlPath over the sun_path limit must return None,
        // so the caller launches without multiplexing rather than letting ssh reject the connection.
        let long_home = format!("/very/long/home/{}", "x".repeat(80));
        assert!(
            control_path(Path::new(&long_home), "0123456789abcdef").is_none(),
            "a too-long ControlPath must disable multiplexing, not break the session"
        );
    }

    #[test]
    fn control_master_options_go_before_the_double_dash_guard() {
        let path = PathBuf::from("/tmp/eggie-ctl/0123456789abcdef");
        let launch = remote_launch(
            &target(),
            &scripts(),
            &BTreeMap::new(),
            None,
            true,
            Some(&path),
        );
        // The options must sit after -t but before --, so the -- guard still immediately precedes
        // the destination.
        let dash_index = launch
            .args
            .iter()
            .position(|arg| arg == "--")
            .expect("the -- guard must be present");
        assert!(dash_index > 2, "ControlMaster options go before --");
        assert!(launch.args.contains(&"ControlMaster=auto".to_owned()));
        assert!(launch
            .args
            .iter()
            .any(|arg| arg.starts_with("ControlPath=")));
    }

    #[test]
    fn the_destination_can_never_be_parsed_as_an_ssh_option() {
        // Without a `--` guard, ssh reads a leading-dash host as an option, and
        // `-oProxyCommand=<cmd>` runs <cmd> on the LOCAL machine at connect time — verified against
        // the real ssh binary. The guard must sit immediately before the destination.
        let hostile = RemoteTarget::new("-oProxyCommand=touch /tmp/pwned".to_owned());
        let launch = remote_launch(&hostile, &scripts(), &BTreeMap::new(), None, true, None);
        let guard = launch
            .args
            .iter()
            .position(|arg| arg == "--")
            .expect("the destination must be guarded by `--`");
        let destination = launch
            .args
            .iter()
            .position(|arg| arg == &hostile.destination())
            .expect("the destination is present");
        assert!(
            guard < destination,
            "`--` must precede the destination, otherwise ssh parses it as an option: {:?}",
            launch.args
        );
        // The guard also has to come after the real options, or `-p` would be swallowed.
        let target = RemoteTarget {
            host: "host".to_owned(),
            user: None,
            port: Some(2222),
        };
        let launch = remote_launch(&target, &scripts(), &BTreeMap::new(), None, true, None);
        let port = launch.args.iter().position(|arg| arg == "2222").unwrap();
        let guard = launch.args.iter().position(|arg| arg == "--").unwrap();
        assert!(port < guard, "real options must precede `--`: {:?}", launch.args);
    }

    #[test]
    fn a_port_and_user_reach_the_ssh_arguments() {
        let target = RemoteTarget {
            host: "host.example.com".to_owned(),
            user: Some("deploy".to_owned()),
            port: Some(2222),
        };
        let launch = remote_launch(&target, &scripts(), &BTreeMap::new(), None, true, None);
        let joined = launch.args.join(" ");
        assert!(joined.contains("-p 2222"), "port must be passed: {joined}");
        assert!(
            launch.args.contains(&"deploy@host.example.com".to_owned()),
            "user@host must be the destination: {joined}"
        );
    }

    #[test]
    fn the_remote_command_is_wrapped_in_sh_so_a_fish_login_shell_can_run_it() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), None, true, None);
        let command = launch.args.last().expect("a remote command");
        assert!(
            command.starts_with("sh -c '"),
            "ssh runs the remote command with the user's login shell, which may be fish: {command}"
        );
        // The payload must be pure base64 inside the quotes — no shell metacharacters that would
        // need escaping, which is what makes the single-quote wrapping safe.
        let inner = command
            .trim_start_matches("sh -c '")
            .trim_end_matches('\'');
        assert!(!inner.contains('\''), "the wrapped command must not contain a quote: {inner}");
    }

    #[test]
    fn the_payload_decodes_to_a_script_that_execs_the_login_shell() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), None, true, None);
        let script = decoded_payload(&launch);
        // Every path must end at a shell, so a failed install still yields a session.
        assert!(script.contains("_eggie_shell=\"${SHELL:-/bin/sh}\""));
        assert!(script.contains(r#"exec "$_eggie_exec" "$_eggie_arg""#), "{script}");
        // `exec A || exec B` does NOT work — a failed exec kills the shell without reaching the
        // fallback (verified: `sh -c 'exec /nonexistent || echo x'` exits 126 and prints nothing).
        // So the shell must be validated *before* exec, with an unconditional last resort after it.
        assert!(
            script.contains(r#"if [ -x "$_eggie_shell" ]"#),
            "the shell must be checked before exec, not after: {script}"
        );
        assert!(
            script.contains("exec /bin/sh -l"),
            "an unusable $SHELL must still land the user in a shell: {script}"
        );
        assert!(
            !script.contains("|| exec"),
            "a failed exec never reaches an `|| exec` fallback"
        );
        // Our scratch variables must not leak into the user's shell.
        assert!(script.contains("unset _eggie_installed"), "{script}");
    }

    #[test]
    fn the_install_is_skipped_when_the_version_marker_matches() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), None, true, None);
        let script = decoded_payload(&launch);
        assert!(script.contains("_eggie_version='1.2.3'"));
        assert!(
            script.contains(&format!("$_eggie_root/{VERSION_MARKER}")),
            "the marker is the idempotence check"
        );
        // The marker is written last, so a partial install is retried rather than trusted.
        let marker_write = script
            .find(r#"printf '%s\n' "$_eggie_version""#)
            .expect("marker write");
        let fish_write = script.find("fish/vendor_conf.d/eggie.fish").expect("fish write");
        assert!(
            marker_write > fish_write,
            "the version marker must be written after the scripts it vouches for"
        );
    }

    #[test]
    fn every_bundled_script_is_carried_in_the_payload() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), None, true, None);
        let script = decoded_payload(&launch);
        for (label, contents) in [
            ("zshenv", b"# zshenv\n".as_slice()),
            ("integration", b"# integration\n".as_slice()),
            ("bash", b"# bash\n".as_slice()),
            ("fish", b"# fish\n".as_slice()),
        ] {
            assert!(
                script.contains(&BASE64.encode(contents)),
                "{label} must be embedded in the bootstrap"
            );
        }
    }

    #[test]
    fn the_remote_shell_dispatch_mirrors_the_local_one() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), None, true, None);
        let script = decoded_payload(&launch);
        // Same mechanism per shell as `build_shell_env`, so remote behaves like local.
        assert!(script.contains("ZDOTDIR=\"$_eggie_root/zsh\""));
        assert!(script.contains("EGGIE_ZDOTDIR_ORIG=\"$ZDOTDIR\""), "must chain the user's ZDOTDIR");
        assert!(script.contains("ENV=\"$_eggie_root/bash/eggie.bash\""));
        assert!(script.contains("EGGIE_BASH_INJECT=1"));
        assert!(script.contains("XDG_DATA_DIRS=\"$_eggie_root:"));
        // bash needs `--posix`, not `-l`: it reads $ENV only in POSIX mode, and `-l --posix` does not
        // count either (measured). Without the flag swap the ENV hook above is never read and the
        // whole bash branch is dead weight — the same reason the local side forces `--posix`.
        assert!(script.contains("_eggie_flag=--posix"), "{script}");
        assert!(
            script.contains("_eggie_flag=-l"),
            "the other shells must still start as login shells"
        );
        assert!(
            script.contains(r#"_eggie_arg="${_eggie_flag:--l}""#),
            "the exec flag must default to -l when no branch set one"
        );
    }

    #[test]
    fn profile_env_is_exported_and_quoted_safely() {
        let env = BTreeMap::from([
            ("AWS_PROFILE".to_owned(), "prod".to_owned()),
            // Values that would break naive interpolation.
            ("QUOTED".to_owned(), "it's \"quoted\"".to_owned()),
            ("DOLLAR".to_owned(), "$HOME `whoami`".to_owned()),
            ("NEWLINE".to_owned(), "a\nb".to_owned()),
        ]);
        let launch = remote_launch(&target(), &scripts(), &env, None, true, None);
        let script = decoded_payload(&launch);
        assert!(script.contains("AWS_PROFILE='prod'\nexport AWS_PROFILE"));
        // A literal `$` and backtick must survive as text, not be expanded by the remote shell.
        assert!(script.contains(r"DOLLAR='$HOME `whoami`'"), "{script}");
        // An embedded single quote is closed, escaped and reopened.
        assert!(script.contains(r#"QUOTED='it'\''s "quoted"'"#), "{script}");
        assert!(script.contains("NEWLINE='a\nb'"));
    }

    #[test]
    fn a_hostile_env_key_cannot_become_a_command() {
        // The key sits on the left of `NAME=value`, which is not a quotable position — so these must
        // be refused outright. Each of them executes arbitrary commands on the remote host otherwise,
        // on BOTH the consented and declined paths (the env section is shared).
        let hostile = [
            "X\nid > /tmp/pwned\n",
            "X;id > /tmp/pwned;Y",
            "X $(id > /tmp/pwned)",
            "X`id`",
            "X&whoami",
            "X|whoami",
            "X>out",
            "1LEADING_DIGIT",
            "has-dash",
            "has space",
            "",
        ];
        for key in hostile {
            let env = BTreeMap::from([
                (key.to_owned(), "v".to_owned()),
                ("KEEP".to_owned(), "yes".to_owned()),
            ]);
            for install in [true, false] {
                let launch = remote_launch(&target(), &scripts(), &env, None, install, None);
                let script = decoded_payload(&launch);
                assert!(
                    !script.contains("/tmp/pwned") && !script.contains("whoami"),
                    "key {key:?} (install={install}) leaked into the script:\n{script}"
                );
                // A refused key must not take the well-formed ones down with it.
                assert!(script.contains("KEEP='yes'"), "key {key:?} dropped a valid sibling");
            }
        }
        // Well-formed names still pass, including the underscore edge cases.
        for key in ["A", "_", "_x9", "AWS_PROFILE", "a1_B2"] {
            let env = BTreeMap::from([(key.to_owned(), "v".to_owned())]);
            let launch = remote_launch(&target(), &scripts(), &env, None, true, None);
            assert!(
                decoded_payload(&launch).contains(&format!("{key}='v'")),
                "valid key {key:?} must be exported"
            );
        }
    }

    #[test]
    fn profile_env_cannot_hijack_the_remote_integration() {
        // The same guarantee the local side enforces: a profile must not be able to redirect the
        // remote ZDOTDIR (whose script gets sourced) or forge an EGGIE_* variable.
        let env = BTreeMap::from([
            ("EGGIE_ZDOTDIR_ORIG".to_owned(), "/tmp/hijack".to_owned()),
            ("ZDOTDIR".to_owned(), "/tmp/hijack".to_owned()),
            ("ENV".to_owned(), "/tmp/hijack".to_owned()),
            ("XDG_DATA_DIRS".to_owned(), "/tmp/hijack".to_owned()),
            ("KEEP".to_owned(), "yes".to_owned()),
        ]);
        let launch = remote_launch(&target(), &scripts(), &env, None, true, None);
        let script = decoded_payload(&launch);
        assert!(!script.contains("/tmp/hijack"), "no profile value may reach an owned variable");
        assert!(script.contains("KEEP='yes'"), "unowned variables still pass through");
    }

    #[test]
    fn a_profile_cwd_is_a_remote_path_and_failure_is_not_fatal() {
        let launch = remote_launch(&target(), &scripts(), &BTreeMap::new(), Some("/srv/www"), true, None);
        let script = decoded_payload(&launch);
        assert!(script.contains("cd '/srv/www' 2>/dev/null || true"));
        // A path with a quote must not break out of the cd.
        let launch = remote_launch(
            &target(),
            &scripts(),
            &BTreeMap::new(),
            Some("/srv/it's here"),
            true,
            None,
        );
        assert!(decoded_payload(&launch).contains(r#"cd '/srv/it'\''s here'"#));
    }

    #[test]
    fn declining_the_install_still_produces_a_usable_session() {
        let env = BTreeMap::from([("AWS_PROFILE".to_owned(), "prod".to_owned())]);
        let launch = remote_launch(&target(), &scripts(), &env, Some("/srv"), false, None);
        let script = decoded_payload(&launch);
        // Nothing is written to the remote host: assert the absence of the encoded script bytes
        // themselves, not merely that the payload got smaller.
        assert!(!script.contains("mkdir -p"), "a declined install must not create directories");
        for contents in [
            b"# zshenv\n".as_slice(),
            b"# integration\n".as_slice(),
            b"# bash\n".as_slice(),
            b"# fish\n".as_slice(),
        ] {
            assert!(!script.contains(&BASE64.encode(contents)));
        }
        // But the profile's environment, directory and shell still apply.
        assert!(script.contains("AWS_PROFILE='prod'"));
        assert!(script.contains("cd '/srv'"));
        assert!(script.contains(r#"exec "$_eggie_exec" "$_eggie_arg""#));
    }

    /// Execute the generated bootstrap the way a remote `sh` would, against a scratch `$HOME`.
    ///
    /// The assertions above check the script's *text*; this one checks that it actually runs. It is the
    /// only test that would catch a POSIX-syntax error, a broken `_eggie_write`, or a degradation path
    /// that dies instead of reaching a shell. Non-interactive (no PTY) — the OSC 133 markers need a
    /// real terminal and are covered by manual smoke runs — so it asserts on the installed tree and on
    /// the environment the final shell inherits.
    #[test]
    fn the_generated_script_runs_and_installs_under_a_real_shell() {
        // Shares the PTY tests' lock: this test spawns shells, and running that concurrently with a
        // test waiting on PTY output makes the latter miss its timeout on a loaded machine.
        let _guard = crate::tests::PTY_TEST_LOCK.lock();
        let home = std::env::temp_dir().join(format!("eggie-bootstrap-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("scratch home");

        let env = BTreeMap::from([("SMOKE".to_owned(), "it's $literal".to_owned())]);
        let launch = remote_launch(&target(), &scripts(), &env, None, true, None);
        let script = decoded_payload(&launch);
        // The script ends by exec'ing a shell; replace that tail with a probe so the test can observe
        // the state the user's shell would have inherited.
        let observable = script.replace(
            r#"if [ -x "$_eggie_shell" ]; then"#,
            "printf 'SMOKE=[%s] ZDOTDIR=[%s] LEAK=[%s]\n' \"$SMOKE\" \"$ZDOTDIR\" \"$_eggie_root$_eggie_installed\"\nif false; then",
        );

        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&observable)
            .env("HOME", &home)
            .env("SHELL", "/bin/zsh")
            .env_remove("ZDOTDIR")
            .output()
            .expect("the bootstrap must be runnable by /bin/sh");
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Every bundled script reached the scratch home, at the paths the shells look for.
        let root = home.join(REMOTE_ROOT);
        for relative in [
            "zsh/.zshenv",
            "zsh/eggie-integration",
            "bash/eggie.bash",
            "fish/vendor_conf.d/eggie.fish",
            VERSION_MARKER,
        ] {
            assert!(
                root.join(relative).is_file(),
                "{relative} must be installed (stdout: {stdout}, stderr: {})",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // The profile's value survived quoting into the shell's environment, verbatim.
        assert!(stdout.contains("SMOKE=[it's $literal]"), "stdout: {stdout}");
        // zsh's mechanism was selected, pointing at the installed tree.
        assert!(stdout.contains("ZDOTDIR=["), "stdout: {stdout}");
        assert!(stdout.contains("shell-integration-v1/zsh]"), "stdout: {stdout}");
        // Scratch variables are cleaned up before the shell starts.
        assert!(stdout.contains("LEAK=[]"), "internal variables leaked: {stdout}");

        // Idempotence and reinstall are asserted by `the_install_is_skipped_when_the_version_marker_matches`
        // against the script text; re-running the whole script here would mean two more subprocesses in
        // a suite whose PTY tests already compete for the machine.

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Decode the base64 payload back out of the `ssh` command line, as the remote `sh` would.
    fn decoded_payload(launch: &RemoteLaunch) -> String {
        let command = launch.args.last().expect("a remote command");
        let encoded = command
            .split_whitespace()
            .find(|token| token.len() > 40 && !token.contains('\''))
            .expect("the base64 payload token");
        String::from_utf8(BASE64.decode(encoded).expect("valid base64")).expect("utf-8 script")
    }
}

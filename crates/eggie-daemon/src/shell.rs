use super::*;

/// The shell command line and environment Eggie should launch a child session with.
pub(super) struct ShellLaunch {
    pub(super) args: Vec<String>,
    pub(super) env: HashMap<String, String>,
}

/// Build the environment and argument list for a child shell, injecting OSC 133 shell integration
/// for the shells that support it. Pure (no I/O) so it can be unit-tested: the caller passes in the
/// already-installed `integration_root` and the user's current `ZDOTDIR`/`ENV`.
///
/// - zsh: point `ZDOTDIR` at our integration dir, preserving the user's original in
///   `EGGIE_ZDOTDIR_ORIG`. Keeps the existing `-l` login argument.
/// - bash: launch `--posix` with `ENV` pointing at our script (the POSIX-mode injection hook),
///   preserving the user's `ENV` in `EGGIE_BASH_ENV` and forwarding intercepted flags via
///   `EGGIE_BASH_INJECT`/`EGGIE_BASH_RCFILE`. Skipped for Apple's `/bin/bash` (3.2), which disables
///   the `ENV`-based POSIX startup path.
/// - anything else / no integration installed: base environment only; resize falls back to native
///   reflow.
///
/// `custom_args`, when `Some`, replaces Eggie's default `-l` login argument with the user's argv.
/// bash's `--posix` is mandatory for the ENV-based integration hook, so it is force-prepended even
/// over custom args (integration wins; the user's args are still honored after it). When
/// `custom_args` is `None` the behavior is byte-identical to before this knob existed.
// Every argument is an independent shell-launch input assembled into one env map; grouping them
// into a struct would just move the same fields elsewhere without improving clarity.
#[allow(clippy::too_many_arguments)]
/// Whether `key` is an environment variable Eggie itself owns, and therefore must never be settable
/// from a profile's `env`.
///
/// This is a **prefix/allowlist filter, not an insert-order race**: several of Eggie's variables are
/// set only conditionally (`EGGIE_ZDOTDIR_ORIG` and `EGGIE_BASH_ENV` only when the daemon itself has
/// `ZDOTDIR`/`ENV`, which a GUI launch does not), and some are read by the integration scripts but
/// never written by Rust at all (`EGGIE_BASH_RCFILE`, and the `EGGIE_FISH_INTEGRATION` / `_eggie_state`
/// run-once guards). Relying on "Eggie inserts last so it wins" therefore leaves real holes — the worst
/// being `EGGIE_ZDOTDIR_ORIG`, which `zsh/.zshenv` exports as `ZDOTDIR` and then sources startup files
/// from, turning a profile variable into arbitrary code execution in every zsh of that profile.
///
/// Filtering by prefix also keeps the guarantee true as later steps add variables: a new `EGGIE_*`
/// name is covered the moment it exists, with no second place to remember to update.
pub(super) fn is_eggie_owned_env_key(key: &str) -> bool {
    key.starts_with("EGGIE_")
        // The zsh integration's run-once guard is a shell variable, but an inherited environment
        // variable of the same name trips it and silently disables integration.
        || key.starts_with("_eggie")
        || matches!(
            key,
            "TERM"
                | "TERMINFO"
                | "COLORTERM"
                | "TERM_PROGRAM"
                | "TERM_PROGRAM_VERSION"
                | "ZDOTDIR"
                | "ENV"
                | "XDG_DATA_DIRS"
        )
}

pub(super) fn build_shell_env(
    shell_name: &str,
    shell_path: &str,
    terminfo: &Path,
    integration_root: Option<&Path>,
    user_zdotdir: Option<String>,
    user_env_var: Option<String>,
    custom_args: Option<Vec<String>>,
    bin_dir: Option<&Path>,
    shell_features: &str,
    profile_env: &BTreeMap<String, String>,
) -> ShellLaunch {
    let mut env = HashMap::new();
    // Profile variables first, minus anything Eggie owns (see `is_eggie_owned_env_key` — the filter,
    // not the insert order, is what makes shell integration untamperable from a profile). A profile can
    // still set everything Eggie does not own (AWS_PROFILE, DOCKER_HOST, …), which is the point.
    //
    // Scope note: this map is layered onto the daemon's inherited environment (the PTY builder never
    // clears it), so a profile extends rather than replaces the environment. Two variables sit outside
    // the filter: `USER` and `HOME` are applied by the PTY layer *before* this map, so a profile naming
    // them does win — deliberate, since a container-style profile may need a different HOME. `PATH` is
    // not set here at all; the integration scripts append `EGGIE_BIN_DIR` to whatever the shell
    // resolves, so a profile-supplied PATH is honored and still gets the eggie directory.
    env.extend(
        profile_env
            .iter()
            .filter(|(key, _)| !is_eggie_owned_env_key(key))
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    env.insert("TERM".to_owned(), "alacritty".to_owned());
    env.insert(
        "TERMINFO".to_owned(),
        terminfo.to_string_lossy().into_owned(),
    );
    env.insert("COLORTERM".to_owned(), "truecolor".to_owned());
    env.insert("TERM_PROGRAM".to_owned(), "Eggie".to_owned());
    env.insert(
        "TERM_PROGRAM_VERSION".to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );
    // Shell-feature plumbing, injected unconditionally for every shell (even those without an
    // integration script): EGGIE_BIN_DIR points at the eggie binary's directory, and
    // EGGIE_SHELL_FEATURES is the comma-separated token list the integration scripts read. This
    // way, once a shell's script learns to read them, the variables are already in place.
    if let Some(dir) = bin_dir {
        env.insert(
            "EGGIE_BIN_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        );
    }
    env.insert("EGGIE_SHELL_FEATURES".to_owned(), shell_features.to_owned());

    let custom = custom_args.is_some();
    let mut args = custom_args.unwrap_or_else(|| vec!["-l".to_owned()]);

    match (shell_name, integration_root) {
        ("zsh", Some(root)) => {
            if let Some(original) = user_zdotdir {
                env.insert("EGGIE_ZDOTDIR_ORIG".to_owned(), original);
            }
            env.insert(
                "ZDOTDIR".to_owned(),
                root.join("zsh").to_string_lossy().into_owned(),
            );
        }
        // Apple's /bin/bash is 3.2 and does not honor the ENV-based POSIX startup path, so
        // integration is impossible there; fall through to no injection.
        ("bash", Some(root)) if shell_path != "/bin/bash" || !cfg!(target_os = "macos") => {
            // `--posix` is required for the ENV-based POSIX startup hook. Without custom args this is
            // the sole argument (preserving the original default); with custom args we force it to
            // the front so integration still works while keeping the user's args.
            if !custom {
                args = vec!["--posix".to_owned()];
            } else if !args.iter().any(|arg| arg == "--posix") {
                args.insert(0, "--posix".to_owned());
            }
            if let Some(original) = user_env_var {
                env.insert("EGGIE_BASH_ENV".to_owned(), original);
            }
            env.insert(
                "ENV".to_owned(),
                root.join("bash")
                    .join("eggie.bash")
                    .to_string_lossy()
                    .into_owned(),
            );
            // "1" marks an automatic (Eggie) injection; the script also reads any forwarded flags
            // from here. We do not intercept --norc/--rcfile from SHELL today, so this is just "1".
            env.insert("EGGIE_BASH_INJECT".to_owned(), "1".to_owned());
        }
        // Fish sources vendor conf.d from every XDG_DATA_DIRS entry (not just the first), so
        // prepending the integration root makes fish pick up `fish/vendor_conf.d/eggie.fish`
        // alongside the user's other vendor confs. The script's own run-once guard prevents
        // double-loading. Preserve any existing XDG_DATA_DIRS (other tools read it too); when
        // unset, seed the common macOS/Linux paths so Homebrew's vendor confs still load.
        // Known gap: MacPorts (/opt/local/share) and nix paths are not in the fallback, but
        // fish's compiled-in vendor conf dir is always searched regardless of XDG_DATA_DIRS.
        ("fish", Some(root)) => {
            env.insert(
                "XDG_DATA_DIRS".to_owned(),
                fish_xdg_data_dirs(root, std::env::var("XDG_DATA_DIRS").ok().as_deref()),
            );
        }
        _ => {}
    }

    ShellLaunch { args, env }
}

/// Build the XDG_DATA_DIRS value for a fish session: the integration root first (so our
/// `fish/vendor_conf.d/eggie.fish` is found), then the user's existing dirs, or the common
/// macOS/Linux data paths when the variable is unset/empty so Homebrew's vendor confs still load.
pub(super) fn fish_xdg_data_dirs(root: &Path, existing: Option<&str>) -> String {
    let mut dirs = vec![root.to_string_lossy().into_owned()];
    match existing {
        Some(value) if !value.is_empty() => dirs.push(value.to_owned()),
        _ => {
            if cfg!(target_os = "macos") {
                dirs.push("/opt/homebrew/share".to_owned());
            }
            dirs.push("/usr/local/share".to_owned());
            dirs.push("/usr/share".to_owned());
        }
    }
    dirs.join(":")
}

pub(super) fn install_bundled_terminfo() -> Result<PathBuf> {
    match INSTALLED_TERMINFO
        .get_or_init(|| install_bundled_terminfo_inner().map_err(|error| format!("{error:#}")))
    {
        Ok(path) => Ok(path.clone()),
        Err(error) => bail!("failed to install bundled terminfo: {error}"),
    }
}

pub(super) fn install_bundled_terminfo_inner() -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let database = std::env::temp_dir()
        .join(format!("eggie-{uid}"))
        .join("terminfo-v1");
    let directory = database.join("61");
    let entry = directory.join("alacritty");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&database, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;

    write_if_changed(&entry, BUNDLED_ALACRITTY_TERMINFO)?;
    Ok(database)
}

/// Write `contents` to `path` (mode 0600) only when it differs from what is already there, via a
/// per-process temporary file and an atomic rename so concurrent sessions never observe a partial
/// file. Shared by the terminfo and shell-integration installers.
pub(super) fn write_if_changed(path: &Path, contents: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("cannot install {}: no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("cannot install {}: invalid file name", path.display()))?;
    let temporary = directory.join(format!("{file_name}.tmp-{}", process::id()));
    fs::write(&temporary, contents)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Install the bundled shell-integration scripts into a per-user runtime directory and return its
/// root. The zsh subdirectory becomes `ZDOTDIR`; the bash script under it becomes `ENV`; the fish
/// script is placed under `fish/vendor_conf.d/` and found via `XDG_DATA_DIRS`.
pub(super) fn install_bundled_shell_integration() -> Result<PathBuf> {
    match INSTALLED_SHELL_INTEGRATION.get_or_init(|| {
        install_bundled_shell_integration_inner().map_err(|error| format!("{error:#}"))
    }) {
        Ok(path) => Ok(path.clone()),
        Err(error) => bail!("failed to install bundled shell integration: {error}"),
    }
}

pub(super) fn install_bundled_shell_integration_inner() -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let root = std::env::temp_dir()
        .join(format!("eggie-{uid}"))
        .join("shell-integration-v1");
    let zsh = root.join("zsh");
    let bash = root.join("bash");
    let fish = root.join("fish").join("vendor_conf.d");
    fs::create_dir_all(&zsh)?;
    fs::create_dir_all(&bash)?;
    fs::create_dir_all(&fish)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&zsh, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&bash, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&fish, fs::Permissions::from_mode(0o700))?;

    // The dotfile name is required verbatim so zsh's ZDOTDIR mechanism finds it.
    write_if_changed(&zsh.join(".zshenv"), BUNDLED_ZSH_ZSHENV)?;
    write_if_changed(&zsh.join("eggie-integration"), BUNDLED_ZSH_INTEGRATION)?;
    write_if_changed(&bash.join("eggie.bash"), BUNDLED_BASH_INTEGRATION)?;
    // The vendor_conf.d path is required verbatim so fish's XDG_DATA_DIRS mechanism finds it.
    write_if_changed(&fish.join("eggie.fish"), BUNDLED_FISH_INTEGRATION)?;
    Ok(root)
}

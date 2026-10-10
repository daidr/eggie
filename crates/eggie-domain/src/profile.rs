use super::*;

/// Stable identity of the built-in "Default" profile.
///
/// Hard-coded rather than randomly generated at first run so that a [`Project::default_profile_id`]
/// pointing at it stays valid across reinstalls, and so the one-time migration of the global shell
/// settings into a profile is idempotent.
pub const DEFAULT_PROFILE_ID: ProfileId =
    Uuid::from_u128(0xeee1_e000_0000_4000_8000_0000_0000_0001);

/// A named launch recipe for terminals: what to run, where, with which environment, and how it
/// should look.
///
/// A profile is the third configuration layer between the global app settings and a single terminal.
/// Launch fields (`shell_program`, `shell_args`, `env`, `cwd`, `remote`) are resolved once when the
/// terminal spawns — a running shell cannot be swapped. Appearance fields are re-resolved whenever
/// the profile or the global settings change, so a live terminal follows edits to either
/// ([`ProfileAppearance`] documents the merge order).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    /// Marks the built-in Default profile, which cannot be deleted (it is the fallback every stale
    /// [`Project::default_profile_id`] resolves to). Never serialized for custom profiles.
    #[serde(default, skip_serializing_if = "is_false")]
    pub builtin: bool,
    /// Shell or command to launch. Empty = fall back to `$SHELL` (then `/bin/zsh`), matching the
    /// global setting this field migrated from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell_program: String,
    /// Arguments for `shell_program`. Empty = Eggie's default launch args.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell_args: Vec<String>,
    /// Extra environment variables exported into the terminal. A `BTreeMap` so the persisted order
    /// is stable. Eggie's own shell-integration variables are applied *after* these and win, so a
    /// profile cannot accidentally break integration by naming one of them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Working directory for new terminals. `None` = the project root (or, for a new tab opened
    /// beside an existing one, that terminal's current directory). For a [`RemoteTarget`] profile
    /// this is a path on the *remote* host; `None` means the remote home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Appearance overrides layered on top of the global settings.
    #[serde(default, skip_serializing_if = "ProfileAppearance::is_empty")]
    pub appearance: ProfileAppearance,
    /// When set, this profile opens a session on a remote host and Eggie may install its shell
    /// integration there. `None` = a local profile (which can still run `ssh` as a plain command —
    /// it just gets no automatic remote integration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteTarget>,
    /// Accent color (`0xRRGGBB`) for this profile's tab badge, so a terminal's identity is visible
    /// at a glance. `None` = no badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub badge_color: Option<u32>,
    /// The `path` shell-integration feature: append Eggie's binary directory to the shell `PATH` so
    /// the `eggie` CLI is runnable inside the terminal. Migrated from the global setting of the same
    /// name; per-profile because a remote or container profile has no local binary to point at.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub shell_integration_path: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

fn default_true() -> bool {
    true
}

impl Profile {
    /// The built-in Default profile: launches the user's `$SHELL` with Eggie's default args and no
    /// overrides. Callers migrating the global shell settings overwrite the launch fields in place.
    pub fn default_builtin() -> Self {
        Self {
            id: DEFAULT_PROFILE_ID,
            name: "Default".to_owned(),
            builtin: true,
            shell_program: String::new(),
            shell_args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            appearance: ProfileAppearance::default(),
            remote: None,
            badge_color: None,
            shell_integration_path: true,
        }
    }

    /// A new custom profile with the given name and no overrides.
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            builtin: false,
            shell_program: String::new(),
            shell_args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            appearance: ProfileAppearance::default(),
            remote: None,
            badge_color: None,
            shell_integration_path: true,
        }
    }

    /// Whether this profile targets a remote host (and so participates in remote shell-integration
    /// installation).
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }
}

/// Whether `key` is a usable environment variable name (`[A-Za-z_][A-Za-z0-9_]*`).
///
/// Lives here, in the shared model, because three independent places need exactly this rule — the
/// profile editor (reject at entry), the client's spawn config, and the daemon's remote bootstrap —
/// and a rule that drifts between them is what made the bootstrap injectable in the first place.
///
/// The rule has to be a rejection rather than an escape: a name appears on the **left** of
/// `NAME=value`, which is not a quotable position. A name containing a newline turns the rest of the
/// line into its own command; `;` and `$(…)` do the same inline. Values are quoted instead, and can
/// hold anything.
///
/// Deliberately narrower than what a POSIX shell would tolerate: `export` accepts some names that
/// `NAME=value` does not, and a name outside this set is always either a typo or an attack.
pub fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Per-profile appearance overrides. Every field is optional: `None` means "inherit".
///
/// Resolution is a three-layer merge — profile override, then the global app setting, then the
/// built-in default — recomputed whenever either layer changes, so editing the global font size
/// moves every terminal that does not override it and leaves the ones that do.
///
/// A profile overrides only the **terminal** theme, never the interface: chrome is window-level, and
/// one window can display tabs from several profiles at once (side by side, when split), so there is
/// no single profile a window's chrome could follow.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileAppearance {
    /// Terminal theme for the dark appearance, resolved against the app's theme catalog. An unknown
    /// name falls back to the global terminal theme, so a typo degrades to "no override".
    ///
    /// A profile that must look the same in both appearances (a production host painted red, say) sets
    /// this and `terminal_light_theme` to the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_dark_theme: Option<String>,
    /// Terminal theme for the light appearance. See [`Self::terminal_dark_theme`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_light_theme: Option<String>,
    /// The single-slot override this type used to carry, when one theme name covered both appearances.
    /// Read on load, folded into the two slots above by [`Self::migrate_legacy`], and never written
    /// back (`skip_serializing`), so the old key disappears from disk after the first save.
    #[serde(default, rename = "theme", skip_serializing)]
    pub legacy_theme: Option<String>,
    /// Terminal font family.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    /// Terminal font size in points. Clamped to the app's supported range on resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

impl ProfileAppearance {
    /// Whether every field inherits (nothing to serialize, nothing to override).
    ///
    /// Deliberately ignores `legacy_theme`: [`Self::migrate_legacy`] runs on load, so by the time
    /// anything asks, it is always `None`. Counting it would let an unmigrated value read as an
    /// override that no slot actually holds.
    pub fn is_empty(&self) -> bool {
        self.terminal_dark_theme.is_none()
            && self.terminal_light_theme.is_none()
            && self.font_family.is_none()
            && self.font_size.is_none()
    }

    /// The theme override for the appearance currently in effect, if any.
    pub fn terminal_theme(&self, use_dark: bool) -> Option<&str> {
        if use_dark {
            self.terminal_dark_theme.as_deref()
        } else {
            self.terminal_light_theme.as_deref()
        }
    }

    /// Fold a legacy single-slot `theme` into both terminal slots, preserving its "same in both
    /// appearances" meaning. Only fills slots that are still empty, so a file already carrying the new
    /// fields wins. Idempotent: the legacy value is taken, not copied.
    pub fn migrate_legacy(&mut self) {
        let Some(legacy) = self.legacy_theme.take() else {
            return;
        };
        if self.terminal_dark_theme.is_none() {
            self.terminal_dark_theme = Some(legacy.clone());
        }
        if self.terminal_light_theme.is_none() {
            self.terminal_light_theme = Some(legacy);
        }
    }
}

/// A remote host a profile connects to. Declaring the target as structured data (rather than only
/// as an `ssh host` command line) is what lets Eggie install its shell integration there: the host
/// has to be known *before* the connection is made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTarget {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl RemoteTarget {
    pub fn new(host: String) -> Self {
        Self {
            host,
            user: None,
            port: None,
        }
    }

    /// `user@host` when a user is set, otherwise just the host. This is the `ssh` destination
    /// argument; the port travels separately as `-p`.
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// The key under which the user's install consent is remembered.
    ///
    /// Includes the **port**, not just `user@host`: a non-default port is very often a different
    /// machine (a bastion forward, a container, a VM), so consenting to write into one account's home
    /// directory must not silently authorize another host reachable through a different port. Also per
    /// account, because the integration lands in that account's `$HOME`.
    ///
    /// Erring toward over-prompting is deliberate — a hostname and its IP, or two CNAMEs for one host,
    /// key separately and ask twice. Asking again is a small annoyance; writing to a machine the user
    /// never approved is not.
    pub fn consent_key(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{port}", self.destination()),
            None => self.destination(),
        }
    }

    /// Whether this target is safe to hand to `ssh`.
    ///
    /// A host or user beginning with `-` would be parsed by ssh as an option rather than a
    /// destination; `-oProxyCommand=<cmd>` makes ssh run `<cmd>` on the *local* machine at connect
    /// time, which is arbitrary local code execution from a profile field. The caller also passes
    /// `--` before the destination, so this is the second of two independent guards — a hand-edited
    /// `profiles.json` should be rejected outright rather than merely defanged.
    ///
    /// Whitespace is rejected for the same reason: a destination is a single argv entry, and a value
    /// containing spaces is always a mistake (or an attempt to smuggle one).
    pub fn is_usable(&self) -> bool {
        let sane = |value: &str| {
            !value.is_empty()
                && !value.starts_with('-')
                && !value.contains(char::is_whitespace)
                && !value.contains('\0')
        };
        sane(&self.host) && self.user.as_deref().is_none_or(sane)
    }
}

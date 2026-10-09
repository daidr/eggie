use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

pub type ProjectId = Uuid;
pub type GroupId = Uuid;
pub type ItemId = Uuid;
pub type SessionId = Uuid;
/// Identity of a [`Profile`]. Terminals record the profile they were launched from, and projects
/// name a default profile for new terminals.
pub type ProfileId = Uuid;
/// Identity of a GUI window. A session in the daemon is tagged with the id of the window that owns
/// it so that each window sees only its own sessions; `None` ownership marks a detached session.
pub type WindowId = Uuid;
/// Identity of a split node in a [`LayoutNode`] tree. Distinct from [`GroupId`] so a split is never
/// confused with the groups it contains.
pub type SplitId = Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Default working directory for new terminals in this project. `None` means the user's home
    /// directory is used.
    pub root: Option<PathBuf>,
    /// Profile new terminals in this project launch with. `None` means the global default profile.
    /// A stale id (the profile was deleted) also resolves to the global default rather than failing.
    #[serde(default)]
    pub default_profile_id: Option<ProfileId>,
}

impl Project {
    pub fn from_root(root: PathBuf) -> Self {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Project")
            .to_owned();
        Self {
            id: Uuid::new_v4(),
            name,
            root: Some(root),
            default_profile_id: None,
        }
    }

    /// Create a project with only a name and no default working directory. New terminals will
    /// start in the user's home directory until a root is set.
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            root: None,
            default_profile_id: None,
        }
    }

    /// The working directory to use for new terminals: the configured root, or the user's home
    /// directory if none is set.
    pub fn effective_root(&self) -> PathBuf {
        self.root
            .clone()
            .unwrap_or_else(|| home_dir())
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
}

/// Stable identity of the built-in "Default" profile.
///
/// Hard-coded rather than randomly generated at first run so that a [`Project::default_profile_id`]
/// pointing at it stays valid across reinstalls, and so the one-time migration of the global shell
/// settings into a profile is idempotent.
pub const DEFAULT_PROFILE_ID: ProfileId = Uuid::from_u128(0xeee1_e000_0000_4000_8000_0000_0000_0001);

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    #[default]
    Terminal,
    File,
    Browser,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabItem {
    pub id: ItemId,
    pub kind: ItemKind,
    /// The process/OSC-derived title. Kept updated by the running shell and never reflects a
    /// user override, so removing a custom title falls back to the live process title.
    pub title: String,
    /// A user-assigned override that wins over `title` when set. Independent metadata: OSC title
    /// updates leave it untouched, and clearing it (`None`) restores the process title.
    #[serde(default)]
    pub custom_title: Option<String>,
    pub session_id: Option<SessionId>,
    /// The profile this terminal was launched from, recorded so the tab can show its badge and so
    /// appearance stays resolvable while the terminal lives. Locked at creation: the launch fields
    /// are already baked into the running shell, so re-pointing a live tab at another profile would
    /// only ever be half-true. `None` for items that predate profiles or are not terminals.
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

impl TabItem {
    pub fn terminal(session_id: SessionId, title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: ItemKind::Terminal,
            title: title.into(),
            custom_title: None,
            session_id: Some(session_id),
            profile_id: None,
        }
    }

    /// A terminal tab launched from `profile_id`.
    pub fn terminal_with_profile(
        session_id: SessionId,
        title: impl Into<String>,
        profile_id: ProfileId,
    ) -> Self {
        Self {
            profile_id: Some(profile_id),
            ..Self::terminal(session_id, title)
        }
    }

    /// The title to render: the user's custom title if set, otherwise the process title.
    pub fn display_title(&self) -> &str {
        self.custom_title.as_deref().unwrap_or(&self.title)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Horizontal,
            Self::Up | Self::Down => SplitAxis::Vertical,
        }
    }

    fn inserts_before(self) -> bool {
        matches!(self, Self::Up | Self::Left)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: GroupId,
    pub items: Vec<TabItem>,
    pub active_item_id: Option<ItemId>,
}

impl TabGroup {
    pub fn new(item: TabItem) -> Self {
        let active_item_id = Some(item.id);
        Self {
            id: Uuid::new_v4(),
            items: vec![item],
            active_item_id,
        }
    }

    pub fn add_item(&mut self, item: TabItem) {
        self.active_item_id = Some(item.id);
        self.items.push(item);
    }

    pub fn insert_item(&mut self, index: usize, item: TabItem) {
        self.active_item_id = Some(item.id);
        self.items.insert(index.min(self.items.len()), item);
    }

    pub fn close_item(&mut self, item_id: ItemId) -> Option<TabItem> {
        let index = self.items.iter().position(|item| item.id == item_id)?;
        let removed = self.items.remove(index);
        if self.active_item_id == Some(item_id) {
            self.active_item_id = self
                .items
                .get(index.min(self.items.len().saturating_sub(1)))
                .map(|item| item.id);
        }
        Some(removed)
    }

    pub fn active_item(&self) -> Option<&TabItem> {
        let active = self.active_item_id?;
        self.items.iter().find(|item| item.id == active)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Group {
        group: TabGroup,
    },
    Split {
        /// Stable, globally unique identity for this split.
        ///
        /// A split must NOT be identified by the first group inside its `first` subtree: when the
        /// `first` subtree is itself a split, the outer and inner splits would derive the same
        /// "first group" and collide, so resizing the inner divider would move the outer one. This
        /// explicit id sidesteps that entirely.
        #[serde(default = "Uuid::new_v4")]
        id: SplitId,
        axis: SplitAxis,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn group(item: TabItem) -> Self {
        Self::Group {
            group: TabGroup::new(item),
        }
    }

    /// A layout with a single empty group and no tabs. Used for a project that currently has no
    /// terminal in this window (e.g. a project synced from another window, or one whose sessions
    /// were all detached): the group is a valid drop target for the first new terminal.
    pub fn empty() -> Self {
        Self::Group {
            group: TabGroup {
                id: Uuid::new_v4(),
                items: Vec::new(),
                active_item_id: None,
            },
        }
    }

    pub fn find_group_mut(&mut self, group_id: GroupId) -> Option<&mut TabGroup> {
        match self {
            Self::Group { group } => (group.id == group_id).then_some(group),
            Self::Split { first, second, .. } => first
                .find_group_mut(group_id)
                .or_else(|| second.find_group_mut(group_id)),
        }
    }

    pub fn find_group(&self, group_id: GroupId) -> Option<&TabGroup> {
        match self {
            Self::Group { group } => (group.id == group_id).then_some(group),
            Self::Split { first, second, .. } => first
                .find_group(group_id)
                .or_else(|| second.find_group(group_id)),
        }
    }

    pub fn find_terminal_item(&self, session_id: SessionId) -> Option<(GroupId, ItemId)> {
        match self {
            Self::Group { group } => group
                .items
                .iter()
                .find(|item| item.session_id == Some(session_id))
                .map(|item| (group.id, item.id)),
            Self::Split { first, second, .. } => first
                .find_terminal_item(session_id)
                .or_else(|| second.find_terminal_item(session_id)),
        }
    }

    /// Updates the title of every tab backed by `session_id`.
    ///
    /// A session can move between groups while its terminal process keeps running, so the
    /// session ID—not the current group or item ID—is the stable identity for OSC title updates.
    pub fn update_terminal_title(&mut self, session_id: SessionId, title: &str) -> bool {
        match self {
            Self::Group { group } => {
                let mut changed = false;
                for item in &mut group.items {
                    if item.session_id == Some(session_id) && item.title != title {
                        item.title = title.to_owned();
                        changed = true;
                    }
                }
                changed
            }
            Self::Split { first, second, .. } => {
                let first_changed = first.update_terminal_title(session_id, title);
                let second_changed = second.update_terminal_title(session_id, title);
                first_changed || second_changed
            }
        }
    }

    /// The title to display for the tab backed by `session_id`: the tab's custom title if set,
    /// otherwise its process title. Returns `None` when no tab is backed by that session.
    ///
    /// Used by surfaces outside the tab bar (e.g. the sidebar session list) that key off a session
    /// id rather than a tab, so a user's custom tab title stays consistent everywhere.
    pub fn terminal_display_title(&self, session_id: SessionId) -> Option<&str> {
        match self {
            Self::Group { group } => group
                .items
                .iter()
                .find(|item| item.session_id == Some(session_id))
                .map(|item| item.display_title()),
            Self::Split { first, second, .. } => first
                .terminal_display_title(session_id)
                .or_else(|| second.terminal_display_title(session_id)),
        }
    }

    /// Sets (or clears, with `None`) the user's custom title override for a single tab, identified
    /// by its group and item id. Returns `true` if the override actually changed.
    ///
    /// Unlike [`Self::update_terminal_title`], this targets one specific item rather than every tab
    /// backed by a session: a custom title is per-tab metadata, not a process property.
    pub fn set_custom_title(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        custom_title: Option<String>,
    ) -> bool {
        let Some(group) = self.find_group_mut(group_id) else {
            return false;
        };
        let Some(item) = group.items.iter_mut().find(|item| item.id == item_id) else {
            return false;
        };
        if item.custom_title == custom_title {
            return false;
        }
        item.custom_title = custom_title;
        true
    }

    pub fn first_group_id(&self) -> GroupId {
        match self {
            Self::Group { group } => group.id,
            Self::Split { first, .. } => first.first_group_id(),
        }
    }

    /// Returns this node's [`SplitId`] if it is a split, or `None` for a group.
    pub fn split_id(&self) -> Option<SplitId> {
        match self {
            Self::Group { .. } => None,
            Self::Split { id, .. } => Some(*id),
        }
    }

    /// Returns the ratio of the split with the given [`SplitId`].
    pub fn split_ratio(&self, split_id: SplitId) -> Option<f32> {
        match self {
            Self::Group { .. } => None,
            Self::Split { id, ratio, .. } if *id == split_id => Some(*ratio),
            Self::Split { first, second, .. } => first
                .split_ratio(split_id)
                .or_else(|| second.split_ratio(split_id)),
        }
    }

    /// Returns the axis of the split with the given [`SplitId`].
    pub fn split_axis(&self, split_id: SplitId) -> Option<SplitAxis> {
        match self {
            Self::Group { .. } => None,
            Self::Split { id, axis, .. } if *id == split_id => Some(*axis),
            Self::Split { first, second, .. } => first
                .split_axis(split_id)
                .or_else(|| second.split_axis(split_id)),
        }
    }

    /// Updates the ratio of the split with the given [`SplitId`].
    ///
    /// Returns `true` if a matching split was found and updated. The ratio is clamped to
    /// `0.05..=0.95` so neither pane can collapse entirely.
    pub fn set_split_ratio(&mut self, split_id: SplitId, ratio: f32) -> bool {
        let ratio = ratio.clamp(0.05, 0.95);
        match self {
            Self::Group { .. } => false,
            Self::Split {
                id,
                ratio: current,
                ..
            } if *id == split_id => {
                *current = ratio;
                true
            }
            Self::Split { first, second, .. } => {
                first.set_split_ratio(split_id, ratio)
                    || second.set_split_ratio(split_id, ratio)
            }
        }
    }

    pub fn top_left_group_id(&self) -> GroupId {
        match self {
            Self::Group { group } => group.id,
            Self::Split { first, .. } => first.top_left_group_id(),
        }
    }

    pub fn top_right_group_id(&self) -> GroupId {
        match self {
            Self::Group { group } => group.id,
            Self::Split {
                axis: SplitAxis::Horizontal,
                second,
                ..
            } => second.top_right_group_id(),
            Self::Split {
                axis: SplitAxis::Vertical,
                first,
                ..
            } => first.top_right_group_id(),
        }
    }

    pub fn split_group(
        &mut self,
        group_id: GroupId,
        axis: SplitAxis,
        item: TabItem,
    ) -> Option<GroupId> {
        let direction = match axis {
            SplitAxis::Horizontal => Direction::Right,
            SplitAxis::Vertical => Direction::Down,
        };
        self.split_group_direction(group_id, direction, item)
    }

    pub fn split_group_direction(
        &mut self,
        group_id: GroupId,
        direction: Direction,
        item: TabItem,
    ) -> Option<GroupId> {
        match self {
            Self::Group { group } if group.id == group_id => {
                let replacement = Self::Group {
                    group: TabGroup {
                        id: group.id,
                        items: std::mem::take(&mut group.items),
                        active_item_id: group.active_item_id.take(),
                    },
                };
                let new_group = TabGroup::new(item);
                let new_group_id = new_group.id;
                let old_group = Box::new(replacement);
                let new_group = Box::new(Self::Group { group: new_group });
                let (first, second) = if direction.inserts_before() {
                    (new_group, old_group)
                } else {
                    (old_group, new_group)
                };
                *self = Self::Split {
                    id: Uuid::new_v4(),
                    axis: direction.axis(),
                    ratio: 0.5,
                    first,
                    second,
                };
                Some(new_group_id)
            }
            Self::Group { .. } => None,
            Self::Split { first, second, .. } => first
                .split_group_direction(group_id, direction, item.clone())
                .or_else(|| second.split_group_direction(group_id, direction, item)),
        }
    }

    pub fn move_item(
        &mut self,
        source_group_id: GroupId,
        item_id: ItemId,
        target_group_id: GroupId,
    ) -> Option<GroupId> {
        if source_group_id == target_group_id {
            let group = self.find_group_mut(source_group_id)?;
            group.active_item_id = Some(item_id);
            return Some(source_group_id);
        }
        self.find_group(target_group_id)?;
        let item = self.close_item(source_group_id, item_id)?;
        self.find_group_mut(target_group_id)?.add_item(item);
        Some(target_group_id)
    }

    pub fn move_item_at(
        &mut self,
        source_group_id: GroupId,
        item_id: ItemId,
        target_group_id: GroupId,
        insertion_index: usize,
    ) -> Option<GroupId> {
        if source_group_id == target_group_id {
            let group = self.find_group_mut(source_group_id)?;
            let source_index = group.items.iter().position(|item| item.id == item_id)?;
            let insertion_index = insertion_index.min(group.items.len());
            let item = group.items.remove(source_index);
            let adjusted_index = if insertion_index > source_index {
                insertion_index - 1
            } else {
                insertion_index
            };
            group.insert_item(adjusted_index, item);
            return Some(source_group_id);
        }

        let target_length = self.find_group(target_group_id)?.items.len();
        let item = self.close_item(source_group_id, item_id)?;
        self.find_group_mut(target_group_id)?
            .insert_item(insertion_index.min(target_length), item);
        Some(target_group_id)
    }

    pub fn move_item_to_new_group(
        &mut self,
        source_group_id: GroupId,
        item_id: ItemId,
        target_group_id: GroupId,
        direction: Direction,
    ) -> Option<GroupId> {
        self.find_group(target_group_id)?;
        let source_item_count = self.find_group(source_group_id)?.items.len();
        if source_group_id == target_group_id && source_item_count == 1 {
            return None;
        }

        let item = self.close_item(source_group_id, item_id)?;
        self.split_group_direction(target_group_id, direction, item)
    }

    pub fn neighbor_group_id(
        &self,
        source_group_id: GroupId,
        direction: Direction,
    ) -> Option<GroupId> {
        let mut groups = Vec::new();
        self.collect_group_rects(NormalizedRect::FULL, &mut groups);
        let source = groups
            .iter()
            .find(|(group_id, _)| *group_id == source_group_id)
            .map(|(_, rect)| *rect)?;

        groups
            .into_iter()
            .filter(|(group_id, _)| *group_id != source_group_id)
            .filter_map(|(group_id, candidate)| {
                let primary_distance = match direction {
                    Direction::Up if candidate.center_y() < source.center_y() => {
                        source.center_y() - candidate.center_y()
                    }
                    Direction::Down if candidate.center_y() > source.center_y() => {
                        candidate.center_y() - source.center_y()
                    }
                    Direction::Left if candidate.center_x() < source.center_x() => {
                        source.center_x() - candidate.center_x()
                    }
                    Direction::Right if candidate.center_x() > source.center_x() => {
                        candidate.center_x() - source.center_x()
                    }
                    _ => return None,
                };
                let cross_axis_gap = match direction {
                    Direction::Up | Direction::Down => interval_gap(
                        source.left,
                        source.right(),
                        candidate.left,
                        candidate.right(),
                    ),
                    Direction::Left | Direction::Right => interval_gap(
                        source.top,
                        source.bottom(),
                        candidate.top,
                        candidate.bottom(),
                    ),
                };
                Some((group_id, primary_distance + cross_axis_gap * 4.))
            })
            .min_by(|(_, first), (_, second)| first.total_cmp(second))
            .map(|(group_id, _)| group_id)
    }

    pub fn group_ids(&self) -> Vec<GroupId> {
        let mut groups = Vec::new();
        self.collect_group_rects(NormalizedRect::FULL, &mut groups);
        groups.into_iter().map(|(group_id, _)| group_id).collect()
    }

    /// The [`SplitId`]s of every split in this layout tree. Mirrors [`group_ids`]; used by the UI to
    /// retain per-split render caches (divider bounds) against the live layout so closed splits do
    /// not leak stale entries.
    pub fn split_ids(&self) -> Vec<SplitId> {
        let mut splits = Vec::new();
        self.collect_split_ids(&mut splits);
        splits
    }

    fn collect_split_ids(&self, splits: &mut Vec<SplitId>) {
        if let Self::Split {
            id, first, second, ..
        } = self
        {
            splits.push(*id);
            first.collect_split_ids(splits);
            second.collect_split_ids(splits);
        }
    }

    /// Total number of tab items across all groups in this layout.
    pub fn item_count(&self) -> usize {
        match self {
            Self::Group { group } => group.items.len(),
            Self::Split { first, second, .. } => first.item_count() + second.item_count(),
        }
    }

    /// Iterate over all tab items across all groups, in layout order.
    pub fn items(&self) -> impl Iterator<Item = &TabItem> {
        let mut items = Vec::new();
        self.collect_items(&mut items);
        items.into_iter()
    }

    /// Iterate over terminal tabs across all groups.
    pub fn terminal_items(&self) -> impl Iterator<Item = &TabItem> {
        self.items().filter(|item| matches!(item.kind, ItemKind::Terminal))
    }

    fn collect_items<'a>(&'a self, items: &mut Vec<&'a TabItem>) {
        match self {
            Self::Group { group } => {
                items.extend(&group.items);
            }
            Self::Split { first, second, .. } => {
                first.collect_items(items);
                second.collect_items(items);
            }
        }
    }

    fn collect_group_rects(
        &self,
        rect: NormalizedRect,
        groups: &mut Vec<(GroupId, NormalizedRect)>,
    ) {
        match self {
            Self::Group { group } => groups.push((group.id, rect)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } => {
                let ratio = ratio.clamp(0.05, 0.95);
                let (first_rect, second_rect) = rect.split(*axis, ratio);
                first.collect_group_rects(first_rect, groups);
                second.collect_group_rects(second_rect, groups);
            }
        }
    }

    pub fn close_item(&mut self, group_id: GroupId, item_id: ItemId) -> Option<TabItem> {
        let removed = self.find_group_mut(group_id)?.close_item(item_id)?;
        let group_is_empty = self
            .find_group(group_id)
            .is_some_and(|group| group.items.is_empty());
        if group_is_empty {
            let placeholder = Self::empty_group(group_id);
            let current = std::mem::replace(self, placeholder);
            if let Some(pruned) = current.without_empty_group(group_id) {
                *self = pruned;
            }
        }
        Some(removed)
    }

    fn empty_group(group_id: GroupId) -> Self {
        Self::Group {
            group: TabGroup {
                id: group_id,
                items: Vec::new(),
                active_item_id: None,
            },
        }
    }

    fn without_empty_group(self, group_id: GroupId) -> Option<Self> {
        match self {
            Self::Group { group } if group.id == group_id && group.items.is_empty() => None,
            Self::Group { .. } => Some(self),
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => match (
                first.without_empty_group(group_id),
                second.without_empty_group(group_id),
            ) {
                // Both subtrees survive: keep this split with its original id so a live drag
                // targeting it is not invalidated by an unrelated group closing elsewhere.
                (Some(first), Some(second)) => Some(Self::Split {
                    id,
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct NormalizedRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl NormalizedRect {
    const FULL: Self = Self {
        left: 0.,
        top: 0.,
        width: 1.,
        height: 1.,
    };

    fn right(self) -> f32 {
        self.left + self.width
    }

    fn bottom(self) -> f32 {
        self.top + self.height
    }

    fn center_x(self) -> f32 {
        self.left + self.width / 2.
    }

    fn center_y(self) -> f32 {
        self.top + self.height / 2.
    }

    fn split(self, axis: SplitAxis, ratio: f32) -> (Self, Self) {
        match axis {
            SplitAxis::Horizontal => {
                let first_width = self.width * ratio;
                (
                    Self {
                        width: first_width,
                        ..self
                    },
                    Self {
                        left: self.left + first_width,
                        width: self.width - first_width,
                        ..self
                    },
                )
            }
            SplitAxis::Vertical => {
                let first_height = self.height * ratio;
                (
                    Self {
                        height: first_height,
                        ..self
                    },
                    Self {
                        top: self.top + first_height,
                        height: self.height - first_height,
                        ..self
                    },
                )
            }
        }
    }
}

fn interval_gap(first_start: f32, first_end: f32, second_start: f32, second_end: f32) -> f32 {
    if first_end < second_start {
        second_start - first_end
    } else if second_end < first_start {
        first_start - second_end
    } else {
        0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_single_slot_theme_migrates_into_both_terminal_slots() {
        // The pre-split shape: one `theme` name that covered both appearances. Folding it into both
        // slots preserves exactly that meaning.
        let mut appearance: ProfileAppearance =
            serde_json::from_str(r#"{"theme":"iTerm2 Tango Light"}"#).unwrap();
        assert_eq!(appearance.legacy_theme.as_deref(), Some("iTerm2 Tango Light"));
        appearance.migrate_legacy();

        assert_eq!(appearance.terminal_dark_theme.as_deref(), Some("iTerm2 Tango Light"));
        assert_eq!(appearance.terminal_light_theme.as_deref(), Some("iTerm2 Tango Light"));
        assert!(appearance.legacy_theme.is_none(), "the legacy value is taken, not copied");
        assert!(!appearance.is_empty());

        // Re-serializing drops the old key, so the migration happens once and leaves no trace.
        let encoded = serde_json::to_string(&appearance).unwrap();
        assert!(!encoded.contains("\"theme\""), "the legacy key must not be written back: {encoded}");
        assert!(encoded.contains("terminal_dark_theme"));

        // Idempotent: a second pass changes nothing.
        let before = appearance.clone();
        appearance.migrate_legacy();
        assert_eq!(appearance, before);
    }

    #[test]
    fn migration_never_overwrites_a_slot_the_file_already_carries() {
        // A hand-edited file could hold both shapes. The new fields are the more specific statement of
        // intent, so they win.
        let mut appearance: ProfileAppearance = serde_json::from_str(
            r#"{"theme":"Legacy","terminal_dark_theme":"Explicit Dark"}"#,
        )
        .unwrap();
        appearance.migrate_legacy();
        assert_eq!(appearance.terminal_dark_theme.as_deref(), Some("Explicit Dark"));
        assert_eq!(
            appearance.terminal_light_theme.as_deref(),
            Some("Legacy"),
            "the empty slot still takes the legacy value"
        );
    }

    #[test]
    fn an_unmigrated_legacy_value_does_not_read_as_an_override() {
        // `is_empty` gates serialization and the "does this profile override anything" question. Counting
        // `legacy_theme` would report an override that no slot actually holds.
        let appearance: ProfileAppearance = serde_json::from_str(r#"{"theme":"Legacy"}"#).unwrap();
        assert!(appearance.is_empty());
    }

    #[test]
    fn a_terminal_override_is_selected_by_the_appearance_in_effect() {
        let appearance = ProfileAppearance {
            terminal_dark_theme: Some("Dark One".to_owned()),
            terminal_light_theme: None,
            ..Default::default()
        };
        assert_eq!(appearance.terminal_theme(true), Some("Dark One"));
        assert_eq!(
            appearance.terminal_theme(false),
            None,
            "an unset half inherits rather than borrowing the other half"
        );
    }

    #[test]
    fn env_key_validation_rejects_every_injection_shape() {
        assert!(is_valid_env_key("AWS_PROFILE"));
        assert!(is_valid_env_key("_private"));
        assert!(is_valid_env_key("X1"));

        // A name is interpolated on the left of `NAME=value`, so these are not merely odd names:
        // each one ends the assignment and starts a command in the remote bootstrap script.
        assert!(!is_valid_env_key("X\nid > /tmp/pwned\nY"), "newline splits the line");
        assert!(!is_valid_env_key("X;id"), "`;` chains a command inline");
        assert!(!is_valid_env_key("X$(id)"), "command substitution");
        assert!(!is_valid_env_key("X`id`"), "backtick substitution");
        assert!(!is_valid_env_key("X Y"), "a space makes two words");
        assert!(!is_valid_env_key("X=Y"), "an embedded `=` shifts the assignment");
        // Shape rules.
        assert!(!is_valid_env_key(""), "empty is not a name");
        assert!(!is_valid_env_key("1ST"), "a name cannot start with a digit");
        assert!(!is_valid_env_key("PÄTH"), "non-ASCII is outside the accepted set");
    }

    #[test]
    fn the_consent_key_distinguishes_ports_but_the_ssh_destination_does_not() {
        let base = RemoteTarget {
            host: "host".to_owned(),
            user: Some("deploy".to_owned()),
            port: None,
        };
        let forwarded = RemoteTarget {
            port: Some(2222),
            ..base.clone()
        };
        // The ssh destination argument never carries the port (it travels as `-p`).
        assert_eq!(base.destination(), "deploy@host");
        assert_eq!(forwarded.destination(), "deploy@host");
        // But consent must: a forwarded port is very often a different machine, and consenting to
        // write into one account's home must not authorize another.
        assert_ne!(
            base.consent_key(),
            forwarded.consent_key(),
            "a different port must be asked about separately"
        );
        assert_eq!(forwarded.consent_key(), "deploy@host:2222");
        // Per account as well: the install lands in that account's $HOME.
        let other_user = RemoteTarget {
            user: Some("root".to_owned()),
            ..base.clone()
        };
        assert_ne!(base.consent_key(), other_user.consent_key());
    }

    #[test]
    fn remote_targets_that_could_pass_as_ssh_options_are_rejected() {
        assert!(RemoteTarget::new("host.example.com".to_owned()).is_usable());
        assert!(
            RemoteTarget {
                host: "host".to_owned(),
                user: Some("deploy".to_owned()),
                port: Some(22),
            }
            .is_usable()
        );

        // Each of these would reach ssh's argv; a leading dash makes it an option, and
        // `-oProxyCommand=` in that position executes locally at connect time.
        for host in [
            "-oProxyCommand=touch /tmp/pwned",
            "--",
            "-p2222",
            "",
            "host with spaces",
            "host\twith\ttabs",
        ] {
            assert!(
                !RemoteTarget::new(host.to_owned()).is_usable(),
                "host {host:?} must be rejected"
            );
        }
        // The user half reaches the same argv entry, so it needs the same guard.
        assert!(
            !RemoteTarget {
                host: "host".to_owned(),
                user: Some("-oProxyCommand=x".to_owned()),
                port: None,
            }
            .is_usable()
        );
    }

    #[test]
    fn groups_own_independent_tab_lists_after_split() {
        let first = TabItem::terminal(Uuid::new_v4(), "shell 1");
        let mut layout = LayoutNode::group(first);
        let original_group = match &layout {
            LayoutNode::Group { group } => group.id,
            _ => unreachable!(),
        };
        let second = TabItem::terminal(Uuid::new_v4(), "shell 2");
        let second_group = layout
            .split_group(original_group, SplitAxis::Horizontal, second)
            .expect("group should split");

        assert_eq!(
            layout.find_group_mut(original_group).unwrap().items.len(),
            1
        );
        assert_eq!(layout.find_group_mut(second_group).unwrap().items.len(), 1);
    }

    #[test]
    fn terminal_items_can_be_found_across_split_groups() {
        let first = TabItem::terminal(Uuid::new_v4(), "shell 1");
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        let session_id = Uuid::new_v4();
        let second = TabItem::terminal(session_id, "shell 2");
        let second_item_id = second.id;
        let second_group = layout
            .split_group(original_group, SplitAxis::Horizontal, second)
            .unwrap();

        assert_eq!(
            layout.find_terminal_item(session_id),
            Some((second_group, second_item_id))
        );
    }

    #[test]
    fn terminal_titles_update_across_split_groups_by_session_id() {
        let first_session_id = Uuid::new_v4();
        let second_session_id = Uuid::new_v4();
        let mut layout = LayoutNode::group(TabItem::terminal(first_session_id, "shell 1"));
        let first_group_id = layout.first_group_id();
        let second_group_id = layout
            .split_group(
                first_group_id,
                SplitAxis::Horizontal,
                TabItem::terminal(second_session_id, "shell 2"),
            )
            .unwrap();

        assert!(layout.update_terminal_title(second_session_id, "OSC title"));
        assert!(!layout.update_terminal_title(second_session_id, "OSC title"));
        assert_eq!(
            layout.find_group(first_group_id).unwrap().items[0].title,
            "shell 1"
        );
        assert_eq!(
            layout.find_group(second_group_id).unwrap().items[0].title,
            "OSC title"
        );
    }

    #[test]
    fn custom_title_overrides_process_title_and_survives_osc_updates() {
        let session_id = Uuid::new_v4();
        let item = TabItem::terminal(session_id, "process title");
        let item_id = item.id;
        let mut layout = LayoutNode::group(item);
        let group_id = layout.first_group_id();

        // No override yet: display falls back to the process title.
        assert_eq!(
            layout.find_group(group_id).unwrap().items[0].display_title(),
            "process title"
        );

        // Setting a custom title wins over the process title.
        assert!(layout.set_custom_title(group_id, item_id, Some("My Tab".to_owned())));
        assert!(!layout.set_custom_title(group_id, item_id, Some("My Tab".to_owned())));
        assert_eq!(
            layout.find_group(group_id).unwrap().items[0].display_title(),
            "My Tab"
        );

        // An OSC title update changes the process title but never the custom override.
        assert!(layout.update_terminal_title(session_id, "new process title"));
        let item = &layout.find_group(group_id).unwrap().items[0];
        assert_eq!(item.title, "new process title");
        assert_eq!(item.custom_title.as_deref(), Some("My Tab"));
        assert_eq!(item.display_title(), "My Tab");

        // Clearing the override restores the (now-updated) process title.
        assert!(layout.set_custom_title(group_id, item_id, None));
        assert_eq!(
            layout.find_group(group_id).unwrap().items[0].display_title(),
            "new process title"
        );
    }

    #[test]
    fn terminal_display_title_reflects_custom_title_by_session_id() {
        let session_id = Uuid::new_v4();
        let item = TabItem::terminal(session_id, "process title");
        let item_id = item.id;
        let mut layout = LayoutNode::group(item);
        let group_id = layout.first_group_id();

        // Falls back to the process title when no override is set.
        assert_eq!(
            layout.terminal_display_title(session_id),
            Some("process title")
        );
        // An unknown session id has no tab.
        assert_eq!(layout.terminal_display_title(Uuid::new_v4()), None);

        // A custom title wins, keyed off the session id (what the sidebar uses).
        layout.set_custom_title(group_id, item_id, Some("Custom".to_owned()));
        assert_eq!(layout.terminal_display_title(session_id), Some("Custom"));
    }

    #[test]
    fn closing_the_active_item_activates_a_neighbor() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_id = first.id;
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_id = second.id;
        let mut group = TabGroup::new(first);
        group.add_item(second);

        group.close_item(second_id);

        assert_eq!(group.active_item_id, Some(first_id));
    }

    #[test]
    fn closing_the_last_item_in_a_split_collapses_its_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_item_id = second.id;
        let second_group = layout
            .split_group(original_group, SplitAxis::Vertical, second)
            .unwrap();

        let removed = layout.close_item(second_group, second_item_id).unwrap();

        assert_eq!(removed.title, "two");
        assert!(layout.find_group(second_group).is_none());
        assert_eq!(layout.first_group_id(), original_group);
    }

    #[test]
    fn closing_the_only_root_item_keeps_an_empty_group_for_new_tabs() {
        let only = TabItem::terminal(Uuid::new_v4(), "one");
        let item_id = only.id;
        let mut layout = LayoutNode::group(only);
        let group_id = layout.first_group_id();

        layout.close_item(group_id, item_id).unwrap();

        let group = layout.find_group(group_id).unwrap();
        assert!(group.items.is_empty());
        assert_eq!(group.active_item_id, None);
    }

    #[test]
    fn directional_splits_place_groups_on_the_requested_side() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let new_group = layout
            .split_group_direction(original_group, Direction::Left, second)
            .unwrap();

        assert_eq!(layout.top_left_group_id(), new_group);
        assert_eq!(layout.top_right_group_id(), original_group);
        assert_eq!(
            layout.neighbor_group_id(new_group, Direction::Right),
            Some(original_group)
        );
        assert_eq!(
            layout.neighbor_group_id(original_group, Direction::Left),
            Some(new_group)
        );
    }

    #[test]
    fn moving_a_tab_to_another_group_prunes_an_empty_source_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_item_id = first.id;
        let mut layout = LayoutNode::group(first);
        let first_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_group = layout
            .split_group_direction(first_group, Direction::Right, second)
            .unwrap();

        assert_eq!(
            layout.move_item(first_group, first_item_id, second_group),
            Some(second_group)
        );
        assert!(layout.find_group(first_group).is_none());
        assert_eq!(layout.find_group(second_group).unwrap().items.len(), 2);
    }

    #[test]
    fn moving_a_tab_at_an_index_reorders_within_its_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_item_id = first.id;
        let mut layout = LayoutNode::group(first);
        let group_id = layout.first_group_id();
        let group = layout.find_group_mut(group_id).unwrap();
        group.add_item(TabItem::terminal(Uuid::new_v4(), "two"));
        group.add_item(TabItem::terminal(Uuid::new_v4(), "three"));

        assert_eq!(
            layout.move_item_at(group_id, first_item_id, group_id, 3),
            Some(group_id)
        );

        let group = layout.find_group(group_id).unwrap();
        assert_eq!(
            group
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["two", "three", "one"]
        );
        assert_eq!(group.active_item_id, Some(first_item_id));

        assert_eq!(
            layout.move_item_at(group_id, first_item_id, group_id, 0),
            Some(group_id)
        );
        assert_eq!(
            layout
                .find_group(group_id)
                .unwrap()
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
    }

    #[test]
    fn moving_a_tab_at_an_index_inserts_into_another_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_item_id = first.id;
        let mut layout = LayoutNode::group(first);
        let first_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_group = layout
            .split_group_direction(first_group, Direction::Right, second)
            .unwrap();
        layout
            .find_group_mut(second_group)
            .unwrap()
            .add_item(TabItem::terminal(Uuid::new_v4(), "three"));

        assert_eq!(
            layout.move_item_at(first_group, first_item_id, second_group, 1),
            Some(second_group)
        );

        assert!(layout.find_group(first_group).is_none());
        let group = layout.find_group(second_group).unwrap();
        assert_eq!(
            group
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["two", "one", "three"]
        );
    }

    #[test]
    fn moving_a_tab_to_an_edge_creates_a_group_at_that_edge() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_item_id = second.id;
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        layout
            .find_group_mut(original_group)
            .unwrap()
            .add_item(second);

        let new_group = layout
            .move_item_to_new_group(
                original_group,
                second_item_id,
                original_group,
                Direction::Up,
            )
            .unwrap();

        assert_eq!(layout.top_left_group_id(), new_group);
        assert_eq!(layout.find_group(new_group).unwrap().items.len(), 1);
        assert_eq!(layout.find_group(original_group).unwrap().items.len(), 1);
    }

    #[test]
    fn a_sole_tab_cannot_split_its_own_group_by_moving_it() {
        let only = TabItem::terminal(Uuid::new_v4(), "one");
        let item_id = only.id;
        let mut layout = LayoutNode::group(only);
        let group_id = layout.first_group_id();

        assert_eq!(
            layout.move_item_to_new_group(group_id, item_id, group_id, Direction::Down),
            None
        );
        assert_eq!(layout.find_group(group_id).unwrap().items.len(), 1);
    }

    #[test]
    fn nested_splits_have_distinct_ids_and_resize_independently() {
        // Build: root Horizontal split, whose `first` child is itself a Vertical split. The inner
        // split's first group is also the root's first group, so identifying a split by its first
        // group would make the two collide. Each must instead have its own id.
        let top_left = TabItem::terminal(Uuid::new_v4(), "top-left");
        let mut layout = LayoutNode::group(top_left);
        let left_group = layout.first_group_id();

        // Split to the right first -> outer Horizontal split (left column / right).
        layout
            .split_group_direction(
                left_group,
                Direction::Right,
                TabItem::terminal(Uuid::new_v4(), "right"),
            )
            .unwrap();
        // Then split the left group downward -> inner Vertical split nested in the outer's `first`.
        layout
            .split_group_direction(
                left_group,
                Direction::Down,
                TabItem::terminal(Uuid::new_v4(), "bottom-left"),
            )
            .unwrap();

        let (outer_id, inner_id) = match &layout {
            LayoutNode::Split {
                id: outer_id,
                axis: SplitAxis::Horizontal,
                first,
                ..
            } => match first.as_ref() {
                LayoutNode::Split {
                    id: inner_id,
                    axis: SplitAxis::Vertical,
                    ..
                } => (*outer_id, *inner_id),
                other => panic!("expected inner vertical split, got {other:?}"),
            },
            other => panic!("expected outer horizontal split, got {other:?}"),
        };

        assert_ne!(outer_id, inner_id, "nested splits must not share an id");
        assert_eq!(layout.split_axis(outer_id), Some(SplitAxis::Horizontal));
        assert_eq!(layout.split_axis(inner_id), Some(SplitAxis::Vertical));

        // Resizing the inner split must not touch the outer split's ratio, and vice versa.
        assert!(layout.set_split_ratio(inner_id, 0.3));
        assert_eq!(layout.split_ratio(inner_id), Some(0.3));
        assert_eq!(layout.split_ratio(outer_id), Some(0.5));

        assert!(layout.set_split_ratio(outer_id, 0.7));
        assert_eq!(layout.split_ratio(outer_id), Some(0.7));
        assert_eq!(layout.split_ratio(inner_id), Some(0.3));
    }
}

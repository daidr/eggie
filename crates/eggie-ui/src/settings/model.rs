use super::*;

pub(crate) const DEFAULT_FONT_FAMILY: &str = "Menlo";
pub(crate) const DEFAULT_FONT_SIZE: f32 = 14.;
pub(crate) const MIN_FONT_SIZE: f32 = 8.;
pub(crate) const MAX_FONT_SIZE: f32 = 32.;
pub(crate) const DEFAULT_TERMINAL_PADDING_X: f32 = 2.;
pub(crate) const DEFAULT_TERMINAL_PADDING_Y: f32 = 2.;
pub(crate) const MIN_TERMINAL_PADDING: f32 = 0.;
pub(crate) const MAX_TERMINAL_PADDING: f32 = 64.;
pub(crate) const DEFAULT_MINIMUM_CONTRAST: f32 = 1.;
pub(crate) const MIN_MINIMUM_CONTRAST: f32 = 1.;
pub(crate) const MAX_MINIMUM_CONTRAST: f32 = 21.;
pub(crate) const DEFAULT_PROGRESS_COMPLETE_TIMEOUT_SECS: u32 = 5;
pub(crate) const DEFAULT_PROGRESS_STALE_TIMEOUT_SECS: u32 = 60;
pub(crate) const MIN_PROGRESS_TIMEOUT_SECS: u32 = 1;
pub(crate) const MAX_PROGRESS_TIMEOUT_SECS: u32 = 3_600;
pub(crate) const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
pub(crate) const MIN_SCROLLBACK_LINES: usize = 0;
pub(crate) const MAX_SCROLLBACK_LINES: usize = 1_000_000;
pub(crate) const SCROLLBACK_LINES_STEP: i64 = 1_000;

pub(crate) fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_true(value: &bool) -> bool {
    *value
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) fn default_scrollback_lines() -> usize {
    DEFAULT_SCROLLBACK_LINES
}

pub(crate) fn default_dark_theme() -> String {
    DEFAULT_DARK_THEME.to_owned()
}

pub(crate) fn default_light_theme() -> String {
    DEFAULT_LIGHT_THEME.to_owned()
}

pub(crate) fn is_default_dark_theme(value: &str) -> bool {
    value == DEFAULT_DARK_THEME
}

pub(crate) fn is_default_light_theme(value: &str) -> bool {
    value == DEFAULT_LIGHT_THEME
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_default_scrollback(value: &usize) -> bool {
    *value == DEFAULT_SCROLLBACK_LINES
}

pub(crate) fn default_thicken_strength() -> u8 {
    255
}

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) fn is_full_strength(value: &u8) -> bool {
    *value == 255
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThemeMode {
    Dark,
    Light,
    #[default]
    System,
}

impl ThemeMode {
    pub(crate) const ALL: [Self; 3] = [Self::System, Self::Dark, Self::Light];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Dark => "Dark",
            Self::Light => "Light",
            Self::System => "System",
        }
    }
}

/// Where the terminal's colors come from, independently of the interface theme.
///
/// The interface theme always decides the app's own chrome. This chooses whether the terminal cells
/// ride along with it or use their own dark/light pair — which is what makes "dark interface, light
/// terminal" (and any other combination) expressible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalThemeSource {
    /// Terminal cells use whichever interface theme is in effect. The default, and what Eggie did
    /// before the two layers were separable.
    #[default]
    FollowInterface,
    /// Terminal cells use [`AppSettings::terminal_dark_theme`] / `terminal_light_theme`, selected by
    /// the same dark/light decision that drives the interface.
    Custom,
}

impl TerminalThemeSource {
    pub(crate) const ALL: [Self; 2] = [Self::FollowInterface, Self::Custom];

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn is_default(&self) -> bool {
        matches!(self, Self::FollowInterface)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Language {
    #[default]
    English,
    SimplifiedChinese,
}

impl Language {
    pub(crate) const ALL: [Self; 2] = [Self::English, Self::SimplifiedChinese];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::SimplifiedChinese => "简体中文",
        }
    }
}

/// How the terminal bell (BEL / `\a`) is surfaced to the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BellMode {
    /// Ignore the bell entirely.
    Silent,
    /// Flash the tab (and sidebar row) of the ringing session.
    #[default]
    Flash,
    /// Play the system bell sound.
    Sound,
    /// Flash and play the sound.
    FlashAndSound,
}

impl BellMode {
    pub(crate) const ALL: [Self; 4] = [Self::Silent, Self::Flash, Self::Sound, Self::FlashAndSound];

    /// A stable ascii slug for element ids (not user-facing; display text comes from i18n).
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Flash => "flash",
            Self::Sound => "sound",
            Self::FlashAndSound => "flash-and-sound",
        }
    }

    pub(crate) fn plays_sound(self) -> bool {
        matches!(self, Self::Sound | Self::FlashAndSound)
    }

    pub(crate) fn flashes(self) -> bool {
        matches!(self, Self::Flash | Self::FlashAndSound)
    }
}

/// Which application opens a directory from the right-sidebar "open" button. Persisted so the last
/// choice becomes the button's default action. `AppWindow`-style custom openers are future work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OpenWith {
    #[default]
    Finder,
    VsCode,
}

impl OpenWith {
    pub(crate) const ALL: [Self; 2] = [Self::Finder, Self::VsCode];

    /// A stable ascii slug for element ids and i18n lookup (not user-facing on its own).
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Finder => "finder",
            Self::VsCode => "vscode",
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        matches!(self, Self::Finder)
    }
}

/// Which release channel the updater follows. `Stable` sees only final releases;
/// `Beta` additionally sees prereleases (alpha/beta/rc). Maps to the feed file
/// the client fetches (`stable.json` vs `beta.json`); see `default_feed_url`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

impl UpdateChannel {
    pub(crate) const ALL: [Self; 2] = [Self::Stable, Self::Beta];

    /// A stable ascii slug for element ids, i18n lookup, and the feed filename.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        matches!(self, Self::Stable)
    }
}

/// The default cursor shape. A running program can still override this at runtime via DECSCUSR
/// (`CSI Ps SP q`); this only sets the shape used when the program hasn't requested one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CursorShapeSetting {
    #[default]
    Block,
    Bar,
    Underline,
    BlockHollow,
}

impl CursorShapeSetting {
    pub(crate) const ALL: [Self; 4] = [Self::Block, Self::Bar, Self::Underline, Self::BlockHollow];

    /// A stable ascii slug for element ids (not user-facing; display text comes from i18n).
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bar => "bar",
            Self::Underline => "underline",
            Self::BlockHollow => "block-hollow",
        }
    }

    pub(crate) fn to_protocol(self) -> eggie_protocol::TerminalCursorShape {
        use eggie_protocol::TerminalCursorShape;
        match self {
            Self::Block => TerminalCursorShape::Block,
            Self::Bar => TerminalCursorShape::Beam,
            Self::Underline => TerminalCursorShape::Underline,
            Self::BlockHollow => TerminalCursorShape::HollowBlock,
        }
    }
}

/// Whether the cursor blinks. `Program` follows what the running program requests (DECSCUSR /
/// DEC Mode 12); `On`/`Off` force blinking regardless of the program (matching Ghostty's
/// `cursor-style-blink`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CursorBlink {
    #[default]
    Program,
    On,
    Off,
}

impl CursorBlink {
    pub(crate) const ALL: [Self; 3] = [Self::Program, Self::On, Self::Off];

    /// A stable ascii slug for element ids (not user-facing; display text comes from i18n).
    pub(crate) fn slug(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    /// Resolve whether the cursor should blink, given the shape's program-reported blinking bit.
    pub(crate) fn resolve(self, program_blinking: bool) -> bool {
        match self {
            Self::Program => program_blinking,
            Self::On => true,
            Self::Off => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) language: Language,
    pub(crate) theme_mode: ThemeMode,
    /// Interface theme used when the resolved appearance is dark. Drives the app's own chrome
    /// ([`UiColors::from_theme`]) and, when [`Self::terminal_theme_source`] is
    /// [`TerminalThemeSource::FollowInterface`], the terminal cells too.
    ///
    /// The `dark_theme` alias is what this field was called when one theme drove both layers; it is
    /// kept so an existing `settings.json` does not silently lose the user's choice.
    #[serde(alias = "dark_theme")]
    pub(crate) interface_dark_theme: String,
    /// Interface theme used when the resolved appearance is light. See [`Self::interface_dark_theme`].
    #[serde(alias = "light_theme")]
    pub(crate) interface_light_theme: String,
    /// Whether the terminal follows the interface theme or picks from its own pair below.
    #[serde(default, skip_serializing_if = "TerminalThemeSource::is_default")]
    pub(crate) terminal_theme_source: TerminalThemeSource,
    /// Terminal theme for the dark appearance. Only consulted when `terminal_theme_source` is
    /// [`TerminalThemeSource::Custom`]; kept across a switch back to `FollowInterface` so toggling
    /// does not discard the choice.
    #[serde(
        default = "default_dark_theme",
        skip_serializing_if = "is_default_dark_theme"
    )]
    pub(crate) terminal_dark_theme: String,
    /// Terminal theme for the light appearance. See [`Self::terminal_dark_theme`].
    #[serde(
        default = "default_light_theme",
        skip_serializing_if = "is_default_light_theme"
    )]
    pub(crate) terminal_light_theme: String,
    pub(crate) font_family: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) font_family_bold: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) font_family_italic: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) font_family_bold_italic: String,
    #[serde(default, skip_serializing_if = "FontSyntheticStyle::is_all_enabled")]
    pub(crate) font_synthetic_style: FontSyntheticStyle,
    /// Master ligature toggle. When off, text renders per-grapheme (no cross-cell shaping) — the
    /// fast path. When on, contiguous same-style runs are shaped together so programming ligatures
    /// (`->`, `==>`, …) form. Default on, matching Ghostty (`liga`/`calt` default-enabled).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub(crate) ligatures: bool,
    /// Advanced OpenType feature overrides applied on top of the font defaults, HarfBuzz-style
    /// (`ss01`, `+cv01`, `-calt`, `cv02=2`). Empty = font defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) font_features: Vec<String>,
    /// Break shaping runs at the cursor cell so the character under the cursor renders un-ligated
    /// (matching Ghostty `font-shaping-break = cursor`). Default on.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub(crate) font_shaping_break_cursor: bool,
    /// Variable-font axis settings applied to every style, HarfBuzz-style (`wght=200`, `slnt=-10`).
    /// Empty = font defaults.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) font_variations: Vec<String>,
    /// Thicken glyph strokes via macOS font smoothing (matching Ghostty `font-thicken`). Default off.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) font_thicken: bool,
    /// Thickening strength 0–255 when `font_thicken` is on (matching `font-thicken-strength`).
    #[serde(
        default = "default_thicken_strength",
        skip_serializing_if = "is_full_strength"
    )]
    pub(crate) font_thicken_strength: u8,
    /// Per-codepoint-range font overrides, matching Ghostty's `font-codepoint-map`. Each entry is
    /// `U+XXXX-U+YYYY=Family` or `U+XXXX=Family`. Empty = no overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) font_codepoint_map: Vec<String>,
    pub(crate) font_size: f32,
    pub(crate) terminal_padding_x: f32,
    pub(crate) terminal_padding_y: f32,
    pub(crate) minimum_contrast: f32,
    pub(crate) progress_complete_timeout_secs: u32,
    pub(crate) progress_stale_timeout_secs: u32,
    pub(crate) allow_osc_clipboard_read: bool,
    pub(crate) detect_urls: bool,
    pub(crate) copy_on_select: bool,
    /// Whether to check for app updates on launch. The check is silent; results
    /// surface as a sidebar indicator.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub(crate) auto_check_updates: bool,
    /// Which release channel the updater follows. Default `Stable`.
    #[serde(default, skip_serializing_if = "UpdateChannel::is_default")]
    pub(crate) update_channel: UpdateChannel,
    pub(crate) cursor_shape: CursorShapeSetting,
    pub(crate) cursor_blink: CursorBlink,
    pub(crate) bell_mode: BellMode,
    /// Which application the right-sidebar "open directory" button launches. Persisted so the last
    /// choice sticks across restarts.
    #[serde(default, skip_serializing_if = "OpenWith::is_default")]
    pub(crate) open_directory_with: OpenWith,
    #[serde(default, skip_serializing_if = "FontMetricAdjustments::is_empty")]
    pub(crate) font_metrics: FontMetricAdjustments,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) keybindings: std::collections::BTreeMap<String, String>,
    /// Scrollback history depth in lines (`0` disables scrollback). Applies to all live sessions at
    /// runtime, matching Ghostty's `scrollback-limit`.
    #[serde(
        default = "default_scrollback_lines",
        skip_serializing_if = "is_default_scrollback"
    )]
    pub(crate) scrollback_lines: usize,
    /// Shell executable to launch (matching Ghostty's `command`). Empty = fall back to `$SHELL`.
    /// Only takes effect for newly created terminals — the running shell can't be swapped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) shell_program: String,
    /// Custom arguments for the shell. Empty = Eggie's default launch args. Stored as a real argv
    /// (the settings UI edits it as a single space-separated line). New terminals only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) shell_args: Vec<String>,
    /// `path` shell-integration feature: append Eggie's binary directory to the shell PATH so
    /// `eggie +version` and the other CLI actions work inside the terminal (matching Ghostty's
    /// `path` feature). Default on. New terminals only.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub(crate) shell_integration_path: bool,
    /// Hosts the user has agreed Eggie may install its shell integration on, keyed by the profile's
    /// `user@host` destination (per account, because the install lands in that account's home
    /// directory). Asked once per destination; a `false` entry is a remembered refusal, so declining
    /// is not re-asked either. A `BTreeSet`-like map rather than a list so the persisted order is
    /// stable and lookups are cheap.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) remote_install_consent: std::collections::BTreeMap<String, bool>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme_mode: ThemeMode::System,
            interface_dark_theme: DEFAULT_DARK_THEME.to_owned(),
            interface_light_theme: DEFAULT_LIGHT_THEME.to_owned(),
            terminal_theme_source: TerminalThemeSource::default(),
            terminal_dark_theme: DEFAULT_DARK_THEME.to_owned(),
            terminal_light_theme: DEFAULT_LIGHT_THEME.to_owned(),
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            font_family_bold: String::new(),
            font_family_italic: String::new(),
            font_family_bold_italic: String::new(),
            font_synthetic_style: FontSyntheticStyle::default(),
            ligatures: true,
            font_features: Vec::new(),
            font_shaping_break_cursor: true,
            font_variations: Vec::new(),
            font_thicken: false,
            font_thicken_strength: 255,
            font_codepoint_map: Vec::new(),
            font_size: DEFAULT_FONT_SIZE,
            terminal_padding_x: DEFAULT_TERMINAL_PADDING_X,
            terminal_padding_y: DEFAULT_TERMINAL_PADDING_Y,
            minimum_contrast: DEFAULT_MINIMUM_CONTRAST,
            progress_complete_timeout_secs: DEFAULT_PROGRESS_COMPLETE_TIMEOUT_SECS,
            progress_stale_timeout_secs: DEFAULT_PROGRESS_STALE_TIMEOUT_SECS,
            allow_osc_clipboard_read: false,
            detect_urls: true,
            copy_on_select: true,
            auto_check_updates: true,
            update_channel: UpdateChannel::default(),
            cursor_shape: CursorShapeSetting::default(),
            cursor_blink: CursorBlink::default(),
            bell_mode: BellMode::default(),
            open_directory_with: OpenWith::default(),
            font_metrics: FontMetricAdjustments::default(),
            keybindings: std::collections::BTreeMap::new(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            shell_program: String::new(),
            shell_args: Vec::new(),
            shell_integration_path: true,
            remote_install_consent: std::collections::BTreeMap::new(),
        }
    }
}

/// Shell-integration feature toggles, assembled into the comma-separated `EGGIE_SHELL_FEATURES`
/// environment string the daemon injects. Mirrors Ghostty's `shell-integration-features`. The
/// client owns the feature vocabulary; the daemon just forwards the assembled string verbatim.
/// Add a bool field + a token here when a new feature lands — the protocol does not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShellFeatures {
    pub(crate) path: bool,
}

impl ShellFeatures {
    /// Join the enabled feature tokens with commas, in a stable order. Empty when all are off.
    pub(crate) fn to_env_string(self) -> String {
        let mut tokens: Vec<&str> = Vec::new();
        if self.path {
            tokens.push("path");
        }
        tokens.join(",")
    }
}

impl AppSettings {
    /// Snapshot of the shell launch fields for the one-time migration into the built-in Default
    /// profile. Read only when `profiles.json` does not yet exist; afterwards the profile owns these
    /// values and these fields are vestigial (kept so downgrading to an older build still works).
    pub(crate) fn legacy_shell_settings(&self) -> crate::profile_store::LegacyShellSettings {
        crate::profile_store::LegacyShellSettings {
            shell_program: self.shell_program.clone(),
            shell_args: self.shell_args.clone(),
            shell_integration_path: self.shell_integration_path,
        }
    }

    pub(crate) fn normalize(&mut self) {
        let catalog = theme_catalog();
        if catalog.dark_theme(&self.interface_dark_theme).is_none() {
            self.interface_dark_theme = DEFAULT_DARK_THEME.to_owned();
        }
        if catalog.light_theme(&self.interface_light_theme).is_none() {
            self.interface_light_theme = DEFAULT_LIGHT_THEME.to_owned();
        }
        // The terminal pair is validated against the same half of the catalog as the interface pair:
        // a dark slot only accepts a theme that measures dark. That is what lets each picker offer
        // one half of the catalog rather than all of it.
        if catalog.dark_theme(&self.terminal_dark_theme).is_none() {
            self.terminal_dark_theme = DEFAULT_DARK_THEME.to_owned();
        }
        if catalog.light_theme(&self.terminal_light_theme).is_none() {
            self.terminal_light_theme = DEFAULT_LIGHT_THEME.to_owned();
        }
        if self.font_family.trim().is_empty() {
            self.font_family = DEFAULT_FONT_FAMILY.to_owned();
        }
        // Per-style families are optional: a blank (or whitespace-only) value means "fall back to
        // the regular family", so normalize whitespace-only entries to empty rather than to Menlo.
        for family in [
            &mut self.font_family_bold,
            &mut self.font_family_italic,
            &mut self.font_family_bold_italic,
        ] {
            if family.trim().is_empty() {
                family.clear();
            }
        }
        if !self.font_size.is_finite() {
            self.font_size = DEFAULT_FONT_SIZE;
        }
        self.font_size = self.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if !self.terminal_padding_x.is_finite() {
            self.terminal_padding_x = DEFAULT_TERMINAL_PADDING_X;
        }
        if !self.terminal_padding_y.is_finite() {
            self.terminal_padding_y = DEFAULT_TERMINAL_PADDING_Y;
        }
        self.terminal_padding_x = self
            .terminal_padding_x
            .clamp(MIN_TERMINAL_PADDING, MAX_TERMINAL_PADDING);
        self.terminal_padding_y = self
            .terminal_padding_y
            .clamp(MIN_TERMINAL_PADDING, MAX_TERMINAL_PADDING);
        if !self.minimum_contrast.is_finite() {
            self.minimum_contrast = DEFAULT_MINIMUM_CONTRAST;
        }
        self.minimum_contrast = self
            .minimum_contrast
            .clamp(MIN_MINIMUM_CONTRAST, MAX_MINIMUM_CONTRAST);
        self.progress_complete_timeout_secs = self
            .progress_complete_timeout_secs
            .clamp(MIN_PROGRESS_TIMEOUT_SECS, MAX_PROGRESS_TIMEOUT_SECS);
        self.progress_stale_timeout_secs = self
            .progress_stale_timeout_secs
            .clamp(MIN_PROGRESS_TIMEOUT_SECS, MAX_PROGRESS_TIMEOUT_SECS);
        self.keybindings.retain(|id, keystroke| {
            crate::keybindings::spec_by_id(id).is_some()
                && gpui::Keystroke::parse(keystroke).is_ok()
                && crate::keybindings::default_keystroke(id) != Some(keystroke.as_str())
        });
        self.font_metrics.normalize();
        self.scrollback_lines = self
            .scrollback_lines
            .clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES);
        // Whitespace-only shell path means "use $SHELL"; normalize it to empty so the daemon's
        // fallback kicks in. Drop blank argument entries so an empty UI line never becomes an arg.
        if self.shell_program.trim().is_empty() {
            self.shell_program.clear();
        } else {
            self.shell_program = self.shell_program.trim().to_owned();
        }
        self.shell_args.retain(|arg| !arg.trim().is_empty());
    }

    /// Build the resolved font families, each per-style slot falling back to the regular family
    /// when its dedicated family is unset (matching Ghostty's `finalize` behavior).
    pub(crate) fn resolved_font_families(&self) -> ResolvedFontFamilies {
        let regular: std::sync::Arc<str> = std::sync::Arc::from(self.font_family.as_str());
        let resolve = |name: &str| -> std::sync::Arc<str> {
            if name.trim().is_empty() {
                std::sync::Arc::clone(&regular)
            } else {
                std::sync::Arc::from(name)
            }
        };
        ResolvedFontFamilies {
            bold: resolve(&self.font_family_bold),
            italic: resolve(&self.font_family_italic),
            bold_italic: resolve(&self.font_family_bold_italic),
            regular,
            synthetic: self.font_synthetic_style,
        }
    }

    /// The effective OpenType features to apply during shaping. When the master `ligatures` toggle
    /// is off, `liga`/`calt`/`dlig` are force-disabled up front; user `font_features` overrides are
    /// then parsed and appended (so an explicit `+liga` could re-enable it, matching Ghostty's
    /// "later wins" ordering). Invalid entries are dropped.
    pub(crate) fn resolved_font_features(&self) -> Vec<FontFeature> {
        let mut features = Vec::new();
        if !self.ligatures {
            for tag in ["liga", "calt", "dlig"] {
                let bytes = tag.as_bytes();
                features.push(FontFeature {
                    tag: [bytes[0], bytes[1], bytes[2], bytes[3]],
                    value: 0,
                });
            }
        }
        features.extend(
            self.font_features
                .iter()
                .filter_map(|feature| FontFeature::parse(feature)),
        );
        features
    }

    /// Parse the variable-font axis settings, dropping any malformed entries.
    pub(crate) fn resolved_font_variations(&self) -> Vec<FontVariation> {
        self.font_variations
            .iter()
            .filter_map(|variation| FontVariation::parse(variation))
            .collect()
    }

    /// Parse the codepoint→family map, dropping malformed entries.
    pub(crate) fn resolved_codepoint_map(&self) -> Vec<CodepointMapEntry> {
        self.font_codepoint_map
            .iter()
            .filter_map(|entry| CodepointMapEntry::parse(entry))
            .collect()
    }

    /// Whether the dark half of every theme pair is in effect. The single dark/light decision both
    /// layers share: `theme_mode` pins it, or `System` defers to the OS appearance.
    pub(crate) fn use_dark(&self, system_is_dark: bool) -> bool {
        match self.theme_mode {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => system_is_dark,
        }
    }

    /// The theme driving the app's own chrome. Also the terminal's theme when
    /// [`TerminalThemeSource::FollowInterface`] is set — see [`Self::terminal_theme`].
    pub(crate) fn interface_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        Self::pick_theme(
            self.use_dark(system_is_dark),
            &self.interface_dark_theme,
            &self.interface_light_theme,
        )
    }

    /// The theme driving terminal cells. `FollowInterface` delegates to [`Self::interface_theme`], so
    /// the two layers stay identical unless the user opts into the custom pair.
    pub(crate) fn terminal_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        match self.terminal_theme_source {
            TerminalThemeSource::FollowInterface => self.interface_theme(system_is_dark),
            TerminalThemeSource::Custom => Self::pick_theme(
                self.use_dark(system_is_dark),
                &self.terminal_dark_theme,
                &self.terminal_light_theme,
            ),
        }
    }

    /// Resolve one half of a dark/light theme pair against the catalog, falling back to the built-in
    /// default for that half (and finally to the catalog's first entry, which cannot be empty).
    pub(crate) fn pick_theme(use_dark: bool, dark: &str, light: &str) -> &'static TerminalTheme {
        let catalog = theme_catalog();
        if use_dark {
            catalog
                .dark_theme(dark)
                .or_else(|| catalog.dark_theme(DEFAULT_DARK_THEME))
                .unwrap_or_else(|| catalog.fallback_theme(true))
        } else {
            catalog
                .light_theme(light)
                .or_else(|| catalog.light_theme(DEFAULT_LIGHT_THEME))
                .unwrap_or_else(|| catalog.fallback_theme(false))
        }
    }

    /// Switch the terminal onto its own theme pair, seeded from whatever the interface currently
    /// specifies so the toggle itself changes nothing on screen. Without this, opting into `Custom`
    /// would jump the terminal to the built-in defaults and read as a bug.
    ///
    /// Lives here rather than in the settings window so the seeding is unit-testable.
    pub(crate) fn use_custom_terminal_themes(&mut self) {
        self.terminal_theme_source = TerminalThemeSource::Custom;
        self.terminal_dark_theme = self.interface_dark_theme.clone();
        self.terminal_light_theme = self.interface_light_theme.clone();
    }
}

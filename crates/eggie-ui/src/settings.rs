use gpui::Context;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Command, sync::OnceLock};

include!(concat!(env!("OUT_DIR"), "/ghostty_themes.rs"));

pub(crate) const DEFAULT_DARK_THEME: &str = "Builtin Dark";
pub(crate) const DEFAULT_LIGHT_THEME: &str = "Builtin Light";
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

fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_true(value: &bool) -> bool {
    *value
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn default_scrollback_lines() -> usize {
    DEFAULT_SCROLLBACK_LINES
}

fn default_dark_theme() -> String {
    DEFAULT_DARK_THEME.to_owned()
}

fn default_light_theme() -> String {
    DEFAULT_LIGHT_THEME.to_owned()
}

fn is_default_dark_theme(value: &str) -> bool {
    value == DEFAULT_DARK_THEME
}

fn is_default_light_theme(value: &str) -> bool {
    value == DEFAULT_LIGHT_THEME
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_default_scrollback(value: &usize) -> bool {
    *value == DEFAULT_SCROLLBACK_LINES
}

fn default_thicken_strength() -> u8 {
    255
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_full_strength(value: &u8) -> bool {
    *value == 255
}

/// A parsed variable-font axis setting, matching Ghostty's `font-variation` syntax `id=value`
/// where `id` is a four-character axis tag (`wght`, `slnt`, `wdth`, `opsz`, …) and `value` a float.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FontVariation {
    pub(crate) tag: [u8; 4],
    pub(crate) value: f32,
}

// Stored in the glyph atlas cache key (`GlyphKey`), which must be `Hash + Eq`. `value` only comes
// from `parse`, which rejects non-finite input, so hashing/eq via the bit pattern is sound.
impl Eq for FontVariation {}

impl std::hash::Hash for FontVariation {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.value.to_bits().hash(state);
    }
}

impl FontVariation {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (tag, value) = text.split_once('=')?;
        let tag = tag.trim();
        let bytes = tag.as_bytes();
        if bytes.len() != 4 || !bytes.iter().all(u8::is_ascii_graphic) {
            return None;
        }
        let value: f32 = value.trim().parse().ok()?;
        if !value.is_finite() {
            return None;
        }
        Some(Self {
            tag: [bytes[0], bytes[1], bytes[2], bytes[3]],
            value,
        })
    }
}

/// A parsed `font-codepoint-map` entry: a Unicode codepoint range mapped to a font family. Matches
/// Ghostty's syntax `U+XXXX-U+YYYY=Family` (or a single `U+XXXX=Family`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CodepointMapEntry {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) family: String,
}

impl CodepointMapEntry {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (range, family) = text.split_once('=')?;
        let family = family.trim();
        if family.is_empty() {
            return None;
        }
        let range = range.trim();
        let parse_cp = |token: &str| -> Option<u32> {
            let hex = token.trim().strip_prefix("U+").or_else(|| token.trim().strip_prefix("u+"))?;
            u32::from_str_radix(hex, 16).ok().filter(|cp| char::from_u32(*cp).is_some())
        };
        let (start, end) = match range.split_once('-') {
            Some((lo, hi)) => (parse_cp(lo)?, parse_cp(hi)?),
            None => {
                let cp = parse_cp(range)?;
                (cp, cp)
            }
        };
        if start > end {
            return None;
        }
        Some(Self {
            start,
            end,
            family: family.to_owned(),
        })
    }

    pub(crate) fn contains(&self, codepoint: u32) -> bool {
        (self.start..=self.end).contains(&codepoint)
    }
}

/// A parsed OpenType feature override, matching Ghostty's `font-feature` / HarfBuzz syntax.
/// Accepts `feat`, `+feat`, `feat on`, `feat=1` (enable); `-feat`, `feat off`, `feat=0` (disable);
/// and `feat=N` for a specific selector value. The tag is exactly four ASCII characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FontFeature {
    /// The four-byte OpenType feature tag (e.g. `liga`, `calt`, `ss01`).
    pub(crate) tag: [u8; 4],
    /// The selector value: 0 disables, 1 enables, >1 selects an alternate.
    pub(crate) value: u32,
}

impl FontFeature {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Split an optional `=value` or trailing ` on`/` off`, and a leading `+`/`-`.
        let (mut name, mut value): (&str, u32) = (trimmed, 1);
        if let Some((lhs, rhs)) = trimmed.split_once('=') {
            name = lhs.trim();
            value = rhs.trim().parse().ok()?;
        } else if let Some(rest) = trimmed.strip_suffix(" on").map(str::trim) {
            name = rest;
            value = 1;
        } else if let Some(rest) = trimmed.strip_suffix(" off").map(str::trim) {
            name = rest;
            value = 0;
        }
        if let Some(rest) = name.strip_prefix('+') {
            name = rest.trim();
            value = 1;
        } else if let Some(rest) = name.strip_prefix('-') {
            name = rest.trim();
            value = 0;
        }
        // Tags are commonly quoted in CSS-style syntax; strip a matching pair.
        let name = name.trim_matches(|c| c == '"' || c == '\'');
        let bytes = name.as_bytes();
        if bytes.len() != 4 || !bytes.iter().all(u8::is_ascii_graphic) {
            return None;
        }
        Some(Self {
            tag: [bytes[0], bytes[1], bytes[2], bytes[3]],
            value,
        })
    }
}

/// A single metric adjustment, matching Ghostty's `MetricModifier` (`src/font/Metrics.zig`). A
/// value is stored as a human-editable string in `settings.json` (`""` / absent means "no change")
/// and parsed into this enum. `Percent` scales the base metric (`"20%"` → ×1.2, `"-20%"` → ×0.8);
/// `Absolute` is a signed pixel delta (`"2"` → +2px, `"-1"` → −1px). Both are *deltas* on the
/// font-derived base metric, never absolute replacements.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MetricModifier {
    /// Multiplier around 1.0 (e.g. `1.2` for `+20%`). Clamped so the metric never goes negative.
    Percent(f64),
    /// Signed pixel delta added to the base metric.
    Absolute(i32),
}

// `MetricModifier` is stored in the glyph atlas cache key (`GlyphKey`), which must be `Hash + Eq`.
// The `Percent` variant holds an `f64`, so hash/eq go through the bit pattern. Values only ever come
// from `parse`, which rejects non-finite input, so equality is reflexive (no NaN) and this is sound.
impl Eq for MetricModifier {}

impl std::hash::Hash for MetricModifier {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Percent(value) => {
                0u8.hash(state);
                value.to_bits().hash(state);
            }
            Self::Absolute(value) => {
                1u8.hash(state);
                value.hash(state);
            }
        }
    }
}

impl MetricModifier {
    /// Parse the on-disk string form. A trailing `%` is a percent adjustment; otherwise a signed
    /// integer pixel delta. Returns `None` for empty/whitespace or unparseable input so `normalize`
    /// can drop bad entries instead of persisting them.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(percent) = trimmed.strip_suffix('%') {
            let value: f64 = percent.trim().parse().ok()?;
            if !value.is_finite() {
                return None;
            }
            // Ghostty clamps `≤ -100%` to a 0 multiplier (metric fully collapses, then Minimums
            // re-clamps where required). Mirror that: 1 + value/100, floored at 0.
            return Some(Self::Percent((1.0 + value / 100.0).max(0.0)));
        }
        let value: i32 = trimmed.parse().ok()?;
        Some(Self::Absolute(value))
    }

    /// Apply this modifier to a base metric value (in pixels). Percent scales; absolute adds.
    pub(crate) fn apply(self, base: f32) -> f32 {
        match self {
            Self::Percent(multiplier) => base * multiplier as f32,
            Self::Absolute(delta) => base + delta as f32,
        }
    }

    /// Convert to device space for a given `scale_factor`. `Percent` is scale-independent (a pure
    /// multiplier) and passes through; `Absolute` is a logical-pixel delta, so it is scaled to
    /// device pixels. This lets a modifier be baked into a device-space cache key (`GlyphKey`) and
    /// applied later without needing the scale factor again.
    pub(crate) fn prescale(self, scale_factor: f32) -> Self {
        match self {
            Self::Percent(_) => self,
            Self::Absolute(delta) => Self::Absolute((delta as f32 * scale_factor).round() as i32),
        }
    }
}

/// Pre-parsed, `Copy` form of [`FontMetricAdjustments`] for cheap threading through the render
/// pipeline (and into cache keys, which need value equality). Each field is `None` when the
/// corresponding setting is unset or invalid.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ResolvedMetricAdjustments {
    pub(crate) cell_width: Option<MetricModifier>,
    pub(crate) cell_height: Option<MetricModifier>,
    pub(crate) font_baseline: Option<MetricModifier>,
    pub(crate) underline_position: Option<MetricModifier>,
    pub(crate) underline_thickness: Option<MetricModifier>,
    pub(crate) strikethrough_position: Option<MetricModifier>,
    pub(crate) strikethrough_thickness: Option<MetricModifier>,
    pub(crate) cursor_thickness: Option<MetricModifier>,
    pub(crate) box_thickness: Option<MetricModifier>,
    pub(crate) icon_height: Option<MetricModifier>,
}

impl ResolvedMetricAdjustments {
    /// Optionally apply a modifier to a base pixel value, clamping to `floor`.
    pub(crate) fn adjust(modifier: Option<MetricModifier>, base: f32, floor: f32) -> f32 {
        match modifier {
            Some(modifier) => modifier.apply(base).max(floor),
            None => base,
        }
    }
}

impl FontMetricAdjustments {
    /// Parse every field into its `MetricModifier` form once (invalid entries become `None`).
    pub(crate) fn resolve(&self) -> ResolvedMetricAdjustments {
        let parse = |slot: &Option<String>| slot.as_deref().and_then(MetricModifier::parse);
        ResolvedMetricAdjustments {
            cell_width: parse(&self.cell_width),
            cell_height: parse(&self.cell_height),
            font_baseline: parse(&self.font_baseline),
            underline_position: parse(&self.underline_position),
            underline_thickness: parse(&self.underline_thickness),
            strikethrough_position: parse(&self.strikethrough_position),
            strikethrough_thickness: parse(&self.strikethrough_thickness),
            cursor_thickness: parse(&self.cursor_thickness),
            box_thickness: parse(&self.box_thickness),
            icon_height: parse(&self.icon_height),
        }
    }
}

use eggie_protocol::DEFAULT_PALETTE;

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
    pub(crate) const ALL: [Self; 4] = [
        Self::Silent,
        Self::Flash,
        Self::Sound,
        Self::FlashAndSound,
    ];

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

    fn is_default(&self) -> bool {
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

    fn is_default(&self) -> bool {
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

/// Which styles Ghostty-style synthesis is allowed to fabricate when a font lacks a native face,
/// matching `font-synthetic-style`. Default: all enabled. When disabled for a style, a missing face
/// falls back to the regular font instead of synthesizing bold (stroke) / italic (skew).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FontSyntheticStyle {
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) bold_italic: bool,
}

impl Default for FontSyntheticStyle {
    fn default() -> Self {
        Self {
            bold: true,
            italic: true,
            bold_italic: true,
        }
    }
}

impl FontSyntheticStyle {
    fn is_all_enabled(&self) -> bool {
        *self == Self::default()
    }
}

/// The four resolved font families (regular + per-style, each already falling back to regular when
/// unset) plus the synthetic-style policy, threaded through the render pipeline. Cheap to clone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedFontFamilies {
    pub(crate) regular: std::sync::Arc<str>,
    pub(crate) bold: std::sync::Arc<str>,
    pub(crate) italic: std::sync::Arc<str>,
    pub(crate) bold_italic: std::sync::Arc<str>,
    pub(crate) synthetic: FontSyntheticStyle,
}

impl ResolvedFontFamilies {
    /// Select the family for a `(bold, italic)` style. Every slot already falls back to regular, so
    /// this is a direct pick.
    pub(crate) fn family(&self, bold: bool, italic: bool) -> &std::sync::Arc<str> {
        match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }

    /// Whether synthesis (CoreText symbolic-trait substitution) is allowed for a `(bold, italic)`
    /// style. Only meaningful when that style has no dedicated family (otherwise the family wins).
    pub(crate) fn allows_synthesis(&self, bold: bool, italic: bool) -> bool {
        match (bold, italic) {
            (false, false) => true,
            (true, false) => self.synthetic.bold,
            (false, true) => self.synthetic.italic,
            (true, true) => self.synthetic.bold_italic,
        }
    }

    /// Whether the `(bold, italic)` style resolved to a dedicated (non-regular) family.
    pub(crate) fn has_dedicated_family(&self, bold: bool, italic: bool) -> bool {
        !std::sync::Arc::ptr_eq(self.family(bold, italic), &self.regular)
            && self.family(bold, italic).as_ref() != self.regular.as_ref()
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
    #[serde(default = "default_dark_theme", skip_serializing_if = "is_default_dark_theme")]
    pub(crate) terminal_dark_theme: String,
    /// Terminal theme for the light appearance. See [`Self::terminal_dark_theme`].
    #[serde(default = "default_light_theme", skip_serializing_if = "is_default_light_theme")]
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
    #[serde(default = "default_thicken_strength", skip_serializing_if = "is_full_strength")]
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
    #[serde(default = "default_scrollback_lines", skip_serializing_if = "is_default_scrollback")]
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

/// Per-metric adjustments matching Ghostty's `adjust-*` config keys. Each is an optional
/// human-editable string (`"2"`, `"-1"`, `"20%"`); `None`/absent means "use the font-derived
/// value unchanged". Only metrics Eggie actually renders are exposed — `adjust-overline-*` is
/// omitted because the vte kernel never emits an overline attribute (no SGR 53), so it would be
/// dead config.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FontMetricAdjustments {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cell_width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cell_height: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) font_baseline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) underline_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) underline_thickness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) strikethrough_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) strikethrough_thickness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cursor_thickness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) box_thickness: Option<String>,
    /// Percentage or pixel adjustment to the constraint height of Nerd Font icons. Unlike the
    /// other metrics this scales fallback icon glyphs at raster time rather than moving a
    /// decoration line; `None` leaves icons at their font-native size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) icon_height: Option<String>,
}

impl FontMetricAdjustments {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Drop any entry that isn't a valid `MetricModifier` string so bad input never persists and
    /// downstream code can treat "present" as "valid".
    fn normalize(&mut self) {
        for slot in [
            &mut self.cell_width,
            &mut self.cell_height,
            &mut self.font_baseline,
            &mut self.underline_position,
            &mut self.underline_thickness,
            &mut self.strikethrough_position,
            &mut self.strikethrough_thickness,
            &mut self.cursor_thickness,
            &mut self.box_thickness,
            &mut self.icon_height,
        ] {
            if slot.as_deref().and_then(MetricModifier::parse).is_none() {
                *slot = None;
            }
        }
    }
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

    fn normalize(&mut self) {
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
    fn pick_theme(use_dark: bool, dark: &str, light: &str) -> &'static TerminalTheme {
        let catalog = theme_catalog();
        if use_dark {
            catalog
                .dark_theme(dark)
                .or_else(|| catalog.dark_theme(DEFAULT_DARK_THEME))
                .unwrap_or(&catalog.dark[0])
        } else {
            catalog
                .light_theme(light)
                .or_else(|| catalog.light_theme(DEFAULT_LIGHT_THEME))
                .unwrap_or(&catalog.light[0])
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

/// The global terminal layer a profile's overrides merge over, together with the dark/light decision
/// the profile layer has to repeat.
///
/// These two always travel together — resolving a profile override needs both the fallback theme and
/// which half of the profile's own pair to read — so they are one value rather than two parameters
/// threaded side by side through the spawn paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlobalTerminalLayer {
    pub(crate) theme: &'static TerminalTheme,
    pub(crate) use_dark: bool,
}

impl GlobalTerminalLayer {
    pub(crate) fn new(config: &AppSettings, system_is_dark: bool) -> Self {
        Self {
            theme: config.terminal_theme(system_is_dark),
            use_dark: config.use_dark(system_is_dark),
        }
    }
}

/// A terminal's appearance after merging its profile's overrides over the global settings.
///
/// One of these is resolved per terminal on every render pass, so a change to either layer moves the
/// terminals it applies to and leaves the rest alone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedAppearance {
    pub(crate) theme: &'static TerminalTheme,
    pub(crate) font_family: std::sync::Arc<str>,
    pub(crate) font_size: f32,
    /// Per-style families, with the profile's family (when it overrides one) as the regular slot.
    pub(crate) families: ResolvedFontFamilies,
}

/// Merge a profile's appearance overrides over the global settings: profile → global → built-in
/// default, one field at a time.
///
/// `global` is the already-resolved global *terminal* layer plus the dark/light decision (see
/// [`GlobalTerminalLayer`] and [`AppSettings::terminal_theme`] — a profile never overrides the
/// interface, because chrome is window-level while one window can show tabs from several profiles at
/// once).
///
/// The dark/light decision cannot be folded into the resolved theme the way it once was: the profile
/// layer has to make the same choice again, so a system flip moves a profile-themed terminal even when
/// the global palette is unchanged. An unknown theme name falls back to `global.theme` rather than to
/// an arbitrary catalog entry, so a typo degrades to "no override".
///
/// `profile_theme_preview` is the settings window's live hover preview when it applies to *this*
/// profile. The caller decides applicability, because it knows which profile the session belongs to and
/// this module deliberately stays profile-id agnostic.
///
/// `font_size` is clamped to the app's supported range, matching what `AppSettings::normalize` does to
/// the global value; a non-finite override is ignored.
pub(crate) fn resolve_appearance(
    config: &AppSettings,
    appearance: Option<&eggie_domain::ProfileAppearance>,
    global: GlobalTerminalLayer,
    profile_theme_preview: Option<&'static TerminalTheme>,
) -> ResolvedAppearance {
    let theme = profile_theme_preview
        .or_else(|| {
            appearance
                .and_then(|appearance| appearance.terminal_theme(global.use_dark))
                .and_then(|name| theme_catalog().theme_by_name(name))
        })
        .unwrap_or(global.theme);

    let mut families = config.resolved_font_families();
    let font_family: std::sync::Arc<str> = match appearance
        .and_then(|appearance| appearance.font_family.as_deref())
        .map(str::trim)
        .filter(|family| !family.is_empty())
    {
        Some(family) => {
            // Overriding the regular family must also move every per-style slot that merely inherited
            // it, or a profile's font would apply to regular text and leave bold/italic on the global
            // family. Slots with their own explicit family keep it.
            //
            // Inheritance is read from the *source* config, not from the resolved strings:
            // `resolved_font_families` already collapsed "unset" and "explicitly set to the regular
            // family" into the same value, so comparing resolved names would move a slot the user had
            // deliberately pinned.
            let overridden: std::sync::Arc<str> = std::sync::Arc::from(family);
            let inherits = |explicit: &str| explicit.trim().is_empty();
            if inherits(&config.font_family_bold) {
                families.bold = std::sync::Arc::clone(&overridden);
            }
            if inherits(&config.font_family_italic) {
                families.italic = std::sync::Arc::clone(&overridden);
            }
            if inherits(&config.font_family_bold_italic) {
                families.bold_italic = std::sync::Arc::clone(&overridden);
            }
            families.regular = std::sync::Arc::clone(&overridden);
            overridden
        }
        None => std::sync::Arc::clone(&families.regular),
    };

    let font_size = appearance
        .and_then(|appearance| appearance.font_size)
        .filter(|size| size.is_finite())
        .map(|size| size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE))
        .unwrap_or(config.font_size);

    ResolvedAppearance {
        theme,
        font_family,
        font_size,
        families,
    }
}

pub(crate) fn minimum_contrast_rgb(foreground: u32, background: u32, minimum_contrast: f32) -> u32 {
    if !minimum_contrast.is_finite() || minimum_contrast <= 1. {
        return foreground;
    }
    if contrast_ratio_rgb(foreground, background) >= minimum_contrast {
        return foreground;
    }

    let white_contrast = contrast_ratio_rgb(0xffffff, background);
    let black_contrast = contrast_ratio_rgb(0x000000, background);
    if white_contrast > black_contrast {
        0xffffff
    } else {
        0x000000
    }
}

fn contrast_ratio_rgb(first: u32, second: u32) -> f32 {
    let first = relative_luminance_rgb(first);
    let second = relative_luminance_rgb(second);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

fn relative_luminance_rgb(color: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff_u32) as f32 / 255.;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// Which theme picker a hover preview came from. Recorded alongside the previewed theme so the
/// preview resolves through the *same* layering as the real setting: previewing an interface theme
/// moves the terminal too when it follows the interface, and leaves it alone when it does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreviewSlot {
    InterfaceDark,
    InterfaceLight,
    TerminalDark,
    TerminalLight,
    /// A profile's terminal override, carrying the profile being edited. The identity travels with the
    /// slot because the main window — which has to decide whether a given terminal is affected — cannot
    /// see the settings window's selection.
    ProfileTerminalDark(eggie_domain::ProfileId),
    ProfileTerminalLight(eggie_domain::ProfileId),
}

impl PreviewSlot {
    /// Whether this slot belongs to the dark half of its pair. Used to label the slot in the UI;
    /// the preview itself applies regardless of which half is currently in effect.
    fn is_dark(self) -> bool {
        matches!(
            self,
            Self::InterfaceDark | Self::TerminalDark | Self::ProfileTerminalDark(_)
        )
    }
}

pub(crate) struct SettingsStore {
    config: AppSettings,
    path: PathBuf,
    /// Transient theme override driven by the settings window's hover preview, tagged with the picker
    /// it came from. Never persisted and never serialized — it only overrides what the `resolved_*`
    /// methods return while the pointer rests on a theme option. Cleared when the pointer leaves, the
    /// dropdown closes, or the window closes.
    theme_preview: Option<(PreviewSlot, &'static TerminalTheme)>,
}

impl SettingsStore {
    pub(crate) fn load() -> Self {
        Self::load_from(crate::persist::data_file_path("settings.json"))
    }

    fn load_from(path: PathBuf) -> Self {
        let mut config = crate::persist::load_json::<AppSettings>(&path).unwrap_or_default();
        config.normalize();
        Self {
            config,
            path,
            theme_preview: None,
        }
    }

    pub(crate) fn config(&self) -> &AppSettings {
        &self.config
    }

    /// The interface theme actually in effect: the live hover preview if one is active on either
    /// interface slot, otherwise the persisted pair. Drives the app's chrome.
    pub(crate) fn resolved_interface_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        // A hover preview is authoritative: it applies regardless of which half is in effect, so
        // hovering the light slot while the app is dark previews the light slot.
        if let Some((slot, theme)) = self.theme_preview
            && matches!(slot, PreviewSlot::InterfaceDark | PreviewSlot::InterfaceLight)
        {
            return theme;
        }
        self.config.interface_theme(system_is_dark)
    }

    /// The global terminal theme actually in effect. Under
    /// [`TerminalThemeSource::FollowInterface`] this delegates to [`Self::resolved_interface_theme`] —
    /// that one line is what makes hovering an interface theme move the terminals along with the
    /// chrome, exactly as committing it would.
    pub(crate) fn resolved_terminal_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        match self.config.terminal_theme_source {
            TerminalThemeSource::FollowInterface => self.resolved_interface_theme(system_is_dark),
            TerminalThemeSource::Custom => {
                if let Some((slot, theme)) = self.theme_preview
                    && matches!(slot, PreviewSlot::TerminalDark | PreviewSlot::TerminalLight)
                {
                    return theme;
                }
                self.config.terminal_theme(system_is_dark)
            }
        }
    }

    /// The hover preview for `profile_id`'s terminal override, when one is active on the half currently
    /// in effect. Returns `None` for any other profile, so previewing one profile's theme never repaints
    /// terminals belonging to another.
    pub(crate) fn profile_theme_preview(
        &self,
        profile_id: eggie_domain::ProfileId,
    ) -> Option<&'static TerminalTheme> {
        // A hover preview applies regardless of which half is in effect, but only for the profile
        // being edited — previewing one profile's theme never repaints another's terminals.
        if let Some((slot, theme)) = self.theme_preview
            && match slot {
                PreviewSlot::ProfileTerminalDark(id) | PreviewSlot::ProfileTerminalLight(id) => {
                    id == profile_id
                }
                _ => false,
            }
        {
            return Some(theme);
        }
        None
    }

    /// Whether any hover preview is active, and on which slot. Feeds the main window's reconcile gate:
    /// a profile preview changes no global palette, so without this the reconcile would skip it.
    pub(crate) fn theme_preview_key(&self) -> Option<(PreviewSlot, usize)> {
        self.theme_preview
            .map(|(slot, theme)| (slot, theme as *const TerminalTheme as usize))
    }

    /// Set the transient hover preview to `theme` on `slot`. No-op (and no notify) if it is already the
    /// active preview, so a stream of repeated hover events over the same option doesn't churn renders.
    pub(crate) fn set_theme_preview(
        &mut self,
        slot: PreviewSlot,
        theme: &'static TerminalTheme,
        cx: &mut Context<Self>,
    ) {
        if self
            .theme_preview
            .is_some_and(|(active, current)| active == slot && std::ptr::eq(current, theme))
        {
            return;
        }
        self.theme_preview = Some((slot, theme));
        cx.notify();
    }

    /// Drop any active hover preview, restoring the persisted theme. No-op if none is active.
    pub(crate) fn clear_theme_preview(&mut self, cx: &mut Context<Self>) {
        if self.theme_preview.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn update(&mut self, update: impl FnOnce(&mut AppSettings), cx: &mut Context<Self>) {
        update(&mut self.config);
        self.config.normalize();
        if let Err(error) = self.save() {
            eprintln!("failed to persist Eggie settings: {error}");
        }
        cx.notify();
    }

    fn save(&self) -> std::io::Result<()> {
        crate::persist::save_json_atomic(&self.path, &self.config)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalTheme {
    pub(crate) name: String,
    pub(crate) palette: [u32; 16],
    pub(crate) background: u32,
    pub(crate) foreground: u32,
    pub(crate) cursor: u32,
    pub(crate) cursor_text: u32,
    pub(crate) selection_background: u32,
    pub(crate) selection_foreground: u32,
}

impl TerminalTheme {
    pub(crate) fn appearance(&self) -> eggie_protocol::TerminalAppearance {
        eggie_protocol::TerminalAppearance {
            palette: self.palette,
            foreground: self.foreground,
            background: self.background,
            cursor: self.cursor,
            cursor_text: self.cursor_text,
        }
    }

    pub(crate) fn is_dark(&self) -> bool {
        let red = ((self.background >> 16) & 0xff) as f32 / 255.;
        let green = ((self.background >> 8) & 0xff) as f32 / 255.;
        let blue = (self.background & 0xff) as f32 / 255.;
        0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.5
    }
}

pub(crate) struct ThemeCatalog {
    dark: Vec<TerminalTheme>,
    light: Vec<TerminalTheme>,
}

impl ThemeCatalog {
    pub(crate) fn dark_names(&self) -> Vec<String> {
        self.dark.iter().map(|theme| theme.name.clone()).collect()
    }

    pub(crate) fn light_names(&self) -> Vec<String> {
        self.light.iter().map(|theme| theme.name.clone()).collect()
    }

    pub(crate) fn dark_theme(&self, name: &str) -> Option<&TerminalTheme> {
        self.dark.iter().find(|theme| theme.name == name)
    }

    pub(crate) fn light_theme(&self, name: &str) -> Option<&TerminalTheme> {
        self.light.iter().find(|theme| theme.name == name)
    }

    /// Look a theme up by name across both the dark and light lists. Used by the settings
    /// window's live hover preview, which resolves whichever theme the pointer is over.
    pub(crate) fn theme_by_name(&self, name: &str) -> Option<&TerminalTheme> {
        self.dark_theme(name).or_else(|| self.light_theme(name))
    }
}

pub(crate) fn theme_catalog() -> &'static ThemeCatalog {
    static CATALOG: OnceLock<ThemeCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut dark = Vec::new();
        let mut light = Vec::new();
        for (name, source) in GHOSTTY_THEME_SOURCES {
            let theme = parse_ghostty_theme(name, source);
            if theme.is_dark() {
                dark.push(theme);
            } else {
                light.push(theme);
            }
        }
        ThemeCatalog { dark, light }
    })
}

pub(crate) fn system_uses_dark_appearance() -> bool {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .eq_ignore_ascii_case("dark")
            })
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn parse_ghostty_theme(name: &str, source: &str) -> TerminalTheme {
    let mut theme = TerminalTheme {
        name: name.to_owned(),
        palette: DEFAULT_PALETTE,
        background: 0x282c34,
        foreground: 0xffffff,
        cursor: 0xffffff,
        cursor_text: 0x282c34,
        selection_background: 0x3e4451,
        selection_foreground: 0xffffff,
    };
    for raw_line in source.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "palette" {
            let Some((index, color)) = value.split_once('=') else {
                continue;
            };
            if let (Ok(index), Some(color)) =
                (index.trim().parse::<usize>(), parse_hex_color(color.trim()))
                && let Some(slot) = theme.palette.get_mut(index)
            {
                *slot = color;
            }
            continue;
        }
        let Some(color) = parse_hex_color(value) else {
            continue;
        };
        match key {
            "background" => theme.background = color,
            "foreground" => theme.foreground = color,
            "cursor-color" => theme.cursor = color,
            "cursor-text" => theme.cursor_text = color,
            "selection-background" => theme.selection_background = color,
            "selection-foreground" => theme.selection_foreground = color,
            _ => {}
        }
    }
    theme
}

fn parse_hex_color(value: &str) -> Option<u32> {
    let value = value.trim().trim_start_matches('#');
    (value.len() == 6)
        .then(|| u32::from_str_radix(value, 16).ok())
        .flatten()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UiColors {
    pub(crate) background: u32,
    pub(crate) panel: u32,
    pub(crate) panel_alt: u32,
    pub(crate) hover: u32,
    pub(crate) border: u32,
    pub(crate) text: u32,
    pub(crate) muted: u32,
    pub(crate) accent: u32,
    pub(crate) accent_foreground: u32,
    pub(crate) accent_hover: u32,
}

impl UiColors {
    pub(crate) fn from_theme(theme: &TerminalTheme) -> Self {
        let dark = theme.is_dark();
        let channels = [
            ((theme.background >> 16) & 0xff) as f32,
            ((theme.background >> 8) & 0xff) as f32,
            (theme.background & 0xff) as f32,
        ];
        let chroma = (channels.iter().copied().fold(0., f32::max)
            - channels.iter().copied().fold(255., f32::min))
            / 255.;
        let background = mix(
            if dark { 0x171b20 } else { 0xf4f5f7 },
            theme.background,
            if dark {
                0.12 + 0.27 * chroma
            } else {
                0.12 + 0.22 * chroma
            },
        );
        let (panel, panel_alt, hover, border, text, muted) = if dark {
            (
                mix(background, 0xffffff, 0.055),
                mix(background, 0xffffff, 0.1),
                mix(background, 0xffffff, 0.16),
                mix(background, 0xffffff, 0.24),
                0xf2f4f7,
                0xaab4c0,
            )
        } else {
            (
                mix(background, 0xffffff, 0.64),
                mix(background, 0x000000, 0.04),
                mix(background, 0x000000, 0.08),
                mix(background, 0x000000, 0.15),
                0x202832,
                0x5b6571,
            )
        };
        let mut muted = muted;
        for _ in 0..16 {
            if contrast_ratio_rgb(muted, panel) >= 4.55
                && contrast_ratio_rgb(muted, panel_alt) >= 4.55
                && contrast_ratio_rgb(muted, hover) >= 4.55
            {
                break;
            }
            muted = mix(muted, if dark { 0xffffff } else { 0x000000 }, 0.08);
        }
        let accent = ui_accent(theme.palette[4], [background, panel, panel_alt], dark);
        let accent_foreground = if contrast_ratio_rgb(0xffffff, accent)
            >= contrast_ratio_rgb(0x000000, accent)
        {
            0xffffff
        } else {
            0x000000
        };
        let accent_hover = mix(
            accent,
            if accent_foreground == 0xffffff { 0x000000 } else { 0xffffff },
            0.1,
        );
        Self {
            background,
            panel,
            panel_alt,
            hover,
            border,
            text,
            muted,
            accent,
            accent_foreground,
            accent_hover,
        }
    }
}

fn ui_accent(source: u32, surfaces: [u32; 3], dark: bool) -> u32 {
    let (mut hue, saturation, _) = rgb_to_hsl(source);
    if saturation < 0.18 {
        hue = 215. / 360.;
    }
    let saturation = saturation.clamp(0.45, 0.68);
    let mut lightness: f32 = if dark { 0.64 } else { 0.42 };
    loop {
        let accent = hsl_to_rgb(hue, saturation, lightness);
        if surfaces
            .iter()
            .all(|surface| contrast_ratio_rgb(accent, *surface) >= 4.55)
        {
            return accent;
        }
        lightness = (lightness + if dark { 0.01 } else { -0.01 }).clamp(0.05, 0.95);
        if lightness <= 0.05 || lightness >= 0.95 {
            return accent;
        }
    }
}

fn rgb_to_hsl(color: u32) -> (f32, f32, f32) {
    let r = ((color >> 16) & 0xff) as f32 / 255.;
    let g = ((color >> 8) & 0xff) as f32 / 255.;
    let b = (color & 0xff) as f32 / 255.;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let lightness = (max + min) / 2.;
    if delta == 0. {
        return (0., 0., lightness);
    }
    let saturation = delta / (1. - (2. * lightness - 1.).abs());
    let hue = if max == r {
        ((g - b) / delta).rem_euclid(6.)
    } else if max == g {
        (b - r) / delta + 2.
    } else {
        (r - g) / delta + 4.
    } / 6.;
    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> u32 {
    let chroma = (1. - (2. * lightness - 1.).abs()) * saturation;
    let segment = (hue * 6.).rem_euclid(6.);
    let second = chroma * (1. - (segment.rem_euclid(2.) - 1.).abs());
    let (r, g, b) = match segment as u8 {
        0 => (chroma, second, 0.),
        1 => (second, chroma, 0.),
        2 => (0., chroma, second),
        3 => (0., second, chroma),
        4 => (second, 0., chroma),
        _ => (chroma, 0., second),
    };
    let offset = lightness - chroma / 2.;
    let channel = |value: f32| ((value + offset) * 255.).round().clamp(0., 255.) as u32;
    (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

fn mix(base: u32, overlay: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let base = ((base >> shift) & 0xff_u32) as f32;
        let overlay = ((overlay >> shift) & 0xff_u32) as f32;
        (base + (overlay - base) * amount).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn ui_palette_audit() {
        use std::fmt::Write as _;

        let catalog = theme_catalog();
        let mut failures = Vec::new();
        let mut html = String::from(
            r#"<!doctype html><html lang="en"><meta charset="utf-8"><title>Eggie UI palette audit</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#15191e;color:#eef1f4;font:14px -apple-system,BlinkMacSystemFont,sans-serif}
header{position:sticky;top:0;z-index:2;display:flex;align-items:center;gap:20px;padding:16px 24px;background:#15191eee;backdrop-filter:blur(18px);border-bottom:1px solid #39414a}
h1{font-size:18px;margin:0}input{margin-left:auto;width:260px;padding:8px 12px;border-radius:9px;border:1px solid #59616a;background:#242b32;color:#fff}
main{padding:24px}h2{margin:30px 0 18px;font-size:18px}.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(320px,1fr));gap:18px}
.theme{padding:10px;border:1px solid #414852;border-radius:18px;background:#20262d}.name{font-weight:650;padding:4px 4px 11px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.window{display:grid;grid-template-columns:80px 1fr;min-height:155px;overflow:hidden;border-radius:12px;background:var(--bg);color:var(--text)}
.sidebar{padding:12px 8px;background:var(--panel);border-right:1px solid var(--border)}.sideitem{padding:6px;border-radius:6px;color:var(--muted)}.sideitem.active{background:var(--hover);color:var(--text)}
.body{padding:11px}.tab{display:inline-block;padding:6px 9px;border:1px solid var(--border);border-radius:7px;background:var(--panel);color:var(--text)}
.sample{margin:12px 0 8px;padding:8px;border-radius:7px;background:var(--panel-alt);color:var(--text)}.muted{color:var(--muted)}
.button{display:inline-block;margin-top:2px;padding:5px 11px;border-radius:6px;background:var(--accent);color:var(--accent-text);font-weight:600}.button:hover{background:var(--accent-hover)}.link{margin-left:10px;color:var(--accent)}
.terminal{margin-top:8px;padding:9px;border-radius:8px;background:var(--term-bg);color:var(--term-fg);font:12px Menlo,monospace}
.ratios{padding:9px 3px 1px;color:#aeb7c2;font-size:11px}
</style><header><h1>Eggie UI palette audit</h1><span>Actual UiColors for every bundled theme; terminal colors shown unchanged.</span><input id="search" placeholder="Filter themes"></header><main>"#,
        );
        for (group, themes) in [("Light", &catalog.light), ("Dark", &catalog.dark)] {
            write!(html, "<section id=\"{}\"><h2>{group} · {} themes</h2><div class=grid>", group.to_lowercase(), themes.len())
                .unwrap();
            for theme in themes {
                let ui = UiColors::from_theme(theme);
                let checks = [
                    ("text / background", ui.text, ui.background, 4.5),
                    ("text / panel", ui.text, ui.panel, 4.5),
                    ("text / hover", ui.text, ui.hover, 4.5),
                    ("muted / panel", ui.muted, ui.panel, 4.5),
                    ("muted / panel alt", ui.muted, ui.panel_alt, 4.5),
                    ("muted / hover", ui.muted, ui.hover, 4.5),
                    ("accent / panel", ui.accent, ui.panel, 4.5),
                    ("accent / panel alt", ui.accent, ui.panel_alt, 4.5),
                    ("accent / background", ui.accent, ui.background, 4.5),
                    ("button text / accent", ui.accent_foreground, ui.accent, 4.5),
                    ("button text / hover", ui.accent_foreground, ui.accent_hover, 4.5),
                ];
                for (label, foreground, background, minimum) in checks {
                    let ratio = contrast_ratio_rgb(foreground, background);
                    if ratio < minimum {
                        failures.push(format!("{}: {label} contrast {ratio:.2} < {minimum}", theme.name));
                    }
                }
                let name = theme.name.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
                write!(
                    html,
                    "<article class=theme data-name=\"{name}\" style=\"--bg:#{:06x};--panel:#{:06x};--panel-alt:#{:06x};--hover:#{:06x};--border:#{:06x};--text:#{:06x};--muted:#{:06x};--accent:#{:06x};--accent-text:#{:06x};--accent-hover:#{:06x};--term-bg:#{:06x};--term-fg:#{:06x}\"><div class=name>{name}</div><div class=window><div class=sidebar><strong>Projects</strong><div class='sideitem active'>Eggie</div><div class=sideitem>Workspace</div></div><div class=body><span class=tab>◉ &nbsp; Terminal</span><div class=sample>Readable sample <span class=muted>· Secondary text</span></div><span class=button>Active</span><span class=link>Accent link</span></div></div><div class=terminal>~/Eggie $ cargo run</div><div class=ratios>text {:.1}:1 · muted {:.1}:1 · accent {:.1}:1</div></article>",
                    ui.background, ui.panel, ui.panel_alt, ui.hover, ui.border, ui.text,
                    ui.muted, ui.accent, ui.accent_foreground, ui.accent_hover, theme.background,
                    theme.foreground,
                    contrast_ratio_rgb(ui.text, ui.panel),
                    contrast_ratio_rgb(ui.muted, ui.panel),
                    contrast_ratio_rgb(ui.accent, ui.panel),
                )
                .unwrap();
            }
            html.push_str("</div></section>");
        }
        html.push_str(
            r#"</main><script>const input=document.querySelector('#search');input.value=new URL(location.href).searchParams.get('q')||'';function filter(){const q=input.value.toLowerCase();document.querySelectorAll('.theme').forEach(card=>card.hidden=!card.dataset.name.toLowerCase().includes(q))}input.addEventListener('input',filter);filter()</script></html>"#,
        );
        if let Some(path) = std::env::var_os("EGGIE_THEME_AUDIT_HTML") {
            fs::write(path, html).unwrap();
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn bundled_catalog_contains_dark_and_light_ghostty_themes() {
        let catalog = theme_catalog();
        assert!(catalog.dark.len() > 300);
        assert!(catalog.light.len() > 20);
        assert!(catalog.dark_theme("Catppuccin Mocha").is_some());
        assert!(catalog.light_theme("Ayu Light").is_some());
    }

    #[test]
    fn theme_by_name_resolves_across_dark_and_light_lists() {
        let catalog = theme_catalog();
        // A dark theme and a light theme both resolve through the unified lookup the hover preview
        // uses, and an unknown name yields None (so preview_theme becomes a no-op).
        assert_eq!(
            catalog.theme_by_name("Catppuccin Mocha").map(|t| t.name.as_str()),
            Some("Catppuccin Mocha")
        );
        assert_eq!(
            catalog.theme_by_name("Ayu Light").map(|t| t.name.as_str()),
            Some("Ayu Light")
        );
        assert!(catalog.theme_by_name("No Such Theme").is_none());
    }

    /// A store with no preview active, for the preview tests.
    fn store_with(config: AppSettings) -> SettingsStore {
        SettingsStore {
            config,
            path: std::path::PathBuf::from("unused"),
            theme_preview: None,
        }
    }

    #[test]
    fn a_preview_overrides_the_persisted_pair_regardless_of_the_appearance() {
        let catalog = theme_catalog();
        let mut store = store_with(AppSettings {
            theme_mode: ThemeMode::Dark,
            interface_dark_theme: "Builtin Dark".to_owned(),
            ..AppSettings::default()
        });

        // No preview active: resolves to the persisted pair.
        assert_eq!(store.resolved_interface_theme(true).name, "Builtin Dark");

        let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
        store.theme_preview = Some((PreviewSlot::InterfaceDark, preview));
        assert_eq!(store.resolved_interface_theme(true).name, "Catppuccin Mocha");

        // The preview is authoritative: it applies even when the system is light, so the dark-slot
        // preview stays visible under a light appearance. Only clearing it falls back to the persisted
        // pair, which then resolves light.
        store.config.theme_mode = ThemeMode::System;
        assert_eq!(
            store.resolved_interface_theme(false).name,
            "Catppuccin Mocha",
            "a preview applies regardless of the current appearance"
        );

        // Clearing restores the persisted pair, now resolving light.
        store.theme_preview = None;
        assert_eq!(
            store.resolved_interface_theme(false).name,
            AppSettings::default().interface_light_theme
        );
    }

    #[test]
    fn a_light_slot_preview_applies_while_the_app_is_dark() {
        let catalog = theme_catalog();
        let mut store = store_with(AppSettings {
            theme_mode: ThemeMode::Dark,
            interface_dark_theme: "Builtin Dark".to_owned(),
            ..AppSettings::default()
        });

        // Hovering the light slot while the app is dark must preview the light slot. Before the fix,
        // the preview was filtered out because the light slot's half did not match the dark mode.
        let preview = catalog.theme_by_name("Catppuccin Latte").unwrap();
        store.theme_preview = Some((PreviewSlot::InterfaceLight, preview));
        assert_eq!(
            store.resolved_interface_theme(true).name,
            "Catppuccin Latte",
            "a light-slot preview applies regardless of the current dark appearance"
        );
    }

    #[test]
    fn previewing_an_interface_theme_moves_the_terminal_only_when_it_follows() {
        let catalog = theme_catalog();
        let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
        let mut store = store_with(AppSettings {
            theme_mode: ThemeMode::Dark,
            interface_dark_theme: "Builtin Dark".to_owned(),
            ..AppSettings::default()
        });
        store.theme_preview = Some((PreviewSlot::InterfaceDark, preview));

        // Following the interface: the terminal rides along with the hover, exactly as it would once
        // the choice is committed.
        assert_eq!(store.config.terminal_theme_source, TerminalThemeSource::FollowInterface);
        assert_eq!(store.resolved_terminal_theme(true).name, "Catppuccin Mocha");

        // With its own pair, the terminal ignores an interface preview.
        store.config.use_custom_terminal_themes();
        store.config.terminal_dark_theme = "Builtin Dark".to_owned();
        assert_eq!(store.resolved_interface_theme(true).name, "Catppuccin Mocha");
        assert_eq!(
            store.resolved_terminal_theme(true).name,
            "Builtin Dark",
            "a custom terminal pair must not follow an interface preview"
        );

        // And a terminal preview moves only the terminal, never the chrome.
        store.theme_preview = Some((PreviewSlot::TerminalDark, preview));
        assert_eq!(store.resolved_terminal_theme(true).name, "Catppuccin Mocha");
        assert_eq!(store.resolved_interface_theme(true).name, "Builtin Dark");
    }

    #[test]
    fn a_profile_preview_is_scoped_to_the_profile_being_edited() {
        let catalog = theme_catalog();
        let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
        let edited = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        let mut store = store_with(AppSettings {
            theme_mode: ThemeMode::Dark,
            ..AppSettings::default()
        });
        store.theme_preview = Some((PreviewSlot::ProfileTerminalDark(edited), preview));

        assert_eq!(
            store.profile_theme_preview(edited).map(|theme| theme.name.as_str()),
            Some("Catppuccin Mocha")
        );
        assert!(
            store.profile_theme_preview(other).is_none(),
            "previewing one profile's theme must not repaint another profile's terminals"
        );
        // A profile preview is also not a global one.
        assert_eq!(store.resolved_interface_theme(true).name, "Builtin Dark");
        assert_eq!(store.resolved_terminal_theme(true).name, "Builtin Dark");
    }

    #[test]
    fn the_terminal_layer_equals_the_interface_layer_while_it_follows() {
        // The default (and pre-split) behaviour: one theme drives both layers, in every combination of
        // theme_mode and system appearance.
        let config = AppSettings {
            interface_dark_theme: "Catppuccin Mocha".to_owned(),
            interface_light_theme: "Ayu Light".to_owned(),
            ..AppSettings::default()
        };
        assert_eq!(config.terminal_theme_source, TerminalThemeSource::FollowInterface);
        for mode in ThemeMode::ALL {
            for system_is_dark in [true, false] {
                let config = AppSettings {
                    theme_mode: mode,
                    ..config.clone()
                };
                assert!(
                    std::ptr::eq(
                        config.interface_theme(system_is_dark),
                        config.terminal_theme(system_is_dark)
                    ),
                    "{mode:?} / system_is_dark={system_is_dark}: the terminal must follow the interface"
                );
            }
        }
    }

    #[test]
    fn a_custom_terminal_pair_is_independent_of_the_interface_pair() {
        // The point of the split: a dark interface with a light terminal, and vice versa.
        let config = AppSettings {
            theme_mode: ThemeMode::Dark,
            interface_dark_theme: "Catppuccin Mocha".to_owned(),
            terminal_theme_source: TerminalThemeSource::Custom,
            terminal_dark_theme: "Builtin Dark".to_owned(),
            ..AppSettings::default()
        };
        assert_eq!(config.interface_theme(true).name, "Catppuccin Mocha");
        assert_eq!(config.terminal_theme(true).name, "Builtin Dark");

        // Each layer picks its own half with the same dark/light decision.
        let config = AppSettings {
            theme_mode: ThemeMode::Light,
            interface_light_theme: "Ayu Light".to_owned(),
            terminal_light_theme: "Builtin Light".to_owned(),
            ..config
        };
        assert_eq!(config.interface_theme(true).name, "Ayu Light");
        assert_eq!(config.terminal_theme(true).name, "Builtin Light");
    }

    #[test]
    fn opting_into_a_custom_terminal_pair_changes_nothing_on_screen() {
        // Seeding from the interface pair is what keeps the toggle from reading as a bug: without it,
        // switching to Custom would jump the terminal to the built-in defaults.
        let mut config = AppSettings {
            interface_dark_theme: "Catppuccin Mocha".to_owned(),
            interface_light_theme: "Ayu Light".to_owned(),
            ..AppSettings::default()
        };
        let before = [config.terminal_theme(true), config.terminal_theme(false)];
        config.use_custom_terminal_themes();
        assert_eq!(config.terminal_theme_source, TerminalThemeSource::Custom);
        for (index, system_is_dark) in [true, false].into_iter().enumerate() {
            assert!(
                std::ptr::eq(config.terminal_theme(system_is_dark), before[index]),
                "system_is_dark={system_is_dark}: the toggle must be visually inert"
            );
        }
    }

    #[test]
    fn a_light_theme_cannot_be_persisted_into_a_dark_slot() {
        // Each slot is validated against its own half of the catalog, which is what lets each picker
        // offer one half rather than all of it.
        let mut config = AppSettings {
            interface_dark_theme: "Ayu Light".to_owned(),
            terminal_dark_theme: "Ayu Light".to_owned(),
            interface_light_theme: "Catppuccin Mocha".to_owned(),
            terminal_light_theme: "Catppuccin Mocha".to_owned(),
            ..AppSettings::default()
        };
        config.normalize();
        assert_eq!(config.interface_dark_theme, DEFAULT_DARK_THEME);
        assert_eq!(config.terminal_dark_theme, DEFAULT_DARK_THEME);
        assert_eq!(config.interface_light_theme, DEFAULT_LIGHT_THEME);
        assert_eq!(config.terminal_light_theme, DEFAULT_LIGHT_THEME);
    }

    #[test]
    fn the_global_palette_alone_cannot_detect_an_appearance_flip() {
        // This is why the main window's reconcile gate carries `use_dark` as an input of its own.
        //
        // Pin the global terminal theme so a system flip leaves it byte-identical (a light/dark pair
        // naming the same theme does this; so does `theme_mode` pinned to Dark or Light). A profile with
        // a per-half override still has to re-resolve, so a gate keyed only on the global palette would
        // skip the reconcile and leave that terminal painting the wrong half.
        let config = AppSettings {
            terminal_theme_source: TerminalThemeSource::Custom,
            terminal_dark_theme: DEFAULT_DARK_THEME.to_owned(),
            terminal_light_theme: DEFAULT_LIGHT_THEME.to_owned(),
            theme_mode: ThemeMode::Dark,
            ..AppSettings::default()
        };
        // `theme_mode: Dark` pins the global layer: both system appearances resolve to the same theme.
        let pinned = config.terminal_theme(true);
        assert!(
            std::ptr::eq(pinned, config.terminal_theme(false)),
            "the global palette must be unchanged across the flip for this test to mean anything"
        );

        let dark_name = theme_catalog().dark_names().into_iter().next().unwrap();
        let light_name = theme_catalog().light_names().into_iter().next().unwrap();
        let appearance = eggie_domain::ProfileAppearance {
            terminal_dark_theme: Some(dark_name.clone()),
            terminal_light_theme: Some(light_name.clone()),
            ..Default::default()
        };

        // Same global layer, same profile, different half in effect → different resolved palette.
        let resolved_dark = resolve_appearance(
            &config,
            Some(&appearance),
            GlobalTerminalLayer {
                theme: pinned,
                use_dark: true,
            },
            None,
        );
        let resolved_light = resolve_appearance(
            &config,
            Some(&appearance),
            GlobalTerminalLayer {
                theme: pinned,
                use_dark: false,
            },
            None,
        );
        assert_eq!(resolved_dark.theme.name, dark_name);
        assert_eq!(resolved_light.theme.name, light_name);
        assert_ne!(
            resolved_dark.theme.appearance(),
            resolved_light.theme.appearance(),
            "the flip changes what the daemon must be told, while the global palette says nothing"
        );
    }

    #[test]
    fn a_preview_key_distinguishes_slots_and_themes() {
        // The gate's other new input. A profile preview changes no global palette, so without a key the
        // reconcile would skip the repaint the preview exists to show.
        let catalog = theme_catalog();
        let first = catalog.theme_by_name("Catppuccin Mocha").unwrap();
        let second = catalog.theme_by_name("Builtin Dark").unwrap();
        let profile = uuid::Uuid::new_v4();
        let mut store = store_with(AppSettings::default());
        assert!(store.theme_preview_key().is_none());

        store.theme_preview = Some((PreviewSlot::ProfileTerminalDark(profile), first));
        let armed = store.theme_preview_key();
        assert!(armed.is_some());

        // Moving to another option on the same slot changes the key.
        store.theme_preview = Some((PreviewSlot::ProfileTerminalDark(profile), second));
        assert_ne!(armed, store.theme_preview_key());

        // So does the same theme on a different slot.
        store.theme_preview = Some((PreviewSlot::TerminalDark, second));
        let terminal_slot = store.theme_preview_key();
        store.theme_preview = Some((PreviewSlot::InterfaceDark, second));
        assert_ne!(terminal_slot, store.theme_preview_key());

        // And clearing it returns to "nothing armed", so the gate notices the restore.
        store.theme_preview = None;
        assert!(store.theme_preview_key().is_none());
    }

    #[test]
    fn an_existing_settings_file_keeps_its_chosen_themes() {
        // The rename would otherwise silently reset both themes to the built-in defaults on first launch,
        // because serde ignores unknown keys. The aliases are what make that a non-event.
        let config: AppSettings = serde_json::from_str(
            r#"{"dark_theme":"Atom One Dark","light_theme":"iTerm2 Solarized Light"}"#,
        )
        .unwrap();
        assert_eq!(config.interface_dark_theme, "Atom One Dark");
        assert_eq!(config.interface_light_theme, "iTerm2 Solarized Light");
        // An old file says nothing about the terminal layer, so it follows the interface — which is
        // exactly the behaviour that file was written under.
        assert_eq!(config.terminal_theme_source, TerminalThemeSource::FollowInterface);
    }

    #[test]
    fn settings_round_trip_and_normalize_font_size() {
        let directory =
            std::env::temp_dir().join(format!("eggie-settings-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.json");
        let mut config = AppSettings {
            language: Language::English,
            theme_mode: ThemeMode::Light,
            interface_dark_theme: "Catppuccin Mocha".to_owned(),
            interface_light_theme: "Ayu Light".to_owned(),
            terminal_theme_source: TerminalThemeSource::default(),
            terminal_dark_theme: DEFAULT_DARK_THEME.to_owned(),
            terminal_light_theme: DEFAULT_LIGHT_THEME.to_owned(),
            font_family: "Menlo".to_owned(),
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
            font_size: 100.,
            terminal_padding_x: -20.,
            terminal_padding_y: f32::NAN,
            minimum_contrast: 99.,
            progress_complete_timeout_secs: 0,
            progress_stale_timeout_secs: u32::MAX,
            allow_osc_clipboard_read: true,
            detect_urls: false,
            copy_on_select: false,
            auto_check_updates: true,
            update_channel: UpdateChannel::Beta,
            cursor_shape: CursorShapeSetting::Bar,
            cursor_blink: CursorBlink::On,
            bell_mode: BellMode::Sound,
            open_directory_with: OpenWith::VsCode,
            font_metrics: FontMetricAdjustments::default(),
            keybindings: std::collections::BTreeMap::new(),
            scrollback_lines: MAX_SCROLLBACK_LINES + 1,
            shell_program: "  /bin/fish  ".to_owned(),
            shell_args: vec!["-l".to_owned(), "  ".to_owned()],
            shell_integration_path: false,
            remote_install_consent: std::collections::BTreeMap::from([(
                "deploy@prod.example.com".to_owned(),
                true,
            )]),
        };
        config.normalize();
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();

        let loaded = SettingsStore::load_from(path);
        assert_eq!(loaded.config.font_size, MAX_FONT_SIZE);
        // Remembered remote-install answers survive a round trip: being re-asked on every launch
        // would defeat the point of recording the decision.
        assert_eq!(
            loaded.config.remote_install_consent.get("deploy@prod.example.com"),
            Some(&true)
        );
        assert_eq!(loaded.config.terminal_padding_x, MIN_TERMINAL_PADDING);
        assert_eq!(loaded.config.terminal_padding_y, DEFAULT_TERMINAL_PADDING_Y);
        assert_eq!(loaded.config.minimum_contrast, MAX_MINIMUM_CONTRAST);
        assert_eq!(loaded.config.scrollback_lines, MAX_SCROLLBACK_LINES);
        assert_eq!(loaded.config.shell_program, "/bin/fish");
        assert_eq!(loaded.config.shell_args, vec!["-l".to_owned()]);
        // A non-default (false) value serializes and survives the round trip.
        assert!(!loaded.config.shell_integration_path);
        // A non-default opener persists across the round trip.
        assert_eq!(loaded.config.open_directory_with, OpenWith::VsCode);
        assert_eq!(
            loaded.config.progress_complete_timeout_secs,
            MIN_PROGRESS_TIMEOUT_SECS
        );
        assert_eq!(
            loaded.config.progress_stale_timeout_secs,
            MAX_PROGRESS_TIMEOUT_SECS
        );
        assert_eq!(loaded.config.interface_light_theme, "Ayu Light");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shell_features_assemble_into_the_env_string() {
        assert_eq!(ShellFeatures { path: true }.to_env_string(), "path");
        assert_eq!(ShellFeatures { path: false }.to_env_string(), "");
        // The path feature ships on, so the value migrated into the Default profile carries it. (The
        // assembled env string now comes from the profile — see `resolve_spawn_config`.)
        assert!(AppSettings::default().legacy_shell_settings().shell_integration_path);
    }

    /// The global theme a terminal without an override inherits, for the merge tests.
    fn global_theme() -> &'static TerminalTheme {
        AppSettings::default().terminal_theme(true)
    }

    /// The global terminal layer for the merge tests, with the dark half in effect.
    fn global_layer() -> GlobalTerminalLayer {
        GlobalTerminalLayer {
            theme: global_theme(),
            use_dark: true,
        }
    }

    /// A profile appearance overriding both terminal halves with the same name — the "looks the same in
    /// either appearance" shape the single `theme` slot used to express.
    fn both_halves(name: &str) -> eggie_domain::ProfileAppearance {
        eggie_domain::ProfileAppearance {
            terminal_dark_theme: Some(name.to_owned()),
            terminal_light_theme: Some(name.to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn appearance_without_overrides_is_the_global_layer() {
        let config = AppSettings::default();
        let resolved = resolve_appearance(&config, None, global_layer(), None);
        assert_eq!(resolved.theme.name, global_theme().name);
        assert_eq!(resolved.font_family.as_ref(), config.font_family.as_str());
        assert_eq!(resolved.font_size, config.font_size);

        // An all-inherit override behaves identically to no profile at all.
        let empty = eggie_domain::ProfileAppearance::default();
        assert_eq!(resolve_appearance(&config, Some(&empty), global_layer(), None), resolved);
    }

    #[test]
    fn profile_overrides_win_field_by_field() {
        let mut config = AppSettings::default();
        config.font_family = "Menlo".to_owned();
        config.font_size = 14.;

        // Only the size is overridden: family and theme still come from the global layer.
        let appearance = eggie_domain::ProfileAppearance {
            font_size: Some(18.),
            ..Default::default()
        };
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
        assert_eq!(resolved.font_size, 18.);
        assert_eq!(resolved.font_family.as_ref(), "Menlo");
        assert_eq!(resolved.theme.name, global_theme().name);

        // Filling both halves with one name is how a "prod is red" profile keeps its marking when the
        // system flips — the single-slot behaviour, now stated explicitly rather than implied.
        let named = theme_catalog().dark_names().into_iter().next().unwrap();
        // Guard the test itself: if the catalog ever ships a theme that sorts first AND is the global
        // default, the assertions below would degenerate into `global == global` and pass vacuously.
        assert_ne!(
            named,
            global_theme().name,
            "the override must name a theme that differs from the global one"
        );
        let appearance = both_halves(&named);
        for use_dark in [true, false] {
            let global = GlobalTerminalLayer {
                theme: config.terminal_theme(use_dark),
                use_dark,
            };
            let resolved = resolve_appearance(&config, Some(&appearance), global, None);
            assert_eq!(
                resolved.theme.name, named,
                "with both halves set, the override must hold in both appearances"
            );
        }
    }

    #[test]
    fn a_profile_override_follows_the_appearance_half_by_half() {
        // What the split buys: a profile can mark itself differently (or not at all) per appearance.
        let config = AppSettings::default();
        let dark_name = theme_catalog().dark_names().into_iter().next().unwrap();
        let appearance = eggie_domain::ProfileAppearance {
            terminal_dark_theme: Some(dark_name.clone()),
            terminal_light_theme: None,
            ..Default::default()
        };

        // Dark half: the override applies.
        let resolved = resolve_appearance(
            &config,
            Some(&appearance),
            GlobalTerminalLayer {
                theme: config.terminal_theme(true),
                use_dark: true,
            },
            None,
        );
        assert_eq!(resolved.theme.name, dark_name);

        // Light half: only the dark slot is set, so this one inherits the global terminal theme. This is
        // the case the old single-slot model could not express.
        let light_global = config.terminal_theme(false);
        let resolved = resolve_appearance(
            &config,
            Some(&appearance),
            GlobalTerminalLayer {
                theme: light_global,
                use_dark: false,
            },
            None,
        );
        assert_eq!(resolved.theme.name, light_global.name);
    }

    #[test]
    fn a_profile_preview_overrides_the_stored_slot() {
        // The hover preview has to win over the profile's own stored value, or hovering an option on a
        // profile that already has one would show nothing.
        let config = AppSettings::default();
        let stored = theme_catalog().dark_names().into_iter().next().unwrap();
        let preview = theme_catalog().theme_by_name("Catppuccin Mocha").unwrap();
        assert_ne!(stored, preview.name, "the preview must differ from the stored value");
        let appearance = both_halves(&stored);
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), Some(preview));
        assert_eq!(resolved.theme.name, preview.name);
    }

    #[test]
    fn two_terminals_in_one_window_resolve_independently() {
        // The central premise: one window, one global layer, two terminals that must not agree.
        let mut config = AppSettings::default();
        config.font_family = "Menlo".to_owned();
        config.font_size = 14.;
        let global = global_theme();

        let themed = eggie_domain::ProfileAppearance {
            // A light theme in the dark half is fine here: the profile layer resolves by name across the
            // whole catalog, and the point of this test is only that the two terminals disagree.
            terminal_dark_theme: theme_catalog().light_names().into_iter().next(),
            font_family: Some("SF Mono".to_owned()),
            font_size: Some(20.),
            ..Default::default()
        };
        let overridden = resolve_appearance(&config, Some(&themed), global_layer(), None);
        // A terminal with no profile (adopted/restored, or a profile with no overrides) follows global.
        let inherited = resolve_appearance(&config, None, global_layer(), None);

        assert_ne!(
            overridden.theme.name, inherited.theme.name,
            "the themed terminal must not share the other's palette"
        );
        assert_eq!(inherited.theme.name, global.name);
        assert_eq!(overridden.font_size, 20.);
        assert_eq!(inherited.font_size, 14.);
        assert_eq!(overridden.font_family.as_ref(), "SF Mono");
        assert_eq!(inherited.font_family.as_ref(), "Menlo");
    }

    #[test]
    fn an_override_matching_the_global_theme_resolves_to_the_same_palette() {
        // Worth pinning because the daemon push is diffed on the resolved palette: a profile that names
        // the theme already in effect must produce no change, hence no redundant SetAppearance.
        let config = AppSettings::default();
        let global = global_theme();
        let appearance = both_halves(&global.name);
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
        assert_eq!(resolved.theme.appearance(), global.appearance());
    }

    #[test]
    fn an_unknown_theme_name_falls_back_to_the_global_theme() {
        let config = AppSettings::default();
        let appearance = both_halves("No Such Theme");
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
        assert_eq!(
            resolved.theme.name,
            global_theme().name,
            "a typo must degrade to no override, not to an arbitrary theme"
        );
    }

    #[test]
    fn a_profile_font_family_moves_the_styles_that_inherited_it() {
        let mut config = AppSettings::default();
        config.font_family = "Menlo".to_owned();
        // Italic has its own family; bold and bold-italic inherit the regular one.
        config.font_family_italic = "Courier New".to_owned();

        let appearance = eggie_domain::ProfileAppearance {
            font_family: Some("SF Mono".to_owned()),
            ..Default::default()
        };
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
        assert_eq!(resolved.font_family.as_ref(), "SF Mono");
        assert_eq!(resolved.families.regular.as_ref(), "SF Mono");
        assert_eq!(
            resolved.families.bold.as_ref(),
            "SF Mono",
            "a slot that inherited the regular family must follow the override"
        );
        assert_eq!(resolved.families.bold_italic.as_ref(), "SF Mono");
        assert_eq!(
            resolved.families.italic.as_ref(),
            "Courier New",
            "a slot with its own explicit family keeps it"
        );
    }

    #[test]
    fn a_style_pinned_to_the_regular_family_is_still_treated_as_explicit() {
        let mut config = AppSettings::default();
        config.font_family = "Menlo".to_owned();
        // Deliberately pinning bold to the same name as regular: `resolved_font_families` collapses
        // this into the same string as "unset", so inheritance must be read from the source config.
        config.font_family_bold = "Menlo".to_owned();

        let appearance = eggie_domain::ProfileAppearance {
            font_family: Some("SF Mono".to_owned()),
            ..Default::default()
        };
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
        assert_eq!(resolved.families.regular.as_ref(), "SF Mono");
        assert_eq!(
            resolved.families.bold.as_ref(),
            "Menlo",
            "an explicitly pinned style must not follow the profile's family override"
        );
        // The genuinely-unset styles do follow it.
        assert_eq!(resolved.families.italic.as_ref(), "SF Mono");
        assert_eq!(resolved.families.bold_italic.as_ref(), "SF Mono");
    }

    #[test]
    fn a_profile_font_size_is_clamped_and_sanitized() {
        let config = AppSettings::default();
        let resolve = |size: f32| {
            let appearance = eggie_domain::ProfileAppearance {
                font_size: Some(size),
                ..Default::default()
            };
            resolve_appearance(&config, Some(&appearance), global_layer(), None).font_size
        };
        assert_eq!(resolve(1_000.), MAX_FONT_SIZE);
        assert_eq!(resolve(1.), MIN_FONT_SIZE);
        assert_eq!(resolve(f32::NAN), config.font_size, "a non-finite override is ignored");
    }

    #[test]
    fn a_blank_profile_font_family_is_not_an_override() {
        let mut config = AppSettings::default();
        config.font_family = "Menlo".to_owned();
        let appearance = eggie_domain::ProfileAppearance {
            font_family: Some("   ".to_owned()),
            ..Default::default()
        };
        let resolved = resolve_appearance(&config, Some(&appearance), global_layer(), None);
        assert_eq!(
            resolved.font_family.as_ref(),
            "Menlo",
            "whitespace must mean inherit, matching how the global per-style families behave"
        );
    }

    #[test]
    fn legacy_settings_use_ghostty_terminal_padding_defaults() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "theme_mode": "system",
                "dark_theme": "Builtin Dark",
                "light_theme": "Builtin Light",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();

        assert_eq!(config.terminal_padding_x, DEFAULT_TERMINAL_PADDING_X);
        assert_eq!(config.terminal_padding_y, DEFAULT_TERMINAL_PADDING_Y);
        assert_eq!(config.minimum_contrast, DEFAULT_MINIMUM_CONTRAST);
        assert_eq!(
            config.progress_complete_timeout_secs,
            DEFAULT_PROGRESS_COMPLETE_TIMEOUT_SECS
        );
        assert_eq!(
            config.progress_stale_timeout_secs,
            DEFAULT_PROGRESS_STALE_TIMEOUT_SECS
        );
    }

    #[test]
    fn legacy_settings_without_keybindings_field_load_empty() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn bell_mode_defaults_to_flash() {
        assert_eq!(BellMode::default(), BellMode::Flash);
        assert_eq!(AppSettings::default().bell_mode, BellMode::Flash);
    }

    #[test]
    fn copy_on_select_defaults_to_enabled() {
        assert!(AppSettings::default().copy_on_select);
    }

    #[test]
    fn legacy_settings_without_copy_on_select_default_to_enabled() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert!(config.copy_on_select);
    }

    #[test]
    fn cursor_shape_defaults_to_block() {
        assert_eq!(CursorShapeSetting::default(), CursorShapeSetting::Block);
        assert_eq!(
            AppSettings::default().cursor_shape,
            CursorShapeSetting::Block
        );
    }

    #[test]
    fn legacy_settings_without_cursor_shape_default_to_block() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert_eq!(config.cursor_shape, CursorShapeSetting::Block);
    }

    #[test]
    fn cursor_blink_defaults_to_program() {
        assert_eq!(CursorBlink::default(), CursorBlink::Program);
        assert_eq!(AppSettings::default().cursor_blink, CursorBlink::Program);
    }

    #[test]
    fn cursor_blink_resolve_follows_the_three_states() {
        // Program follows the program-reported bit.
        assert!(CursorBlink::Program.resolve(true));
        assert!(!CursorBlink::Program.resolve(false));
        // On/Off force the outcome regardless of the program.
        assert!(CursorBlink::On.resolve(false));
        assert!(!CursorBlink::Off.resolve(true));
    }

    #[test]
    fn legacy_settings_without_cursor_blink_default_to_program() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert_eq!(config.cursor_blink, CursorBlink::Program);
    }

    #[test]
    fn legacy_settings_without_bell_mode_default_to_flash() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "theme_mode": "system",
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert_eq!(config.bell_mode, BellMode::Flash);
    }

    #[test]
    fn bell_mode_sound_and_flash_predicates() {
        assert!(!BellMode::Silent.plays_sound() && !BellMode::Silent.flashes());
        assert!(BellMode::Flash.flashes() && !BellMode::Flash.plays_sound());
        assert!(BellMode::Sound.plays_sound() && !BellMode::Sound.flashes());
        assert!(BellMode::FlashAndSound.plays_sound() && BellMode::FlashAndSound.flashes());
    }

    #[test]
    fn normalize_drops_unknown_action_ids() {
        let mut config = AppSettings::default();
        config
            .keybindings
            .insert("not_a_real_action".to_owned(), "cmd-shift-z".to_owned());
        config.normalize();
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn normalize_drops_invalid_keystrokes() {
        let mut config = AppSettings::default();
        // Two non-modifier keys is not a valid single keystroke.
        config
            .keybindings
            .insert("terminal_copy".to_owned(), "cmd-a-b".to_owned());
        config.normalize();
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn normalize_drops_overrides_equal_to_default() {
        let mut config = AppSettings::default();
        // terminal_copy's default is cmd-c; storing it as an override is redundant.
        config
            .keybindings
            .insert("terminal_copy".to_owned(), "cmd-c".to_owned());
        config.normalize();
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn normalize_keeps_valid_overrides() {
        let mut config = AppSettings::default();
        config
            .keybindings
            .insert("terminal_copy".to_owned(), "cmd-shift-c".to_owned());
        config.normalize();
        assert_eq!(
            config.keybindings.get("terminal_copy").map(String::as_str),
            Some("cmd-shift-c")
        );
    }

    #[test]
    fn keybindings_only_serialize_when_present() {
        let config = AppSettings::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("keybindings"),
            "empty keybindings should be skipped: {json}"
        );

        let mut with_override = AppSettings::default();
        with_override
            .keybindings
            .insert("terminal_copy".to_owned(), "cmd-shift-c".to_owned());
        let json = serde_json::to_string(&with_override).unwrap();
        assert!(json.contains("keybindings"));
        assert!(json.contains("terminal_copy"));
    }

    #[test]
    fn minimum_contrast_matches_ghostty_black_or_white_fallback() {
        assert_eq!(minimum_contrast_rgb(0x222222, 0x222222, 1.), 0x222222);
        assert_eq!(minimum_contrast_rgb(0x222222, 0x222222, 1.1), 0xffffff);
        assert_eq!(minimum_contrast_rgb(0xeeeeee, 0xeeeeee, 1.1), 0x000000);
        assert_eq!(minimum_contrast_rgb(0xffffff, 0x000000, 21.), 0xffffff);
    }

    #[test]
    fn metric_modifier_parses_percent_and_absolute() {
        assert_eq!(MetricModifier::parse("2"), Some(MetricModifier::Absolute(2)));
        assert_eq!(
            MetricModifier::parse("-1"),
            Some(MetricModifier::Absolute(-1))
        );
        assert!(matches!(
            MetricModifier::parse("20%"),
            Some(MetricModifier::Percent(m)) if (m - 1.2).abs() < 1e-9
        ));
        assert!(matches!(
            MetricModifier::parse("-20%"),
            Some(MetricModifier::Percent(m)) if (m - 0.8).abs() < 1e-9
        ));
        // ≤ -100% collapses the metric to a zero multiplier (Ghostty semantics).
        assert!(matches!(
            MetricModifier::parse("-150%"),
            Some(MetricModifier::Percent(m)) if m == 0.0
        ));
        // Empty / garbage → None so bad input never persists.
        assert_eq!(MetricModifier::parse(""), None);
        assert_eq!(MetricModifier::parse("  "), None);
        assert_eq!(MetricModifier::parse("abc"), None);
        assert_eq!(MetricModifier::parse("1.5"), None); // not an integer delta
    }

    #[test]
    fn metric_modifier_apply_scales_or_adds() {
        assert!((MetricModifier::Percent(1.2).apply(10.) - 12.).abs() < 1e-6);
        assert!((MetricModifier::Absolute(3).apply(10.) - 13.).abs() < 1e-6);
        assert!((MetricModifier::Absolute(-4).apply(10.) - 6.).abs() < 1e-6);
    }

    #[test]
    fn resolved_adjust_honors_floor_and_absence() {
        assert_eq!(
            ResolvedMetricAdjustments::adjust(None, 10., 1.),
            10.
        );
        assert!(
            (ResolvedMetricAdjustments::adjust(Some(MetricModifier::Absolute(2)), 10., 1.) - 12.)
                .abs()
                < 1e-6
        );
        // A large negative delta is clamped to the floor.
        assert_eq!(
            ResolvedMetricAdjustments::adjust(Some(MetricModifier::Absolute(-100)), 3., 1.),
            1.
        );
    }

    #[test]
    fn normalize_drops_invalid_metric_adjustments_and_keeps_valid_ones() {
        let mut config = AppSettings::default();
        config.font_metrics.cell_height = Some("20%".to_owned());
        config.font_metrics.underline_thickness = Some("bogus".to_owned());
        config.font_metrics.cursor_thickness = Some("".to_owned());
        config.font_metrics.icon_height = Some("-15%".to_owned());
        config.normalize();
        assert_eq!(config.font_metrics.cell_height.as_deref(), Some("20%"));
        assert_eq!(config.font_metrics.underline_thickness, None);
        assert_eq!(config.font_metrics.cursor_thickness, None);
        assert_eq!(config.font_metrics.icon_height.as_deref(), Some("-15%"));
        assert!(matches!(
            config.font_metrics.resolve().icon_height,
            Some(MetricModifier::Percent(m)) if (m - 0.85).abs() < 1e-9
        ));
    }

    #[test]
    fn font_metrics_only_serialize_when_present() {
        let config = AppSettings::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("font_metrics"),
            "empty font_metrics should be skipped: {json}"
        );

        let mut with_adjust = AppSettings::default();
        with_adjust.font_metrics.cell_height = Some("2".to_owned());
        let json = serde_json::to_string(&with_adjust).unwrap();
        assert!(json.contains("font_metrics"));
        assert!(json.contains("cell_height"));
    }

    #[test]
    fn legacy_settings_without_font_metrics_default_to_empty() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert_eq!(config.font_metrics, FontMetricAdjustments::default());
    }

    #[test]
    fn scrollback_and_shell_default_and_serialize_only_when_non_default() {
        let config = AppSettings::default();
        assert_eq!(config.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
        assert!(config.shell_program.is_empty());
        assert!(config.shell_args.is_empty());
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("scrollback_lines"), "default skipped: {json}");
        assert!(!json.contains("shell_program"));
        assert!(!json.contains("shell_args"));

        let mut custom = AppSettings::default();
        custom.scrollback_lines = 500;
        custom.shell_program = "/opt/homebrew/bin/fish".to_owned();
        custom.shell_args = vec!["-l".to_owned()];
        let json = serde_json::to_string(&custom).unwrap();
        let round_trip: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip.scrollback_lines, 500);
        assert_eq!(round_trip.shell_program, "/opt/homebrew/bin/fish");
        assert_eq!(round_trip.shell_args, vec!["-l".to_owned()]);
    }

    #[test]
    fn legacy_settings_without_scrollback_or_shell_use_defaults() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert_eq!(config.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
        assert!(config.shell_program.is_empty());
        assert!(config.shell_args.is_empty());
    }

    #[test]
    fn normalize_clamps_scrollback_and_cleans_shell_fields() {
        let mut config = AppSettings::default();
        config.scrollback_lines = MAX_SCROLLBACK_LINES + 5_000;
        config.shell_program = "  /bin/fish  ".to_owned();
        config.shell_args = vec![" -l ".to_owned(), "  ".to_owned(), String::new()];
        config.normalize();
        assert_eq!(config.scrollback_lines, MAX_SCROLLBACK_LINES);
        assert_eq!(config.shell_program, "/bin/fish");
        // Blank/whitespace-only args dropped; the meaningful one is kept (untrimmed value preserved
        // beyond the emptiness check — argv semantics are the daemon's concern).
        assert_eq!(config.shell_args, vec![" -l ".to_owned()]);

        // Whitespace-only shell path normalizes to empty (=> $SHELL fallback).
        let mut blank = AppSettings::default();
        blank.shell_program = "   ".to_owned();
        blank.normalize();
        assert!(blank.shell_program.is_empty());
    }

    #[test]
    fn per_style_families_default_to_empty_and_fall_back_to_regular() {
        let config = AppSettings::default();
        assert!(config.font_family_bold.is_empty());
        assert!(config.font_family_italic.is_empty());
        assert!(config.font_family_bold_italic.is_empty());
        let families = config.resolved_font_families();
        // Every style resolves to the regular family when no dedicated family is set.
        assert_eq!(families.family(false, false).as_ref(), config.font_family);
        assert_eq!(families.family(true, false).as_ref(), config.font_family);
        assert_eq!(families.family(false, true).as_ref(), config.font_family);
        assert_eq!(families.family(true, true).as_ref(), config.font_family);
        assert!(!families.has_dedicated_family(true, false));
    }

    #[test]
    fn dedicated_bold_family_is_selected_for_bold_style() {
        let mut config = AppSettings::default();
        config.font_family = "Menlo".to_owned();
        config.font_family_bold = "Menlo Bold".to_owned();
        let families = config.resolved_font_families();
        assert_eq!(families.family(true, false).as_ref(), "Menlo Bold");
        assert!(families.has_dedicated_family(true, false));
        // Italic has no dedicated family, so it still falls back to regular.
        assert_eq!(families.family(false, true).as_ref(), "Menlo");
        assert!(!families.has_dedicated_family(false, true));
    }

    #[test]
    fn font_synthetic_style_defaults_to_all_enabled_and_gates_synthesis() {
        let config = AppSettings::default();
        assert_eq!(config.font_synthetic_style, FontSyntheticStyle::default());
        let families = config.resolved_font_families();
        assert!(families.allows_synthesis(true, false));
        assert!(families.allows_synthesis(false, true));

        let mut disabled = AppSettings::default();
        disabled.font_synthetic_style.italic = false;
        let families = disabled.resolved_font_families();
        assert!(!families.allows_synthesis(false, true));
        assert!(families.allows_synthesis(true, false));
    }

    #[test]
    fn legacy_settings_without_font_family_styles_default_to_empty_and_synthetic_on() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert!(config.font_family_bold.is_empty());
        assert!(config.font_family_italic.is_empty());
        assert!(config.font_family_bold_italic.is_empty());
        assert_eq!(config.font_synthetic_style, FontSyntheticStyle::default());
    }

    #[test]
    fn normalize_clears_whitespace_only_style_families() {
        let mut config = AppSettings::default();
        config.font_family_bold = "   ".to_owned();
        config.normalize();
        assert!(config.font_family_bold.is_empty());
    }

    #[test]
    fn font_feature_parses_harfbuzz_forms() {
        let liga_on = FontFeature::parse("liga").unwrap();
        assert_eq!(&liga_on.tag, b"liga");
        assert_eq!(liga_on.value, 1);
        assert_eq!(FontFeature::parse("+calt").unwrap().value, 1);
        assert_eq!(FontFeature::parse("-calt").unwrap().value, 0);
        assert_eq!(FontFeature::parse("calt off").unwrap().value, 0);
        assert_eq!(FontFeature::parse("calt on").unwrap().value, 1);
        assert_eq!(FontFeature::parse("cv01=2").unwrap().value, 2);
        assert_eq!(&FontFeature::parse("\"ss01\"").unwrap().tag, b"ss01");
        // Tags must be exactly four ASCII characters.
        assert_eq!(FontFeature::parse("lig"), None);
        assert_eq!(FontFeature::parse("ligas"), None);
        assert_eq!(FontFeature::parse(""), None);
        assert_eq!(FontFeature::parse("liga=x"), None);
    }

    #[test]
    fn ligatures_default_on_and_resolve_to_no_features() {
        let config = AppSettings::default();
        assert!(config.ligatures);
        assert!(config.font_shaping_break_cursor);
        assert!(config.resolved_font_features().is_empty());
    }

    #[test]
    fn ligatures_off_disables_liga_calt_dlig() {
        let mut config = AppSettings::default();
        config.ligatures = false;
        let features = config.resolved_font_features();
        let disabled: Vec<_> = features
            .iter()
            .filter(|f| f.value == 0)
            .map(|f| f.tag)
            .collect();
        assert!(disabled.contains(b"liga"));
        assert!(disabled.contains(b"calt"));
        assert!(disabled.contains(b"dlig"));
    }

    #[test]
    fn user_font_features_append_after_master_toggle() {
        let mut config = AppSettings::default();
        config.ligatures = false;
        config.font_features = vec!["+liga".to_owned(), "ss01".to_owned(), "bogus".to_owned()];
        let features = config.resolved_font_features();
        // The last `liga` entry (user +liga) wins under HarfBuzz "later wins" ordering.
        let last_liga = features.iter().rev().find(|f| &f.tag == b"liga").unwrap();
        assert_eq!(last_liga.value, 1);
        assert!(features.iter().any(|f| &f.tag == b"ss01" && f.value == 1));
        // Invalid entries are dropped.
        assert!(features.len() == 3 /* liga,calt,dlig */ + 2 /* +liga, ss01 */);
    }

    #[test]
    fn legacy_settings_without_ligature_fields_default_on() {
        let config: AppSettings = serde_json::from_str(
            r#"{
                "font_family": "Menlo",
                "font_size": 14
            }"#,
        )
        .unwrap();
        assert!(config.ligatures);
        assert!(config.font_shaping_break_cursor);
        assert!(config.font_features.is_empty());
    }

    #[test]
    fn default_ligature_fields_are_not_serialized() {
        let config = AppSettings::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("ligatures"), "default ligatures skipped: {json}");
        assert!(!json.contains("font_shaping_break_cursor"));
        assert!(!json.contains("font_features"));
    }

    #[test]
    fn font_variation_parses_axis_settings() {
        let wght = FontVariation::parse("wght=200").unwrap();
        assert_eq!(&wght.tag, b"wght");
        assert!((wght.value - 200.).abs() < 1e-6);
        assert!((FontVariation::parse("slnt=-10").unwrap().value + 10.).abs() < 1e-6);
        assert!((FontVariation::parse(" wdth = 87.5 ").unwrap().value - 87.5).abs() < 1e-6);
        // Tag must be exactly four chars; value must be a finite number.
        assert_eq!(FontVariation::parse("wght"), None);
        assert_eq!(FontVariation::parse("wg=100"), None);
        assert_eq!(FontVariation::parse("wght=abc"), None);
    }

    #[test]
    fn resolved_font_variations_drops_invalid_entries() {
        let mut config = AppSettings::default();
        config.font_variations = vec!["wght=300".to_owned(), "bad".to_owned()];
        let variations = config.resolved_font_variations();
        assert_eq!(variations.len(), 1);
        assert_eq!(&variations[0].tag, b"wght");
    }

    #[test]
    fn thicken_defaults_off_at_full_strength_and_is_not_serialized() {
        let config = AppSettings::default();
        assert!(!config.font_thicken);
        assert_eq!(config.font_thicken_strength, 255);
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("font_thicken"), "defaults skipped: {json}");
        assert!(!json.contains("font_variations"));
    }

    #[test]
    fn legacy_settings_without_variation_or_thicken_use_defaults() {
        let config: AppSettings = serde_json::from_str(
            r#"{ "font_family": "Menlo", "font_size": 14 }"#,
        )
        .unwrap();
        assert!(config.font_variations.is_empty());
        assert!(!config.font_thicken);
        assert_eq!(config.font_thicken_strength, 255);
    }

    #[test]
    fn codepoint_map_parses_ranges_and_single_points() {
        let range = CodepointMapEntry::parse("U+E000-U+E00A=Symbols Nerd Font").unwrap();
        assert_eq!(range.start, 0xE000);
        assert_eq!(range.end, 0xE00A);
        assert_eq!(range.family, "Symbols Nerd Font");
        assert!(range.contains(0xE005));
        assert!(!range.contains(0xE00B));

        let single = CodepointMapEntry::parse("U+2764=Apple Color Emoji").unwrap();
        assert_eq!(single.start, 0x2764);
        assert_eq!(single.end, 0x2764);

        // Malformed entries are rejected.
        assert_eq!(CodepointMapEntry::parse("E000=Foo"), None); // missing U+
        assert_eq!(CodepointMapEntry::parse("U+E000="), None); // empty family
        assert_eq!(CodepointMapEntry::parse("U+E00A-U+E000=Foo"), None); // reversed range
        assert_eq!(CodepointMapEntry::parse("nonsense"), None);
    }

    #[test]
    fn resolved_codepoint_map_drops_invalid_entries() {
        let mut config = AppSettings::default();
        config.font_codepoint_map =
            vec!["U+E000-U+E0FF=Nerd".to_owned(), "garbage".to_owned()];
        let map = config.resolved_codepoint_map();
        assert_eq!(map.len(), 1);
        assert_eq!(map[0].family, "Nerd");
        // Default is empty and not serialized.
        assert!(AppSettings::default().resolved_codepoint_map().is_empty());
        let json = serde_json::to_string(&AppSettings::default()).unwrap();
        assert!(!json.contains("font_codepoint_map"));
    }
}

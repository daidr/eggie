use gpui::Context;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, process::Command, sync::OnceLock};

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

const DEFAULT_PALETTE: [u32; 16] = [
    0x1d2027, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x5c6370,
    0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
];

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
    pub(crate) dark_theme: String,
    pub(crate) light_theme: String,
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
            dark_theme: DEFAULT_DARK_THEME.to_owned(),
            light_theme: DEFAULT_LIGHT_THEME.to_owned(),
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
    /// Assemble the `EGGIE_SHELL_FEATURES` string from the enabled feature toggles.
    pub(crate) fn shell_features_string(&self) -> String {
        ShellFeatures {
            path: self.shell_integration_path,
        }
        .to_env_string()
    }

    fn normalize(&mut self) {
        let catalog = theme_catalog();
        if catalog.dark_theme(&self.dark_theme).is_none() {
            self.dark_theme = DEFAULT_DARK_THEME.to_owned();
        }
        if catalog.light_theme(&self.light_theme).is_none() {
            self.light_theme = DEFAULT_LIGHT_THEME.to_owned();
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

    pub(crate) fn effective_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        let use_dark = match self.theme_mode {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => system_is_dark,
        };
        let catalog = theme_catalog();
        if use_dark {
            catalog
                .dark_theme(&self.dark_theme)
                .or_else(|| catalog.dark_theme(DEFAULT_DARK_THEME))
                .unwrap_or(&catalog.dark[0])
        } else {
            catalog
                .light_theme(&self.light_theme)
                .or_else(|| catalog.light_theme(DEFAULT_LIGHT_THEME))
                .unwrap_or(&catalog.light[0])
        }
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

pub(crate) struct SettingsStore {
    config: AppSettings,
    path: PathBuf,
    /// Transient theme override driven by the settings window's hover preview. Never persisted and
    /// never serialized — it only overrides what `resolved_theme` returns while the pointer rests on
    /// a theme option. Cleared when the pointer leaves, the dropdown closes, or the window closes.
    theme_preview: Option<&'static TerminalTheme>,
}

impl SettingsStore {
    pub(crate) fn load() -> Self {
        Self::load_from(settings_path())
    }

    fn load_from(path: PathBuf) -> Self {
        let mut config = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AppSettings>(&bytes).ok())
            .unwrap_or_default();
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

    /// The terminal theme actually in effect: the live hover preview if one is active, otherwise the
    /// persisted `effective_theme`. Both the main window's terminals and the settings preview read
    /// through here so a hovered theme lights up everywhere at once.
    pub(crate) fn resolved_theme(&self, system_is_dark: bool) -> &'static TerminalTheme {
        self.theme_preview
            .unwrap_or_else(|| self.config.effective_theme(system_is_dark))
    }

    /// Set the transient hover preview to `theme`. No-op (and no notify) if it is already the active
    /// preview, so a stream of repeated hover events over the same option doesn't churn renders.
    pub(crate) fn set_theme_preview(
        &mut self,
        theme: &'static TerminalTheme,
        cx: &mut Context<Self>,
    ) {
        if self
            .theme_preview
            .is_some_and(|current| std::ptr::eq(current, theme))
        {
            return;
        }
        self.theme_preview = Some(theme);
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
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let encoded = serde_json::to_vec_pretty(&self.config)?;
        let temporary_path = self.path.with_extension("json.tmp");
        fs::write(&temporary_path, encoded)?;
        fs::rename(temporary_path, &self.path)
    }
}

fn settings_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    return home
        .join("Library")
        .join("Application Support")
        .join("Eggie")
        .join("settings.json");
    #[cfg(not(target_os = "macos"))]
    return home.join(".config").join("eggie").join("settings.json");
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
}

impl UiColors {
    pub(crate) fn from_theme(theme: &TerminalTheme) -> Self {
        let dark = theme.is_dark();
        Self {
            background: theme.background,
            panel: mix(
                theme.background,
                theme.foreground,
                if dark { 0.035 } else { 0.025 },
            ),
            panel_alt: mix(
                theme.background,
                theme.foreground,
                if dark { 0.075 } else { 0.06 },
            ),
            hover: mix(
                theme.background,
                theme.foreground,
                if dark { 0.1 } else { 0.085 },
            ),
            border: mix(
                theme.background,
                theme.foreground,
                if dark { 0.14 } else { 0.16 },
            ),
            text: theme.foreground,
            muted: mix(
                theme.background,
                theme.foreground,
                if dark { 0.55 } else { 0.58 },
            ),
            accent: theme.palette[4],
        }
    }
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

    #[test]
    fn resolved_theme_prefers_the_hover_preview_then_falls_back() {
        let catalog = theme_catalog();
        let mut store = SettingsStore {
            config: AppSettings {
                theme_mode: ThemeMode::Dark,
                dark_theme: "Builtin Dark".to_owned(),
                ..AppSettings::default()
            },
            path: std::path::PathBuf::from("unused"),
            theme_preview: None,
        };

        // No preview active: resolves to the persisted effective theme.
        assert_eq!(store.resolved_theme(true).name, "Builtin Dark");

        // A preview override wins over the persisted theme, regardless of light/dark or system mode.
        let preview = catalog.theme_by_name("Catppuccin Mocha").unwrap();
        store.theme_preview = Some(preview);
        assert_eq!(store.resolved_theme(true).name, "Catppuccin Mocha");
        assert_eq!(store.resolved_theme(false).name, "Catppuccin Mocha");

        // Clearing the override restores the persisted theme.
        store.theme_preview = None;
        assert_eq!(store.resolved_theme(true).name, "Builtin Dark");
    }

    #[test]
    fn settings_round_trip_and_normalize_font_size() {
        let directory =
            std::env::temp_dir().join(format!("eggie-settings-{}", uuid::Uuid::new_v4()));
        let path = directory.join("settings.json");
        let mut config = AppSettings {
            language: Language::English,
            theme_mode: ThemeMode::Light,
            dark_theme: "Catppuccin Mocha".to_owned(),
            light_theme: "Ayu Light".to_owned(),
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
        };
        config.normalize();
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();

        let loaded = SettingsStore::load_from(path);
        assert_eq!(loaded.config.font_size, MAX_FONT_SIZE);
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
        assert_eq!(loaded.config.light_theme, "Ayu Light");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shell_features_assemble_into_the_env_string() {
        assert_eq!(ShellFeatures { path: true }.to_env_string(), "path");
        assert_eq!(ShellFeatures { path: false }.to_env_string(), "");
        // Default settings enable the path feature.
        assert_eq!(AppSettings::default().shell_features_string(), "path");
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

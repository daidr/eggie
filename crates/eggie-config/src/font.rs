use serde::{Deserialize, Serialize};

/// A parsed variable-font axis setting, matching Ghostty's `font-variation` syntax `id=value`
/// where `id` is a four-character axis tag (`wght`, `slnt`, `wdth`, `opsz`, …) and `value` a float.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontVariation {
    pub tag: [u8; 4],
    pub value: f32,
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
    pub fn parse(text: &str) -> Option<Self> {
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
pub struct CodepointMapEntry {
    pub start: u32,
    pub end: u32,
    pub family: String,
}

impl CodepointMapEntry {
    pub fn parse(text: &str) -> Option<Self> {
        let (range, family) = text.split_once('=')?;
        let family = family.trim();
        if family.is_empty() {
            return None;
        }
        let range = range.trim();
        let parse_cp = |token: &str| -> Option<u32> {
            let hex = token
                .trim()
                .strip_prefix("U+")
                .or_else(|| token.trim().strip_prefix("u+"))?;
            u32::from_str_radix(hex, 16)
                .ok()
                .filter(|cp| char::from_u32(*cp).is_some())
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

    pub fn contains(&self, codepoint: u32) -> bool {
        (self.start..=self.end).contains(&codepoint)
    }
}

/// A parsed OpenType feature override, matching Ghostty's `font-feature` / HarfBuzz syntax.
/// Accepts `feat`, `+feat`, `feat on`, `feat=1` (enable); `-feat`, `feat off`, `feat=0` (disable);
/// and `feat=N` for a specific selector value. The tag is exactly four ASCII characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontFeature {
    /// The four-byte OpenType feature tag (e.g. `liga`, `calt`, `ss01`).
    pub tag: [u8; 4],
    /// The selector value: 0 disables, 1 enables, >1 selects an alternate.
    pub value: u32,
}

impl FontFeature {
    pub fn parse(text: &str) -> Option<Self> {
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
pub enum MetricModifier {
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
    pub fn parse(text: &str) -> Option<Self> {
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
    pub fn apply(self, base: f32) -> f32 {
        match self {
            Self::Percent(multiplier) => base * multiplier as f32,
            Self::Absolute(delta) => base + delta as f32,
        }
    }

    /// Convert to device space for a given `scale_factor`. `Percent` is scale-independent (a pure
    /// multiplier) and passes through; `Absolute` is a logical-pixel delta, so it is scaled to
    /// device pixels. This lets a modifier be baked into a device-space cache key (`GlyphKey`) and
    /// applied later without needing the scale factor again.
    pub fn prescale(self, scale_factor: f32) -> Self {
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
pub struct ResolvedMetricAdjustments {
    pub cell_width: Option<MetricModifier>,
    pub cell_height: Option<MetricModifier>,
    pub font_baseline: Option<MetricModifier>,
    pub underline_position: Option<MetricModifier>,
    pub underline_thickness: Option<MetricModifier>,
    pub strikethrough_position: Option<MetricModifier>,
    pub strikethrough_thickness: Option<MetricModifier>,
    pub cursor_thickness: Option<MetricModifier>,
    pub box_thickness: Option<MetricModifier>,
    pub icon_height: Option<MetricModifier>,
}

impl ResolvedMetricAdjustments {
    /// Optionally apply a modifier to a base pixel value, clamping to `floor`.
    pub fn adjust(modifier: Option<MetricModifier>, base: f32, floor: f32) -> f32 {
        match modifier {
            Some(modifier) => modifier.apply(base).max(floor),
            None => base,
        }
    }
}

impl FontMetricAdjustments {
    /// Parse every field into its `MetricModifier` form once (invalid entries become `None`).
    pub fn resolve(&self) -> ResolvedMetricAdjustments {
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

/// Which styles Ghostty-style synthesis is allowed to fabricate when a font lacks a native face,
/// matching `font-synthetic-style`. Default: all enabled. When disabled for a style, a missing face
/// falls back to the regular font instead of synthesizing bold (stroke) / italic (skew).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSyntheticStyle {
    pub bold: bool,
    pub italic: bool,
    pub bold_italic: bool,
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
    pub fn is_all_enabled(&self) -> bool {
        *self == Self::default()
    }
}

/// The four resolved font families (regular + per-style, each already falling back to regular when
/// unset) plus the synthetic-style policy, threaded through the render pipeline. Cheap to clone.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedFontFamilies {
    pub regular: std::sync::Arc<str>,
    pub bold: std::sync::Arc<str>,
    pub italic: std::sync::Arc<str>,
    pub bold_italic: std::sync::Arc<str>,
    pub synthetic: FontSyntheticStyle,
}

impl ResolvedFontFamilies {
    /// Select the family for a `(bold, italic)` style. Every slot already falls back to regular, so
    /// this is a direct pick.
    pub fn family(&self, bold: bool, italic: bool) -> &std::sync::Arc<str> {
        match (bold, italic) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }

    /// Whether synthesis (CoreText symbolic-trait substitution) is allowed for a `(bold, italic)`
    /// style. Only meaningful when that style has no dedicated family (otherwise the family wins).
    pub fn allows_synthesis(&self, bold: bool, italic: bool) -> bool {
        match (bold, italic) {
            (false, false) => true,
            (true, false) => self.synthetic.bold,
            (false, true) => self.synthetic.italic,
            (true, true) => self.synthetic.bold_italic,
        }
    }

    /// Whether the `(bold, italic)` style resolved to a dedicated (non-regular) family.
    pub fn has_dedicated_family(&self, bold: bool, italic: bool) -> bool {
        !std::sync::Arc::ptr_eq(self.family(bold, italic), &self.regular)
            && self.family(bold, italic).as_ref() != self.regular.as_ref()
    }
}

/// Per-metric adjustments matching Ghostty's `adjust-*` config keys. Each is an optional
/// human-editable string (`"2"`, `"-1"`, `"20%"`); `None`/absent means "use the font-derived
/// value unchanged". Only metrics Eggie actually renders are exposed — `adjust-overline-*` is
/// omitted because the vte kernel never emits an overline attribute (no SGR 53), so it would be
/// dead config.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontMetricAdjustments {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell_width: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell_height: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_baseline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub underline_thickness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strikethrough_position: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strikethrough_thickness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_thickness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub box_thickness: Option<String>,
    /// Percentage or pixel adjustment to the constraint height of Nerd Font icons. Unlike the
    /// other metrics this scales fallback icon glyphs at raster time rather than moving a
    /// decoration line; `None` leaves icons at their font-native size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_height: Option<String>,
}

impl FontMetricAdjustments {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Drop any entry that isn't a valid `MetricModifier` string so bad input never persists and
    /// downstream code can treat "present" as "valid".
    pub fn normalize(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metric_modifier_parses_percent_and_absolute() {
        assert_eq!(
            MetricModifier::parse("2"),
            Some(MetricModifier::Absolute(2))
        );
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
}

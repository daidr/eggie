//! Embedded terminal themes and semantic interface colors; independent of GPUI and Lua.
use eggie_domain::DEFAULT_PALETTE;
use std::sync::OnceLock;
include!(concat!(env!("OUT_DIR"), "/ghostty_themes.rs"));
pub const DEFAULT_DARK_THEME: &str = "Builtin Dark";
pub const DEFAULT_LIGHT_THEME: &str = "Builtin Light";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalTheme {
    pub name: String,
    pub palette: [u32; 16],
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    pub cursor_text: u32,
    pub selection_background: u32,
    pub selection_foreground: u32,
}

impl TerminalTheme {
    pub fn appearance(&self) -> eggie_domain::TerminalAppearance {
        eggie_domain::TerminalAppearance {
            palette: self.palette,
            foreground: self.foreground,
            background: self.background,
            cursor: self.cursor,
            cursor_text: self.cursor_text,
        }
    }

    pub fn is_dark(&self) -> bool {
        let red = ((self.background >> 16) & 0xff) as f32 / 255.;
        let green = ((self.background >> 8) & 0xff) as f32 / 255.;
        let blue = (self.background & 0xff) as f32 / 255.;
        0.2126 * red + 0.7152 * green + 0.0722 * blue < 0.5
    }
}

pub struct ThemeCatalog {
    dark: Vec<TerminalTheme>,
    light: Vec<TerminalTheme>,
}

impl ThemeCatalog {
    pub fn fallback_theme(&self, dark: bool) -> &TerminalTheme {
        if dark { &self.dark[0] } else { &self.light[0] }
    }
    pub fn dark_names(&self) -> Vec<String> {
        self.dark.iter().map(|theme| theme.name.clone()).collect()
    }

    pub fn light_names(&self) -> Vec<String> {
        self.light.iter().map(|theme| theme.name.clone()).collect()
    }

    pub fn dark_theme(&self, name: &str) -> Option<&TerminalTheme> {
        self.dark.iter().find(|theme| theme.name == name)
    }

    pub fn light_theme(&self, name: &str) -> Option<&TerminalTheme> {
        self.light.iter().find(|theme| theme.name == name)
    }

    /// Look a theme up by name across both the dark and light lists. Used by the settings
    /// window's live hover preview, which resolves whichever theme the pointer is over.
    pub fn theme_by_name(&self, name: &str) -> Option<&TerminalTheme> {
        self.dark_theme(name).or_else(|| self.light_theme(name))
    }
}

pub fn theme_catalog() -> &'static ThemeCatalog {
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
pub struct UiColors {
    pub background: u32,
    pub panel: u32,
    pub panel_alt: u32,
    pub hover: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_foreground: u32,
    pub accent_hover: u32,
}

impl UiColors {
    pub fn from_theme(theme: &TerminalTheme) -> Self {
        let dark = theme.is_dark();
        let chroma = rgb_chroma(theme.background);
        // Near-neutral terminal backgrounds borrow a hue from their own palette.
        let tint_source = if chroma > 0.025 {
            theme.background
        } else {
            theme.palette[4]
        };
        let (hue, saturation, _) = rgb_to_hsl(tint_source);
        let neutral = if dark { 0x171b20 } else { 0xf4f5f7 };
        let tinted_neutral = mix(
            neutral,
            hsl_to_rgb(
                hue,
                if dark { 0.26 } else { 0.25 },
                if dark { 0.12 } else { 0.95 },
            ),
            0.4 * (saturation / 0.2).clamp(0., 1.),
        );
        let background = mix(
            tinted_neutral,
            theme.background,
            if dark { 0.43 } else { 0.48 },
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
                mix(background, 0xffffff, 0.5),
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
        let accent_foreground =
            if contrast_ratio_rgb(0xffffff, accent) >= contrast_ratio_rgb(0x000000, accent) {
                0xffffff
            } else {
                0x000000
            };
        let accent_hover = mix(
            accent,
            if accent_foreground == 0xffffff {
                0x000000
            } else {
                0xffffff
            },
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

fn rgb_chroma(color: u32) -> f32 {
    let channels = [
        ((color >> 16) & 0xff) as f32,
        ((color >> 8) & 0xff) as f32,
        (color & 0xff) as f32,
    ];
    (channels.iter().copied().fold(0., f32::max) - channels.iter().copied().fold(255., f32::min))
        / 255.
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

pub fn minimum_contrast_rgb(foreground: u32, background: u32, minimum_contrast: f32) -> u32 {
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
            write!(
                html,
                "<section id=\"{}\"><h2>{group} · {} themes</h2><div class=grid>",
                group.to_lowercase(),
                themes.len()
            )
            .unwrap();
            for theme in themes {
                let ui = UiColors::from_theme(theme);
                let (theme_hue, _, _) = rgb_to_hsl(theme.background);
                let (ui_hue, ui_saturation, _) = rgb_to_hsl(ui.background);
                let theme_chroma = rgb_chroma(theme.background);
                if theme_chroma > 0.05 {
                    let hue_distance = (theme_hue - ui_hue).abs();
                    if hue_distance.min(1. - hue_distance) > 30. / 360. || ui_saturation < 0.08 {
                        failures.push(format!(
                            "{}: interface background lost theme hue",
                            theme.name
                        ));
                    }
                } else if theme_chroma < 0.025
                    && rgb_chroma(theme.palette[4]) < 0.025
                    && rgb_chroma(ui.background) > 0.06
                {
                    failures.push(format!(
                        "{}: neutral theme gained an unrelated hue",
                        theme.name
                    ));
                }
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
                    (
                        "button text / hover",
                        ui.accent_foreground,
                        ui.accent_hover,
                        4.5,
                    ),
                ];
                for (label, foreground, background, minimum) in checks {
                    let ratio = contrast_ratio_rgb(foreground, background);
                    if ratio < minimum {
                        failures.push(format!(
                            "{}: {label} contrast {ratio:.2} < {minimum}",
                            theme.name
                        ));
                    }
                }
                let name = theme
                    .name
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('"', "&quot;");
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
}

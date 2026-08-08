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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    pub(crate) language: Language,
    pub(crate) theme_mode: ThemeMode,
    pub(crate) dark_theme: String,
    pub(crate) light_theme: String,
    pub(crate) font_family: String,
    pub(crate) font_size: f32,
    pub(crate) terminal_padding_x: f32,
    pub(crate) terminal_padding_y: f32,
    pub(crate) minimum_contrast: f32,
    pub(crate) progress_complete_timeout_secs: u32,
    pub(crate) progress_stale_timeout_secs: u32,
    pub(crate) allow_osc_clipboard_read: bool,
    pub(crate) detect_urls: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme_mode: ThemeMode::System,
            dark_theme: DEFAULT_DARK_THEME.to_owned(),
            light_theme: DEFAULT_LIGHT_THEME.to_owned(),
            font_family: DEFAULT_FONT_FAMILY.to_owned(),
            font_size: DEFAULT_FONT_SIZE,
            terminal_padding_x: DEFAULT_TERMINAL_PADDING_X,
            terminal_padding_y: DEFAULT_TERMINAL_PADDING_Y,
            minimum_contrast: DEFAULT_MINIMUM_CONTRAST,
            progress_complete_timeout_secs: DEFAULT_PROGRESS_COMPLETE_TIMEOUT_SECS,
            progress_stale_timeout_secs: DEFAULT_PROGRESS_STALE_TIMEOUT_SECS,
            allow_osc_clipboard_read: false,
            detect_urls: true,
        }
    }
}

impl AppSettings {
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
        Self { config, path }
    }

    pub(crate) fn config(&self) -> &AppSettings {
        &self.config
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

    fn dark_theme(&self, name: &str) -> Option<&TerminalTheme> {
        self.dark.iter().find(|theme| theme.name == name)
    }

    fn light_theme(&self, name: &str) -> Option<&TerminalTheme> {
        self.light.iter().find(|theme| theme.name == name)
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
            font_size: 100.,
            terminal_padding_x: -20.,
            terminal_padding_y: f32::NAN,
            minimum_contrast: 99.,
            progress_complete_timeout_secs: 0,
            progress_stale_timeout_secs: u32::MAX,
            allow_osc_clipboard_read: true,
            detect_urls: false,
        };
        config.normalize();
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();

        let loaded = SettingsStore::load_from(path);
        assert_eq!(loaded.config.font_size, MAX_FONT_SIZE);
        assert_eq!(loaded.config.terminal_padding_x, MIN_TERMINAL_PADDING);
        assert_eq!(loaded.config.terminal_padding_y, DEFAULT_TERMINAL_PADDING_Y);
        assert_eq!(loaded.config.minimum_contrast, MAX_MINIMUM_CONTRAST);
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
    fn minimum_contrast_matches_ghostty_black_or_white_fallback() {
        assert_eq!(minimum_contrast_rgb(0x222222, 0x222222, 1.), 0x222222);
        assert_eq!(minimum_contrast_rgb(0x222222, 0x222222, 1.1), 0xffffff);
        assert_eq!(minimum_contrast_rgb(0xeeeeee, 0xeeeeee, 1.1), 0x000000);
        assert_eq!(minimum_contrast_rgb(0xffffff, 0x000000, 21.), 0xffffff);
    }
}

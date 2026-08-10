use std::sync::OnceLock;

use crate::{
    icons::{IconName, icon, icon_sized},
    settings::{
        AppSettings, BellMode, CursorBlink, CursorShapeSetting, Language, MAX_FONT_SIZE,
        MAX_MINIMUM_CONTRAST, MAX_PROGRESS_TIMEOUT_SECS, MAX_TERMINAL_PADDING, MIN_FONT_SIZE,
        MIN_MINIMUM_CONTRAST, MIN_PROGRESS_TIMEOUT_SECS, MIN_TERMINAL_PADDING, SettingsStore,
        TerminalTheme, ThemeMode, UiColors, minimum_contrast_rgb, theme_catalog,
    },
    text_input::{TextInput, TextInputEvent, TextInputStyle},
};
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Entity, Menu,
    MenuItem, MouseButton, OsAction, Pixels, ScrollHandle, SharedString, SystemMenuType,
    TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowControlArea, WindowOptions,
    actions, anchored, deferred, div, point, prelude::*, px, rgb, size,
};

#[cfg(not(target_os = "macos"))]
use gpui::font;

actions!(
    eggie,
    [
        OpenSettings,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        TerminalCopy,
        TerminalPaste,
        TerminalSelectAll,
        TerminalFind,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        SplitRight,
        SplitDown,
        ClearScreen,
        ScrollTop,
        ScrollBottom,
        PageUp,
        PageDown,
        FontIncrease,
        FontDecrease,
        FontReset,
        JumpPrevPrompt,
        JumpNextPrompt
    ]
);

const SETTINGS_WIDTH: f32 = 680.;
const SETTINGS_HEIGHT: f32 = 540.;
const SETTINGS_MIN_WIDTH: f32 = 640.;
const SETTINGS_MIN_HEIGHT: f32 = 420.;
const SETTINGS_TITLEBAR_HEIGHT: f32 = 52.;
const SETTINGS_TRAFFIC_LIGHT_DIAMETER: f32 = 14.;
const SETTINGS_TRAFFIC_LIGHT_LEFT: f32 = 12.;
const SETTINGS_TITLE_LEFT_INSET: f32 = 84.;
const SETTINGS_SIDEBAR_WIDTH: f32 = 168.;
const SETTINGS_SCROLLBAR_WIDTH: f32 = 8.;
const SELECTOR_WIDTH: f32 = 270.;
const SELECTOR_DROPDOWN_HEIGHT: f32 = 208.;
const SELECTOR_DROPDOWN_GAP: f32 = 4.;
const SELECTOR_WINDOW_MARGIN: f32 = 8.;
/// Width of the compact bell-mode dropdown (trigger and popover).
const BELL_DROPDOWN_WIDTH: f32 = 180.;
const BELL_DROPDOWN_ROW_HEIGHT: f32 = 30.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectorKind {
    DarkTheme,
    LightTheme,
    Font,
    FontBold,
    FontItalic,
    FontBoldItalic,
}

impl SelectorKind {
    const fn index(self) -> usize {
        match self {
            Self::DarkTheme => 0,
            Self::LightTheme => 1,
            Self::Font => 2,
            Self::FontBold => 3,
            Self::FontItalic => 4,
            Self::FontBoldItalic => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaddingAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProgressTimeoutKind {
    Complete,
    Stale,
}

/// Which synthetic style the toggle controls (`font-synthetic-style`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyntheticStyleKind {
    Bold,
    Italic,
    BoldItalic,
}

impl SyntheticStyleKind {
    fn slug(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::BoldItalic => "bold-italic",
        }
    }
}

/// The ten renderable font-metric adjustments (`adjust-*`). The settings window edits each as an
/// integer pixel delta (0 = unset / use the font-derived value); percent forms remain editable by
/// hand in settings.json. `adjust-overline-*` is intentionally absent — the vte kernel never emits
/// an overline attribute, so it would be dead config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetricAdjustmentKind {
    CellWidth,
    CellHeight,
    FontBaseline,
    UnderlinePosition,
    UnderlineThickness,
    StrikethroughPosition,
    StrikethroughThickness,
    CursorThickness,
    BoxThickness,
    IconHeight,
}

impl MetricAdjustmentKind {
    const ALL: [Self; 10] = [
        Self::CellWidth,
        Self::CellHeight,
        Self::FontBaseline,
        Self::UnderlinePosition,
        Self::UnderlineThickness,
        Self::StrikethroughPosition,
        Self::StrikethroughThickness,
        Self::CursorThickness,
        Self::BoxThickness,
        Self::IconHeight,
    ];

    /// A stable ascii slug for element ids (not user-facing).
    fn slug(self) -> &'static str {
        match self {
            Self::CellWidth => "cell-width",
            Self::CellHeight => "cell-height",
            Self::FontBaseline => "font-baseline",
            Self::UnderlinePosition => "underline-position",
            Self::UnderlineThickness => "underline-thickness",
            Self::StrikethroughPosition => "strikethrough-position",
            Self::StrikethroughThickness => "strikethrough-thickness",
            Self::CursorThickness => "cursor-thickness",
            Self::BoxThickness => "box-thickness",
            Self::IconHeight => "icon-height",
        }
    }

    /// Mutable access to the backing field within [`FontMetricAdjustments`].
    fn slot(self, metrics: &mut crate::settings::FontMetricAdjustments) -> &mut Option<String> {
        match self {
            Self::CellWidth => &mut metrics.cell_width,
            Self::CellHeight => &mut metrics.cell_height,
            Self::FontBaseline => &mut metrics.font_baseline,
            Self::UnderlinePosition => &mut metrics.underline_position,
            Self::UnderlineThickness => &mut metrics.underline_thickness,
            Self::StrikethroughPosition => &mut metrics.strikethrough_position,
            Self::StrikethroughThickness => &mut metrics.strikethrough_thickness,
            Self::CursorThickness => &mut metrics.cursor_thickness,
            Self::BoxThickness => &mut metrics.box_thickness,
            Self::IconHeight => &mut metrics.icon_height,
        }
    }

    fn value(self, metrics: &crate::settings::FontMetricAdjustments) -> &Option<String> {
        match self {
            Self::CellWidth => &metrics.cell_width,
            Self::CellHeight => &metrics.cell_height,
            Self::FontBaseline => &metrics.font_baseline,
            Self::UnderlinePosition => &metrics.underline_position,
            Self::UnderlineThickness => &metrics.underline_thickness,
            Self::StrikethroughPosition => &metrics.strikethrough_position,
            Self::StrikethroughThickness => &metrics.strikethrough_thickness,
            Self::CursorThickness => &metrics.cursor_thickness,
            Self::BoxThickness => &metrics.box_thickness,
            Self::IconHeight => &metrics.icon_height,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsSection {
    #[default]
    General,
    Appearance,
    Keybindings,
    Advanced,
}

impl SettingsSection {
    const ALL: [Self; 4] = [
        Self::General,
        Self::Appearance,
        Self::Keybindings,
        Self::Advanced,
    ];
}

pub(crate) fn install(settings: Entity<SettingsStore>, cx: &mut App) {
    let settings_for_action = settings.clone();
    cx.on_action(move |_: &OpenSettings, cx| open_settings_window(settings_for_action.clone(), cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    // Build the keymap from settings (defaults + user overrides). This also
    // registers the text-input bindings in the correct order to preserve the
    // binding-index tiebreak. Action handlers above are registered exactly once
    // and are intentionally *not* rebuilt when the keymap changes.
    let config = settings.read(cx).config().clone();
    crate::keybindings::rebuild_keymap(&config, cx);
    let language = config.language;
    cx.set_menus(build_menus(language));

    let settings_for_observer = settings.clone();
    cx.observe(&settings, move |_, cx| {
        let config = settings_for_observer.read(cx).config().clone();
        // Rebuild the keymap first: `set_menus` snapshots the current keymap to resolve each
        // menu item's shortcut, so it must run *after* the keymap reflects this change or the
        // menu bar lags one edit behind.
        crate::keybindings::rebuild_keymap(&config, cx);
        cx.set_menus(build_menus(config.language));
    })
    .detach();
}

fn build_menus(language: Language) -> [Menu; 2] {
    [
        Menu::new("Eggie").items([
            MenuItem::action(language.settings_menu_item(), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(language.hide_eggie(), Hide),
            MenuItem::action(language.hide_others(), HideOthers),
            MenuItem::action(language.show_all(), ShowAll),
            MenuItem::separator(),
            MenuItem::action(language.quit_eggie(), Quit),
        ]),
        Menu::new(language.edit_menu()).items([
            MenuItem::os_action(language.copy(), TerminalCopy, OsAction::Copy),
            MenuItem::os_action(language.paste(), TerminalPaste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action(language.select_all(), TerminalSelectAll, OsAction::SelectAll),
        ]),
    ]
}

fn open_settings_window(settings: Entity<SettingsStore>, cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<SettingsWindow>())
    {
        existing
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }

    let fonts = monospace_font_names(cx);
    let language = settings.read(cx).config().language;
    cx.open_window(
        WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(language.settings_window_title().into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(
                    px(SETTINGS_TRAFFIC_LIGHT_LEFT),
                    px((SETTINGS_TITLEBAR_HEIGHT - SETTINGS_TRAFFIC_LIGHT_DIAMETER) / 2.),
                )),
            }),
            app_owns_titlebar_drag: true,
            window_bounds: Some(WindowBounds::centered(
                size(px(SETTINGS_WIDTH), px(SETTINGS_HEIGHT)),
                cx,
            )),
            window_min_size: Some(size(px(SETTINGS_MIN_WIDTH), px(SETTINGS_MIN_HEIGHT))),
            focus: true,
            ..Default::default()
        },
        move |window, cx| cx.new(|cx| SettingsWindow::new(settings.clone(), fonts, window, cx)),
    )
    .expect("failed to open Eggie settings window");
}

fn monospace_font_names(cx: &App) -> Vec<String> {
    static CACHED: OnceLock<Vec<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| monospace_font_names_uncached(cx))
        .clone()
}

#[cfg(target_os = "macos")]
fn monospace_font_names_uncached(_cx: &App) -> Vec<String> {
    use core_foundation::{array::CFArray, base::TCFType};
    use core_text::font_collection::create_for_all_families;
    use core_text::font_descriptor::{CTFontDescriptor, SymbolicTraitAccessors, TraitAccessors};
    use std::collections::HashSet;

    let collection = create_for_all_families();
    let descriptors: Option<CFArray<CTFontDescriptor>> = unsafe {
        unsafe extern "C" {
            fn CTFontCollectionCreateMatchingFontDescriptors(
                collection: core_text::font_collection::CTFontCollectionRef,
            ) -> core_foundation::array::CFArrayRef;
        }
        let array_ref =
            CTFontCollectionCreateMatchingFontDescriptors(collection.as_concrete_TypeRef());
        if array_ref.is_null() {
            None
        } else {
            Some(CFArray::wrap_under_create_rule(array_ref))
        }
    };

    let Some(descriptors) = descriptors else {
        return Vec::new();
    };

    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for descriptor in descriptors.into_iter() {
        if !descriptor.traits().symbolic_traits().is_monospace() {
            continue;
        }
        let name = descriptor.family_name();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names.sort_unstable_by_key(|name| name.to_lowercase());
    if !names.iter().any(|name| name == "Menlo") {
        names.insert(0, "Menlo".to_owned());
    }
    names
}

#[cfg(not(target_os = "macos"))]
fn monospace_font_names_uncached(cx: &App) -> Vec<String> {
    let text_system = cx.text_system();
    let mut names = text_system
        .all_font_names()
        .into_iter()
        .filter(|name| {
            let font_id = text_system.resolve_font(&font(name.clone()));
            let narrow = text_system.advance(font_id, px(14.), 'i').ok();
            let wide = text_system.advance(font_id, px(14.), 'm').ok();
            match (narrow, wide) {
                (Some(narrow), Some(wide)) => {
                    (f32::from(narrow.width) - f32::from(wide.width)).abs() < 0.01
                }
                _ => false,
            }
        })
        .collect::<Vec<_>>();
    names.sort_unstable_by_key(|name| name.to_lowercase());
    names.dedup();
    if !names.iter().any(|name| name == "Menlo") {
        names.insert(0, "Menlo".to_owned());
    }
    names
}

struct SettingsWindow {
    settings: Entity<SettingsStore>,
    dark_theme_names: Vec<String>,
    light_theme_names: Vec<String>,
    font_names: Vec<String>,
    colors: UiColors,
    open_selector: Option<SelectorKind>,
    selector_search_input: Entity<TextInput>,
    selector_scroll_handle: ScrollHandle,
    settings_scroll_handle: ScrollHandle,
    selector_bounds: [Option<Bounds<Pixels>>; 6],
    moving_window: bool,
    selected_section: SettingsSection,
    /// The action id currently being recorded (its shortcut cell is capturing keys), if any.
    recording: Option<String>,
    /// Keystroke interceptor active while recording; dropping it uninstalls the capture.
    recording_subscription: Option<gpui::Subscription>,
    /// When a recorded keystroke collides with another action, its label (for the conflict hint).
    recording_conflict: Option<String>,
    /// Whether the bell-mode dropdown popover is open.
    bell_dropdown_open: bool,
    /// Bounds of the bell-mode dropdown trigger, for positioning the popover.
    bell_dropdown_bounds: Option<Bounds<Pixels>>,
}

impl SettingsWindow {
    fn new(
        settings: Entity<SettingsStore>,
        font_names: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let system_is_dark = is_dark_appearance(window.appearance());
        let theme = settings.read(cx).config().effective_theme(system_is_dark);
        let colors = UiColors::from_theme(theme);
        let selector_search_input = cx.new(|cx| {
            TextInput::new(
                window,
                cx,
                TextInputStyle {
                    text_color: (colors.text << 8) | 0xff,
                    placeholder_color: (colors.muted << 8) | 0xff,
                    cursor_color: (colors.accent << 8) | 0xff,
                    selection_color: (colors.accent << 8) | 0x55,
                },
            )
        });
        cx.subscribe_in(&selector_search_input, window, Self::on_selector_search_event)
            .detach();
        Self {
            settings,
            dark_theme_names: theme_catalog().dark_names(),
            light_theme_names: theme_catalog().light_names(),
            font_names,
            colors,
            open_selector: None,
            selector_search_input,
            selector_scroll_handle: ScrollHandle::new(),
            settings_scroll_handle: ScrollHandle::new(),
            selector_bounds: [None; 6],
            moving_window: false,
            selected_section: SettingsSection::default(),
            recording: None,
            recording_subscription: None,
            recording_conflict: None,
            bell_dropdown_open: false,
            bell_dropdown_bounds: None,
        }
    }

    /// React to the selector filter input: content changes re-render the filtered list; Enter
    /// applies the first match; Escape closes the dropdown without changing the selection.
    fn on_selector_search_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                self.selector_scroll_handle
                    .set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }
            TextInputEvent::Confirm | TextInputEvent::ConfirmReverse => {
                self.apply_first_selector_match(cx);
            }
            TextInputEvent::Cancel => {
                self.open_selector = None;
                cx.notify();
            }
        }
    }

    /// Apply the first item matching the current filter for the open selector, then close it.
    fn apply_first_selector_match(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.open_selector else {
            return;
        };
        let query = self.selector_search_input.read(cx).content().trim().to_owned();
        let names = match kind {
            SelectorKind::DarkTheme => &self.dark_theme_names,
            SelectorKind::LightTheme => &self.light_theme_names,
            SelectorKind::Font
            | SelectorKind::FontBold
            | SelectorKind::FontItalic
            | SelectorKind::FontBoldItalic => &self.font_names,
        };
        let Some(choice) = names
            .iter()
            .find(|item| selector_matches(item, &query))
            .cloned()
        else {
            return;
        };
        self.settings.update(cx, |store, cx| {
            store.update(
                |config| match kind {
                    SelectorKind::DarkTheme => config.dark_theme = choice.clone(),
                    SelectorKind::LightTheme => config.light_theme = choice.clone(),
                    SelectorKind::Font => config.font_family = choice.clone(),
                    SelectorKind::FontBold => config.font_family_bold = choice.clone(),
                    SelectorKind::FontItalic => config.font_family_italic = choice.clone(),
                    SelectorKind::FontBoldItalic => {
                        config.font_family_bold_italic = choice.clone()
                    }
                },
                cx,
            )
        });
        self.open_selector = None;
        cx.notify();
    }

    fn set_theme_mode(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.theme_mode = mode, cx)
        });
    }

    fn set_bell_mode(&mut self, mode: BellMode, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.bell_mode = mode, cx)
        });
    }

    fn set_osc_clipboard_read(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.allow_osc_clipboard_read = enabled, cx)
        });
    }

    fn set_detect_urls(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.detect_urls = enabled, cx)
        });
    }

    fn set_copy_on_select(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.copy_on_select = enabled, cx)
        });
    }

    fn set_ligatures(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.ligatures = enabled, cx)
        });
    }

    fn set_shaping_break_cursor(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.font_shaping_break_cursor = enabled, cx)
        });
    }

    fn set_font_thicken(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.font_thicken = enabled, cx)
        });
    }

    fn change_font_thicken_strength(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    settings.font_thicken_strength = (i32::from(settings.font_thicken_strength)
                        + delta)
                        .clamp(0, 255) as u8;
                },
                cx,
            )
        });
    }

    fn set_synthetic_style(
        &mut self,
        kind: SyntheticStyleKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let field = match kind {
                        SyntheticStyleKind::Bold => &mut settings.font_synthetic_style.bold,
                        SyntheticStyleKind::Italic => &mut settings.font_synthetic_style.italic,
                        SyntheticStyleKind::BoldItalic => {
                            &mut settings.font_synthetic_style.bold_italic
                        }
                    };
                    *field = enabled;
                },
                cx,
            )
        });
    }

    fn set_cursor_shape(&mut self, shape: CursorShapeSetting, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.cursor_shape = shape, cx)
        });
    }

    fn set_cursor_blink(&mut self, blink: CursorBlink, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.cursor_blink = blink, cx)
        });
    }

    fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.language = language, cx)
        });
    }

    fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| settings.font_size = (settings.font_size + delta).round(),
                cx,
            )
        });
    }

    fn change_minimum_contrast(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    settings.minimum_contrast =
                        ((settings.minimum_contrast + delta) * 10.).round() / 10.;
                },
                cx,
            )
        });
    }

    fn change_terminal_padding(&mut self, axis: PaddingAxis, delta: f32, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let value = match axis {
                        PaddingAxis::Horizontal => &mut settings.terminal_padding_x,
                        PaddingAxis::Vertical => &mut settings.terminal_padding_y,
                    };
                    *value = (*value + delta).round();
                },
                cx,
            )
        });
    }

    /// Step a font-metric adjustment by `delta` integer pixels. The GUI only edits the integer
    /// (absolute) form; a value of 0 clears the entry back to "unset". A hand-written percent value
    /// is treated as a 0 baseline for stepping (the next step replaces it with an integer delta).
    fn change_metric_adjustment(
        &mut self,
        kind: MetricAdjustmentKind,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let slot = kind.slot(&mut settings.font_metrics);
                    let current = slot
                        .as_deref()
                        .and_then(|text| text.trim().parse::<i32>().ok())
                        .unwrap_or(0);
                    let next = (current + delta).clamp(-64, 64);
                    *slot = if next == 0 {
                        None
                    } else {
                        Some(next.to_string())
                    };
                },
                cx,
            )
        });
    }

    fn change_progress_timeout(
        &mut self,
        kind: ProgressTimeoutKind,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let value = match kind {
                        ProgressTimeoutKind::Complete => {
                            &mut settings.progress_complete_timeout_secs
                        }
                        ProgressTimeoutKind::Stale => &mut settings.progress_stale_timeout_secs,
                    };
                    *value = (i64::from(*value) + i64::from(delta)).clamp(
                        i64::from(MIN_PROGRESS_TIMEOUT_SECS),
                        i64::from(MAX_PROGRESS_TIMEOUT_SECS),
                    ) as u32;
                },
                cx,
            )
        });
    }

    fn render_mode_control(&self, selected: ThemeMode, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        ThemeMode::ALL
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, mode| {
                    let label = match mode {
                        ThemeMode::Dark => language.theme_mode_dark(),
                        ThemeMode::Light => language.theme_mode_light(),
                        ThemeMode::System => language.theme_mode_system(),
                    };
                    control.child(
                        div()
                            .id(format!("theme-mode-{}", mode.label()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected == mode, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_theme_mode(mode, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn bell_mode_label(language: Language, mode: BellMode) -> &'static str {
        match mode {
            BellMode::Silent => language.bell_mode_silent(),
            BellMode::Flash => language.bell_mode_flash(),
            BellMode::Sound => language.bell_mode_sound(),
            BellMode::FlashAndSound => language.bell_mode_flash_and_sound(),
        }
    }

    fn render_bell_control(
        &self,
        selected: BellMode,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let dropdown_position = self.bell_dropdown_bounds.map(|bounds| {
            let popover_height = px(BELL_DROPDOWN_ROW_HEIGHT * BellMode::ALL.len() as f32 + 8.);
            let gap = px(SELECTOR_DROPDOWN_GAP);
            let margin = px(SELECTOR_WINDOW_MARGIN);
            let y = if window.viewport_size().height - bounds.bottom()
                >= popover_height + gap + margin
            {
                bounds.bottom() + gap
            } else {
                bounds.top() - popover_height - gap
            };
            point(bounds.left(), y)
        });
        let control = cx.entity().downgrade();
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let control = control.clone();
                cx.defer(move |cx| {
                    control
                        .update(cx, |settings, cx| {
                            if settings.bell_dropdown_bounds != Some(bounds) {
                                settings.bell_dropdown_bounds = Some(bounds);
                                if settings.bell_dropdown_open {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .id("bell-mode-container")
            .relative()
            .w(px(BELL_DROPDOWN_WIDTH))
            .h(px(32.))
            .child(
                div()
                    .id("bell-mode-trigger")
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .size_full()
                    .px_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(self.colors.panel_alt))
                    .cursor_pointer()
                    .hover(|element| element.border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.bell_dropdown_open = !settings.bell_dropdown_open;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(Self::bell_mode_label(language, selected)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(self.colors.muted))
                            .child(icon(IconName::ArrowDown)),
                    ),
            )
            .when(self.bell_dropdown_open, |control| {
                let mut dropdown = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    dropdown = dropdown.position(position);
                }
                control.child(
                    deferred(dropdown.child(self.render_bell_dropdown(selected, language, cx)))
                        .priority(2),
                )
            })
            .into_any_element()
    }

    fn render_bell_dropdown(
        &self,
        selected: BellMode,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = BellMode::ALL.into_iter().fold(
            div().flex().flex_col().py_1(),
            |list, mode| {
                let is_selected = mode == selected;
                list.child(
                    div()
                        .id(SharedString::from(format!("bell-mode-{}", mode.slug())))
                        .flex()
                        .items_center()
                        .flex_none()
                        .h(px(BELL_DROPDOWN_ROW_HEIGHT))
                        .mx_1()
                        .px_3()
                        .rounded_md()
                        .cursor_pointer()
                        .when(is_selected, |element| {
                            element
                                .bg(rgb(self.colors.panel_alt))
                                .text_color(rgb(self.colors.accent))
                        })
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, window, cx| {
                            settings.set_bell_mode(mode, cx);
                            settings.bell_dropdown_open = false;
                            let _ = window;
                            cx.notify();
                        }))
                        .child(Self::bell_mode_label(language, mode)),
                )
            },
        );
        div()
            .id("bell-mode-dropdown")
            .flex()
            .flex_col()
            .w(px(BELL_DROPDOWN_WIDTH))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .shadow_lg()
            // Block mouse/hover for the settings rows painted behind the popover so clicks on its
            // blank areas can't bubble through to a control underneath (canonical GPUI overlay
            // isolation). Children still receive their own events.
            .occlude()
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |settings, _, cx| {
                    if settings.bell_dropdown_open {
                        settings.bell_dropdown_open = false;
                        cx.notify();
                    }
                });
            }))
            .child(options)
            .into_any_element()
    }

    fn render_osc_clipboard_read_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.block()), (true, language.allow())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    control.child(
                        div()
                            .id(format!("osc-clipboard-read-{label}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_osc_clipboard_read(value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn render_detect_urls_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    control.child(
                        div()
                            .id(format!("detect-urls-{label}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_detect_urls(value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn render_synthetic_style_control(
        &self,
        kind: SyntheticStyleKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    let id = SharedString::from(format!("synthetic-{}-{label}", kind.slug()));
                    control.child(
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_synthetic_style(kind, value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn render_ligatures_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    control.child(
                        div()
                            .id(format!("ligatures-{label}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_ligatures(value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn render_shaping_break_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    control.child(
                        div()
                            .id(format!("shaping-break-{label}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_shaping_break_cursor(value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn render_font_thicken_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    control.child(
                        div()
                            .id(format!("font-thicken-{label}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_font_thicken(value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn render_font_thicken_strength_control(
        &self,
        strength: u8,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = |id: &'static str,
                      label: &'static str,
                      enabled: bool,
                      delta: i32,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.change_font_thicken_strength(delta, cx)
                        }))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(
                "decrease-font-thicken-strength",
                "−",
                strength > 0,
                -16,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(54.))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(format!("{strength}")),
            )
            .child(button(
                "increase-font-thicken-strength",
                "+",
                strength < 255,
                16,
                cx,
            ))
            .into_any_element()
    }

    fn render_copy_on_select_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (value, label)| {
                    control.child(
                        div()
                            .id(format!("copy-on-select-{label}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(enabled == value, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_copy_on_select(value, cx)
                            }))
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    fn metric_adjustment_labels(
        language: Language,
        kind: MetricAdjustmentKind,
    ) -> (&'static str, &'static str) {
        match kind {
            MetricAdjustmentKind::CellWidth => (
                language.adjust_cell_width_row(),
                language.adjust_cell_width_description(),
            ),
            MetricAdjustmentKind::CellHeight => (
                language.adjust_cell_height_row(),
                language.adjust_cell_height_description(),
            ),
            MetricAdjustmentKind::FontBaseline => (
                language.adjust_font_baseline_row(),
                language.adjust_font_baseline_description(),
            ),
            MetricAdjustmentKind::UnderlinePosition => (
                language.adjust_underline_position_row(),
                language.adjust_underline_position_description(),
            ),
            MetricAdjustmentKind::UnderlineThickness => (
                language.adjust_underline_thickness_row(),
                language.adjust_underline_thickness_description(),
            ),
            MetricAdjustmentKind::StrikethroughPosition => (
                language.adjust_strikethrough_position_row(),
                language.adjust_strikethrough_position_description(),
            ),
            MetricAdjustmentKind::StrikethroughThickness => (
                language.adjust_strikethrough_thickness_row(),
                language.adjust_strikethrough_thickness_description(),
            ),
            MetricAdjustmentKind::CursorThickness => (
                language.adjust_cursor_thickness_row(),
                language.adjust_cursor_thickness_description(),
            ),
            MetricAdjustmentKind::BoxThickness => (
                language.adjust_box_thickness_row(),
                language.adjust_box_thickness_description(),
            ),
            MetricAdjustmentKind::IconHeight => (
                language.adjust_icon_height_row(),
                language.adjust_icon_height_description(),
            ),
        }
    }

    fn cursor_shape_label(language: Language, shape: CursorShapeSetting) -> &'static str {
        match shape {
            CursorShapeSetting::Block => language.cursor_shape_block(),
            CursorShapeSetting::Bar => language.cursor_shape_bar(),
            CursorShapeSetting::Underline => language.cursor_shape_underline(),
            CursorShapeSetting::BlockHollow => language.cursor_shape_block_hollow(),
        }
    }

    fn render_cursor_shape_control(
        &self,
        selected: CursorShapeSetting,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        CursorShapeSetting::ALL
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, shape| {
                    control.child(
                        div()
                            .id(format!("cursor-shape-{}", shape.slug()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected == shape, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_cursor_shape(shape, cx)
                            }))
                            .child(Self::cursor_shape_label(language, shape)),
                    )
                },
            )
            .into_any_element()
    }

    fn cursor_blink_label(language: Language, blink: CursorBlink) -> &'static str {
        match blink {
            CursorBlink::Program => language.cursor_blink_program(),
            CursorBlink::On => language.cursor_blink_on(),
            CursorBlink::Off => language.cursor_blink_off(),
        }
    }

    fn render_cursor_blink_control(
        &self,
        selected: CursorBlink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        CursorBlink::ALL
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, blink| {
                    control.child(
                        div()
                            .id(format!("cursor-blink-{}", blink.slug()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected == blink, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_cursor_blink(blink, cx)
                            }))
                            .child(Self::cursor_blink_label(language, blink)),
                    )
                },
            )
            .into_any_element()
    }

    fn render_language_control(
        &self,
        selected: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Language::ALL
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, language| {
                    control.child(
                        div()
                            .id(format!("language-{}", language.label()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected == language, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_language(language, cx)
                            }))
                            .child(language.label()),
                    )
                },
            )
            .into_any_element()
    }

    fn render_selector(
        &self,
        id: &'static str,
        value: String,
        font_family: Option<String>,
        kind: SelectorKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dropdown_position = self.selector_bounds[kind.index()].map(|bounds| {
            let dropdown_height = px(SELECTOR_DROPDOWN_HEIGHT);
            let gap = px(SELECTOR_DROPDOWN_GAP);
            let margin = px(SELECTOR_WINDOW_MARGIN);
            let y = if window.viewport_size().height - bounds.bottom()
                >= dropdown_height + gap + margin
            {
                bounds.bottom() + gap
            } else {
                bounds.top() - dropdown_height - gap
            };
            point(bounds.left(), y)
        });
        let selector = cx.entity().downgrade();
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let selector = selector.clone();
                cx.defer(move |cx| {
                    selector
                        .update(cx, |settings, cx| {
                            let stored = &mut settings.selector_bounds[kind.index()];
                            if *stored != Some(bounds) {
                                *stored = Some(bounds);
                                if settings.open_selector == Some(kind) {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .id(format!("{id}-container"))
            .relative()
            .w(px(SELECTOR_WIDTH))
            .h(px(32.))
            .child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .size_full()
                    .px_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(self.colors.panel_alt))
                    .cursor_pointer()
                    .hover(|element| element.border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        if settings.open_selector == Some(kind) {
                            settings.open_selector = None;
                        } else {
                            settings.open_selector = Some(kind);
                            let language = settings.settings.read(cx).config().language;
                            let placeholder = match kind {
                                SelectorKind::DarkTheme => language.search_dark_themes(),
                                SelectorKind::LightTheme => language.search_light_themes(),
                                SelectorKind::Font
                                | SelectorKind::FontBold
                                | SelectorKind::FontItalic
                                | SelectorKind::FontBoldItalic => language.search_fonts(),
                            };
                            settings.selector_search_input.update(cx, |input, cx| {
                                input.set_placeholder(placeholder);
                                input.set_content("", cx);
                            });
                            settings
                                .selector_scroll_handle
                                .set_offset(point(px(0.), px(0.)));
                            let handle =
                                settings.selector_search_input.read(cx).focus_handle();
                            handle.focus(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when_some(font_family, |element, family| {
                                element.font_family(SharedString::from(family))
                            })
                            .child(value),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(self.colors.muted))
                            .child(icon(IconName::ArrowDown)),
                    ),
            )
            .when(self.open_selector == Some(kind), |selector| {
                let mut dropdown = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    dropdown = dropdown.position(position);
                }
                selector.child(
                    deferred(dropdown.child(self.render_selector_dropdown(kind, window, cx)))
                        .priority(2),
                )
            })
            .into_any_element()
    }

    fn render_selector_dropdown(
        &self,
        kind: SelectorKind,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = self.settings.read(cx).config();
        let language = config.language;
        let (current, items) = match kind {
            SelectorKind::DarkTheme => (
                config.dark_theme.clone(),
                self.dark_theme_names.clone(),
            ),
            SelectorKind::LightTheme => (
                config.light_theme.clone(),
                self.light_theme_names.clone(),
            ),
            SelectorKind::Font => (
                config.font_family.clone(),
                self.font_names.clone(),
            ),
            SelectorKind::FontBold => {
                (config.font_family_bold.clone(), self.font_names.clone())
            }
            SelectorKind::FontItalic => {
                (config.font_family_italic.clone(), self.font_names.clone())
            }
            SelectorKind::FontBoldItalic => (
                config.font_family_bold_italic.clone(),
                self.font_names.clone(),
            ),
        };
        let query_owned = self.selector_search_input.read(cx).content().trim().to_owned();
        let items = items
            .into_iter()
            .filter(|item| selector_matches(item, &query_owned))
            .collect::<Vec<_>>();
        let has_items = !items.is_empty();

        let mut list = div()
            .id("settings-selector-list")
            .flex()
            .flex_1()
            .min_h_0()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.selector_scroll_handle)
            .py_1();
        for (index, item) in items.into_iter().enumerate() {
            let selected = item == current;
            let selected_item = item.clone();
            list = list.child(
                div()
                    .id(("settings-selector-option", index))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .flex_none()
                    .h(px(32.))
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.accent))
                    })
                    .when(
                        matches!(
                            kind,
                            SelectorKind::Font
                                | SelectorKind::FontBold
                                | SelectorKind::FontItalic
                                | SelectorKind::FontBoldItalic
                        ),
                        |element| element.font_family(SharedString::from(item.clone())),
                    )
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.settings.update(cx, |store, cx| {
                            store.update(
                                |config| match kind {
                                    SelectorKind::DarkTheme => {
                                        config.dark_theme = selected_item.clone()
                                    }
                                    SelectorKind::LightTheme => {
                                        config.light_theme = selected_item.clone()
                                    }
                                    SelectorKind::Font => {
                                        config.font_family = selected_item.clone()
                                    }
                                    SelectorKind::FontBold => {
                                        config.font_family_bold = selected_item.clone()
                                    }
                                    SelectorKind::FontItalic => {
                                        config.font_family_italic = selected_item.clone()
                                    }
                                    SelectorKind::FontBoldItalic => {
                                        config.font_family_bold_italic = selected_item.clone()
                                    }
                                },
                                cx,
                            )
                        });
                        settings.open_selector = None;
                        cx.notify();
                    }))
                    .child(div().min_w_0().truncate().child(item)),
            );
        }
        if !has_items {
            list = list.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(self.colors.muted))
                    .child(language.no_matches()),
            );
        }

        let search = div()
            .id("settings-selector-search")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(30.))
            .px_3()
            // Flush against the dropdown's edges: no outer margin/border/radius, just a divider that
            // separates the search field from the results list below.
            .border_b_1()
            .border_color(rgb(self.colors.border))
            .text_size(px(12.))
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(self.colors.muted))
                    .child(icon_sized(IconName::Search, 13.)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .text_color(rgb(self.colors.text))
                    .child(self.selector_search_input.clone()),
            );

        div()
            .id("settings-selector-dropdown")
            .flex()
            .flex_col()
            .w(px(SELECTOR_WIDTH))
            .h(px(SELECTOR_DROPDOWN_HEIGHT))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .shadow_lg()
            // Floating overlay: block every mouse event (and hover/tooltip) for the settings rows
            // painted behind it, so clicks on the dropdown's blank areas (search padding, list gaps)
            // can't bubble through and misfire a control underneath. This is the canonical GPUI way —
            // one call replaces per-event stop_propagation patches. Children paint on top, so the
            // search input and list items still receive their own events.
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |settings, _, cx| {
                    if settings.open_selector == Some(kind) {
                        settings.open_selector = None;
                        cx.notify();
                    }
                });
            }))
            .child(search)
            .child(list)
            .into_any_element()
    }

    // --- Keyboard shortcut recording ---------------------------------------------------------

    /// Enter recording mode for `id`: install a keystroke interceptor that swallows every key
    /// (so bound shortcuts like ⌘C are captured, not triggered) and forwards it to the state
    /// machine. Dropping the returned subscription (in `cancel_recording`) uninstalls it.
    fn start_recording(&mut self, id: String, cx: &mut Context<Self>) {
        self.recording = Some(id);
        self.recording_conflict = None;
        let weak = cx.weak_entity();
        self.recording_subscription = Some(cx.intercept_keystrokes(move |event, _window, cx| {
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            weak.update(cx, |this, cx| this.on_recorded_keystroke(keystroke, cx))
                .ok();
        }));
        cx.notify();
    }

    fn on_recorded_keystroke(&mut self, keystroke: gpui::Keystroke, cx: &mut Context<Self>) {
        // Escape cancels recording.
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.cancel_recording(cx);
            return;
        }
        // Ignore the intermediate state where only modifiers are held.
        if crate::keybindings::is_bare_modifier(&keystroke.key) {
            return;
        }
        // Require at least one modifier so a bare letter cannot swallow terminal input.
        if !keystroke.modifiers.modified() {
            return;
        }
        let Some(id) = self.recording.clone() else {
            return;
        };
        let canonical = crate::keybindings::canonical_keystroke(&keystroke);
        // Reject a combination already bound to a different action.
        if let Some(other_id) = self.find_binding_conflict(&canonical, &id, cx) {
            let language = self.settings.read(cx).config().language;
            let other_label = crate::keybindings::spec_by_id(other_id)
                .map(|spec| (spec.label)(language))
                .unwrap_or(other_id);
            self.recording_conflict = Some(other_label.to_owned());
            cx.notify();
            return;
        }
        self.set_binding(id, canonical, cx);
        self.cancel_recording(cx);
    }

    fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.recording = None;
        self.recording_subscription = None;
        self.recording_conflict = None;
        cx.notify();
    }

    fn find_binding_conflict(
        &self,
        candidate: &str,
        exclude_id: &str,
        cx: &Context<Self>,
    ) -> Option<&'static str> {
        let config = self.settings.read(cx).config();
        crate::keybindings::find_conflict(config, candidate, exclude_id)
    }

    /// Persist an override for `id`. If the recorded keystroke equals the default, drop the
    /// override instead of storing a redundant entry.
    fn set_binding(&mut self, id: String, canonical: String, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |config| {
                    if crate::keybindings::default_keystroke(&id) == Some(canonical.as_str()) {
                        config.keybindings.remove(&id);
                    } else {
                        config.keybindings.insert(id.clone(), canonical.clone());
                    }
                },
                cx,
            )
        });
    }

    fn reset_binding(&mut self, id: String, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|config| {
                config.keybindings.remove(&id);
            }, cx)
        });
    }

    fn reset_all_bindings(&mut self, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|config| config.keybindings.clear(), cx)
        });
    }

    fn render_keybindings_section(
        &self,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let config = self.settings.read(cx).config().clone();
        let has_overrides = !config.keybindings.is_empty();
        let colors = self.colors;

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(language.keybindings_hint()),
            )
            .when(has_overrides, |element| {
                element.child(
                    div()
                        .id("reset-all-keybindings")
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(28.))
                        .px_3()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|element| element.bg(rgb(colors.panel_alt)))
                        .child(language.reset_all())
                        .on_click(cx.listener(|settings, _, _, cx| {
                            settings.reset_all_bindings(cx);
                        })),
                )
            })
            .into_any_element();

        let rows: Vec<AnyElement> = crate::keybindings::ACTION_SPECS
            .iter()
            .map(|spec| self.render_keybinding_row(spec, &config, language, cx))
            .collect();

        vec![
            header,
            settings_section(language.keybindings_section(), rows, colors),
        ]
    }

    fn render_keybinding_row(
        &self,
        spec: &'static crate::keybindings::ActionSpec,
        config: &AppSettings,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let id = spec.id;
        let label = (spec.label)(language);
        let effective = crate::keybindings::effective_keystroke(config, id)
            .unwrap_or(spec.default_keystroke);
        let is_overridden = config.keybindings.contains_key(id);
        let is_recording = self.recording.as_deref() == Some(id);
        let conflict = if is_recording {
            self.recording_conflict.clone()
        } else {
            None
        };

        let cell_text = if is_recording {
            if let Some(other) = conflict.as_deref() {
                language.keybind_conflict(other)
            } else {
                language.recording_prompt().to_owned()
            }
        } else {
            crate::keybindings::display_keystroke(effective)
        };
        let cell_text_color = if conflict.is_some() {
            0xff6b6b
        } else if is_recording {
            colors.accent
        } else {
            colors.text
        };
        let cell_border = if is_recording {
            colors.accent
        } else {
            colors.border
        };

        let recorded_id = id.to_string();
        let shortcut_cell = div()
            .id(SharedString::from(format!("keybind-{id}")))
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.))
            .min_w(px(84.))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(cell_border))
            .text_size(px(13.))
            .text_color(rgb(cell_text_color))
            .cursor_pointer()
            .when(!is_recording, |element| {
                element.hover(|element| element.bg(rgb(colors.panel_alt)))
            })
            .when(is_recording, |element| element.bg(rgb(colors.panel_alt)))
            .child(cell_text)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |settings, _, _, cx| {
                if settings.recording.as_deref() == Some(recorded_id.as_str()) {
                    settings.cancel_recording(cx);
                } else {
                    settings.start_recording(recorded_id.clone(), cx);
                }
            }));

        let reset_id = id.to_string();
        let reset_button = div().flex_none().when(is_overridden, |element| {
            element.child(
                div()
                    .id(SharedString::from(format!("reset-keybind-{id}")))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.))
                    .px_2()
                    .rounded_md()
                    .text_size(px(11.))
                    .text_color(rgb(colors.muted))
                    .cursor_pointer()
                    .hover(|element| element.bg(rgb(colors.panel_alt)))
                    .child(language.reset_to_default())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.reset_binding(reset_id.clone(), cx);
                    })),
            )
        });

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .min_h(px(48.))
            .px_4()
            .py_2()
            .child(div().flex_1().min_w_0().child(label))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(reset_button)
                    .child(shortcut_cell),
            )
            .into_any_element()
    }

    fn render_font_size_control(&self, font_size: f32, cx: &mut Context<Self>) -> AnyElement {
        let button = |id: &'static str,
                      label: &'static str,
                      enabled: bool,
                      delta: f32,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.change_font_size(delta, cx)
                        }))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(
                "decrease-font-size",
                "−",
                font_size > MIN_FONT_SIZE,
                -1.,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(54.))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(format!("{font_size:.0}")),
            )
            .child(button(
                "increase-font-size",
                "+",
                font_size < MAX_FONT_SIZE,
                1.,
                cx,
            ))
            .into_any_element()
    }

    fn render_minimum_contrast_control(
        &self,
        minimum_contrast: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let button = |id: &'static str,
                      label: &'static str,
                      enabled: bool,
                      delta: f32,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.change_minimum_contrast(delta, cx)
                        }))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(
                "decrease-minimum-contrast",
                "−",
                minimum_contrast > MIN_MINIMUM_CONTRAST,
                -0.1,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(54.))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(format!("{minimum_contrast:.1}")),
            )
            .child(button(
                "increase-minimum-contrast",
                "+",
                minimum_contrast < MAX_MINIMUM_CONTRAST,
                0.1,
                cx,
            ))
            .into_any_element()
    }

    fn render_terminal_padding_control(
        &self,
        axis: PaddingAxis,
        value: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (decrease_id, increase_id) = match axis {
            PaddingAxis::Horizontal => (
                "decrease-horizontal-terminal-padding",
                "increase-horizontal-terminal-padding",
            ),
            PaddingAxis::Vertical => (
                "decrease-vertical-terminal-padding",
                "increase-vertical-terminal-padding",
            ),
        };
        let button = |id: &'static str,
                      label: &'static str,
                      enabled: bool,
                      delta: f32,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.change_terminal_padding(axis, delta, cx)
                        }))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(
                decrease_id,
                "−",
                value > MIN_TERMINAL_PADDING,
                -1.,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(54.))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(format!("{value:.0}")),
            )
            .child(button(
                increase_id,
                "+",
                value < MAX_TERMINAL_PADDING,
                1.,
                cx,
            ))
            .into_any_element()
    }

    fn render_metric_adjustment_control(
        &self,
        kind: MetricAdjustmentKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Parse the current integer pixel delta (percent values, hand-edited in settings.json,
        // display as their raw string and step from 0).
        let raw = kind
            .value(&self.settings.read(cx).config().font_metrics)
            .clone();
        let numeric = raw
            .as_deref()
            .and_then(|text| text.trim().parse::<i32>().ok());
        let display = match (&raw, numeric) {
            (Some(text), None) => text.clone(), // percent or other non-integer form
            (_, Some(value)) if value > 0 => format!("+{value}"),
            (_, Some(value)) => format!("{value}"),
            (None, _) => "0".to_owned(),
        };
        let current = numeric.unwrap_or(0);
        let decrease_id = SharedString::from(format!("decrease-adjust-{}", kind.slug()));
        let increase_id = SharedString::from(format!("increase-adjust-{}", kind.slug()));
        let button = |id: SharedString,
                      label: &'static str,
                      enabled: bool,
                      delta: i32,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.change_metric_adjustment(kind, delta, cx)
                        }))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(decrease_id, "−", current > -64, -1, cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(54.))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(display),
            )
            .child(button(increase_id, "+", current < 64, 1, cx))
            .into_any_element()
    }

    fn render_progress_timeout_control(
        &self,
        kind: ProgressTimeoutKind,
        value: u32,
        step: i32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (decrease_id, increase_id) = match kind {
            ProgressTimeoutKind::Complete => (
                "decrease-progress-complete-timeout",
                "increase-progress-complete-timeout",
            ),
            ProgressTimeoutKind::Stale => (
                "decrease-progress-stale-timeout",
                "increase-progress-stale-timeout",
            ),
        };
        let button = |id: &'static str,
                      label: &'static str,
                      enabled: bool,
                      delta: i32,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.change_progress_timeout(kind, delta, cx)
                        }))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(
                decrease_id,
                "−",
                value > MIN_PROGRESS_TIMEOUT_SECS,
                -step,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(62.))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(format!("{value}s")),
            )
            .child(button(
                increase_id,
                "+",
                value < MAX_PROGRESS_TIMEOUT_SECS,
                step,
                cx,
            ))
            .into_any_element()
    }

    fn render_terminal_preview(&self, config: &AppSettings, theme: &TerminalTheme) -> AnyElement {
        let language = config.language;
        let foreground =
            minimum_contrast_rgb(theme.foreground, theme.background, config.minimum_contrast);
        let command =
            minimum_contrast_rgb(theme.palette[2], theme.background, config.minimum_contrast);
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .h(px(124.))
            .p_3()
            .border_t_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgb(self.colors.muted))
                    .child(language.terminal_preview_label()),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .flex_col()
                    .overflow_hidden()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(theme.background))
                    .px(px(config.terminal_padding_x))
                    .py(px(config.terminal_padding_y))
                    .font_family(SharedString::from(config.font_family.clone()))
                    .text_size(px(config.font_size))
                    .text_color(rgb(foreground))
                    .child("~/Eggie")
                    .child(div().text_color(rgb(command)).child("$ cargo run")),
            )
            .into_any_element()
    }
}

impl gpui::Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = self.settings.read(cx).config().clone();
        let theme = config.effective_theme(is_dark_appearance(window.appearance()));
        self.colors = UiColors::from_theme(theme);

        let language = config.language;
        let language_control = self.render_language_control(language, cx);
        let bell_control = self.render_bell_control(config.bell_mode, window, cx);
        let mode_control = self.render_mode_control(config.theme_mode, cx);
        let dark_selector = self.render_selector(
            "dark-theme-selector",
            config.dark_theme.clone(),
            None,
            SelectorKind::DarkTheme,
            window,
            cx,
        );
        let light_selector = self.render_selector(
            "light-theme-selector",
            config.light_theme.clone(),
            None,
            SelectorKind::LightTheme,
            window,
            cx,
        );
        let font_selector = self.render_selector(
            "font-selector",
            config.font_family.clone(),
            Some(config.font_family.clone()),
            SelectorKind::Font,
            window,
            cx,
        );
        // Per-style family selectors. An empty stored value means "fall back to the regular family";
        // show a localized hint and preview in the regular family in that case.
        let style_family_selector = |this: &Self,
                                      id: &'static str,
                                      stored: &str,
                                      kind: SelectorKind,
                                      window: &Window,
                                      cx: &mut Context<Self>| {
            let has_value = !stored.trim().is_empty();
            let display = if has_value {
                stored.to_owned()
            } else {
                language.font_family_use_regular().to_owned()
            };
            let preview_family = if has_value {
                stored.to_owned()
            } else {
                config.font_family.clone()
            };
            this.render_selector(id, display, Some(preview_family), kind, window, cx)
        };
        let font_bold_selector = style_family_selector(
            self,
            "font-bold-selector",
            &config.font_family_bold,
            SelectorKind::FontBold,
            window,
            cx,
        );
        let font_italic_selector = style_family_selector(
            self,
            "font-italic-selector",
            &config.font_family_italic,
            SelectorKind::FontItalic,
            window,
            cx,
        );
        let font_bold_italic_selector = style_family_selector(
            self,
            "font-bold-italic-selector",
            &config.font_family_bold_italic,
            SelectorKind::FontBoldItalic,
            window,
            cx,
        );
        let synthetic_bold_control = self.render_synthetic_style_control(
            SyntheticStyleKind::Bold,
            config.font_synthetic_style.bold,
            cx,
        );
        let synthetic_italic_control = self.render_synthetic_style_control(
            SyntheticStyleKind::Italic,
            config.font_synthetic_style.italic,
            cx,
        );
        let synthetic_bold_italic_control = self.render_synthetic_style_control(
            SyntheticStyleKind::BoldItalic,
            config.font_synthetic_style.bold_italic,
            cx,
        );
        let ligatures_control = self.render_ligatures_control(config.ligatures, cx);
        let shaping_break_control =
            self.render_shaping_break_control(config.font_shaping_break_cursor, cx);
        let font_thicken_control = self.render_font_thicken_control(config.font_thicken, cx);
        let font_thicken_strength_control =
            self.render_font_thicken_strength_control(config.font_thicken_strength, cx);
        let font_size_control = self.render_font_size_control(config.font_size, cx);
        let cursor_shape_control = self.render_cursor_shape_control(config.cursor_shape, cx);
        let cursor_blink_control = self.render_cursor_blink_control(config.cursor_blink, cx);
        let minimum_contrast_control =
            self.render_minimum_contrast_control(config.minimum_contrast, cx);
        let horizontal_padding_control = self.render_terminal_padding_control(
            PaddingAxis::Horizontal,
            config.terminal_padding_x,
            cx,
        );
        let vertical_padding_control = self.render_terminal_padding_control(
            PaddingAxis::Vertical,
            config.terminal_padding_y,
            cx,
        );
        // Build the nine font-metric adjustment rows up front (label + description + stepper).
        let metric_rows: Vec<AnyElement> = MetricAdjustmentKind::ALL
            .into_iter()
            .map(|kind| {
                let (label, description) = Self::metric_adjustment_labels(language, kind);
                settings_row(
                    label,
                    description,
                    self.render_metric_adjustment_control(kind, cx),
                    self.colors,
                )
            })
            .collect();
        let progress_complete_timeout_control = self.render_progress_timeout_control(
            ProgressTimeoutKind::Complete,
            config.progress_complete_timeout_secs,
            1,
            cx,
        );
        let progress_stale_timeout_control = self.render_progress_timeout_control(
            ProgressTimeoutKind::Stale,
            config.progress_stale_timeout_secs,
            5,
            cx,
        );
        let osc_clipboard_read_control =
            self.render_osc_clipboard_read_control(config.allow_osc_clipboard_read, cx);
        let detect_urls_control = self.render_detect_urls_control(config.detect_urls, cx);
        let copy_on_select_control =
            self.render_copy_on_select_control(config.copy_on_select, cx);
        let terminal_preview = self.render_terminal_preview(&config, theme);

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(self.colors.background))
            .text_color(rgb(self.colors.text))
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .child(
                div()
                    .id("settings-window-drag-region")
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(SETTINGS_TITLEBAR_HEIGHT))
                    .pl(px(SETTINGS_TITLE_LEFT_INSET))
                    .pr_5()
                    .border_b_1()
                    .border_color(rgb(self.colors.border))
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|settings, _, _, _| settings.moving_window = true),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|settings, _, _, _| settings.moving_window = false),
                    )
                    .on_mouse_move(cx.listener(|settings, _, window, _| {
                        if settings.moving_window {
                            settings.moving_window = false;
                            window.start_window_move();
                        }
                    }))
                    .on_click(|event, window, _| {
                        if event.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    })
                    .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child(language.settings_title())),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .flex_col()
                            .w(px(SETTINGS_SIDEBAR_WIDTH))
                            .h_full()
                            .p_2()
                            .gap_1()
                            .border_r_1()
                            .border_color(rgb(self.colors.border))
                            .bg(rgb(self.colors.panel))
                            .children(SettingsSection::ALL.map(|section| {
                                let (id, label, icon_name) = match section {
                                    SettingsSection::General => (
                                        "settings-sidebar-general",
                                        language.general_sidebar(),
                                        IconName::Settings,
                                    ),
                                    SettingsSection::Appearance => (
                                        "settings-sidebar-appearance",
                                        language.appearance_sidebar(),
                                        IconName::Appearance,
                                    ),
                                    SettingsSection::Keybindings => (
                                        "settings-sidebar-keybindings",
                                        language.keybindings_sidebar(),
                                        IconName::Keyboard,
                                    ),
                                    SettingsSection::Advanced => (
                                        "settings-sidebar-advanced",
                                        language.advanced_sidebar(),
                                        IconName::Info,
                                    ),
                                };
                                let selected = self.selected_section == section;
                                div()
                                    .id(id)
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .h(px(34.))
                                    .px_2()
                                    .rounded_lg()
                                    .cursor_pointer()
                                    .when(selected, |element| {
                                        element
                                            .bg(rgb(self.colors.panel_alt))
                                            .text_color(rgb(self.colors.accent))
                                    })
                                    .when(!selected, |element| {
                                        element.hover(|element| {
                                            element.bg(rgb(self.colors.panel_alt))
                                        })
                                    })
                                    .on_click(cx.listener(move |settings, _, _, cx| {
                                        if settings.recording.is_some() {
                                            settings.cancel_recording(cx);
                                        }
                                        settings.bell_dropdown_open = false;
                                        settings.selected_section = section;
                                        cx.notify();
                                    }))
                                    .child(icon(icon_name))
                                    .child(label)
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex_col()
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_h_0()
                                    .child(
                                        div()
                                            .id("settings-scroll-content")
                                            .size_full()
                                            .min_h_0()
                                            .overflow_x_hidden()
                                            .overflow_y_scroll()
                                            .scrollbar_width(px(SETTINGS_SCROLLBAR_WIDTH))
                                            .track_scroll(&self.settings_scroll_handle)
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_none()
                                                    .flex_col()
                                                    .mx_auto()
                                                    .w_full()
                                                    .max_w(px(640.))
                                                    .p_5()
                                                    .gap_5()
                                                    .child(
                                                        div()
                                                            .text_size(px(18.))
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .child(match self.selected_section {
                                                                SettingsSection::General => language.general_section(),
                                                                SettingsSection::Appearance => language.appearance_section(),
                                                                SettingsSection::Keybindings => language.keybindings_section(),
                                                                SettingsSection::Advanced => language.advanced_section(),
                                                            }),
                                                    )
                                                    .children(match self.selected_section {
                                                        SettingsSection::General => vec![
                                                            settings_section(
                                                                language.general_section(),
                                                                vec![settings_row(
                                                                    language.language_row(),
                                                                    language.language_description(),
                                                                    language_control,
                                                                    self.colors,
                                                                )],
                                                                self.colors,
                                                            ),
                                                        ],
                                                        SettingsSection::Appearance => vec![
                                                            settings_section(
                                                                language.theme_section(),
                                                                vec![
                                                                    settings_row(
                                                                        language.theme_row(),
                                                                        language.theme_description(),
                                                                        mode_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.dark_theme_row(),
                                                                        language.dark_theme_description(),
                                                                        dark_selector,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.light_theme_row(),
                                                                        language.light_theme_description(),
                                                                        light_selector,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.minimum_contrast_row(),
                                                                        language.minimum_contrast_description(),
                                                                        minimum_contrast_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                language.terminal_text_section(),
                                                                vec![
                                                                    settings_row(
                                                                        language.font_row(),
                                                                        language.font_description(),
                                                                        font_selector,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.font_bold_row(),
                                                                        language.font_bold_description(),
                                                                        font_bold_selector,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.font_italic_row(),
                                                                        language.font_italic_description(),
                                                                        font_italic_selector,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.font_bold_italic_row(),
                                                                        language.font_bold_italic_description(),
                                                                        font_bold_italic_selector,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.synthetic_bold_row(),
                                                                        language.synthetic_bold_description(),
                                                                        synthetic_bold_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.synthetic_italic_row(),
                                                                        language.synthetic_italic_description(),
                                                                        synthetic_italic_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.synthetic_bold_italic_row(),
                                                                        language.synthetic_bold_italic_description(),
                                                                        synthetic_bold_italic_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.ligatures_row(),
                                                                        language.ligatures_description(),
                                                                        ligatures_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.shaping_break_row(),
                                                                        language.shaping_break_description(),
                                                                        shaping_break_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.font_thicken_row(),
                                                                        language.font_thicken_description(),
                                                                        font_thicken_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.font_thicken_strength_row(),
                                                                        language.font_thicken_strength_description(),
                                                                        font_thicken_strength_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.font_size_row(),
                                                                        language.font_size_description(),
                                                                        font_size_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                language.cursor_section(),
                                                                vec![
                                                                    settings_row(
                                                                        language.cursor_shape_row(),
                                                                        language.cursor_shape_description(),
                                                                        cursor_shape_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.cursor_blink_row(),
                                                                        language.cursor_blink_description(),
                                                                        cursor_blink_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                language.terminal_layout_section(),
                                                                vec![
                                                                    settings_row(
                                                                        language.horizontal_padding_row(),
                                                                        language.horizontal_padding_description(),
                                                                        horizontal_padding_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.vertical_padding_row(),
                                                                        language.vertical_padding_description(),
                                                                        vertical_padding_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                language.font_metrics_section(),
                                                                metric_rows,
                                                                self.colors,
                                                            ),
                                                        ],
                                                        SettingsSection::Advanced => vec![
                                                            settings_section(
                                                                language.progress_indicators_section(),
                                                                vec![
                                                                    settings_row(
                                                                        language.completed_timeout_row(),
                                                                        language.completed_timeout_description(),
                                                                        progress_complete_timeout_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.inactive_timeout_row(),
                                                                        language.inactive_timeout_description(),
                                                                        progress_stale_timeout_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                language.terminal_behavior_section(),
                                                                vec![
                                                                    settings_row(
                                                                        language.detect_urls_row(),
                                                                        language.detect_urls_description(),
                                                                        detect_urls_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.copy_on_select_row(),
                                                                        language.copy_on_select_description(),
                                                                        copy_on_select_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.bell_row(),
                                                                        language.bell_description(),
                                                                        bell_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                language.terminal_security_section(),
                                                                vec![settings_row(
                                                                    language.osc_clipboard_read_row(),
                                                                    language.osc_clipboard_read_description(),
                                                                    osc_clipboard_read_control,
                                                                    self.colors,
                                                                )],
                                                                self.colors,
                                                            ),
                                                        ],
                                                        SettingsSection::Keybindings => {
                                                            self.render_keybindings_section(language, cx)
                                                        }
                                                    }),
                                            ),
                                    ),
                            )
                            .when(self.selected_section == SettingsSection::Appearance, |element| {
                                element.child(terminal_preview)
                            }),
                    ),
            )
    }
}

fn settings_section(title: &'static str, rows: Vec<AnyElement>, colors: UiColors) -> AnyElement {
    let row_count = rows.len();
    rows.into_iter()
        .enumerate()
        .fold(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .overflow_hidden()
                .rounded_xl()
                .border_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.panel))
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .border_b_1()
                        .border_color(rgb(colors.border))
                        .child(title),
                ),
            |section, (index, row)| {
                section.child(
                    div()
                        .when(index + 1 < row_count, |element| {
                            element.border_b_1().border_color(rgb(colors.border))
                        })
                        .child(row),
                )
            },
        )
        .into_any_element()
}

fn settings_row(
    label: &'static str,
    description: &'static str,
    control: AnyElement,
    colors: UiColors,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_6()
        .min_h(px(64.))
        .px_4()
        .py_3()
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .flex_col()
                .gap_1()
                .child(label)
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(colors.muted))
                        .child(description),
                ),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

pub(crate) fn is_dark_appearance(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

fn selector_matches(item: &str, query: &str) -> bool {
    query.is_empty() || item.to_lowercase().contains(&query.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::selector_matches;

    #[test]
    fn selector_search_is_case_insensitive_and_supports_partial_matches() {
        assert!(selector_matches("Maple Mono NF CN", "maple"));
        assert!(selector_matches("Builtin Dark", "IN DA"));
        assert!(!selector_matches("Menlo", "mono"));
        assert!(selector_matches("Menlo", ""));
    }
}

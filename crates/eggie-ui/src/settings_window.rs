use std::sync::OnceLock;

use crate::{
    icons::{IconName, icon},
    settings::{
        AppSettings, Language, MAX_FONT_SIZE, MAX_MINIMUM_CONTRAST, MAX_PROGRESS_TIMEOUT_SECS,
        MAX_TERMINAL_PADDING, MIN_FONT_SIZE, MIN_MINIMUM_CONTRAST, MIN_PROGRESS_TIMEOUT_SECS,
        MIN_TERMINAL_PADDING, SettingsStore, TerminalTheme, ThemeMode, UiColors,
        minimum_contrast_rgb, theme_catalog,
    },
};
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Entity, FocusHandle, KeyBinding, KeyDownEvent, Menu,
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
        TerminalSelectAll
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectorKind {
    DarkTheme,
    LightTheme,
    Font,
}

impl SelectorKind {
    const fn index(self) -> usize {
        match self {
            Self::DarkTheme => 0,
            Self::LightTheme => 1,
            Self::Font => 2,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SettingsSection {
    #[default]
    General,
    Appearance,
    Advanced,
}

impl SettingsSection {
    const ALL: [Self; 3] = [Self::General, Self::Appearance, Self::Advanced];
}

pub(crate) fn install(settings: Entity<SettingsStore>, cx: &mut App) {
    let settings_for_action = settings.clone();
    cx.on_action(move |_: &OpenSettings, cx| open_settings_window(settings_for_action.clone(), cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-c", TerminalCopy, None),
        KeyBinding::new("cmd-v", TerminalPaste, None),
        KeyBinding::new("cmd-a", TerminalSelectAll, None),
    ]);
    let language = settings.read(cx).config().language;
    cx.set_menus(build_menus(language));

    let settings_for_observer = settings.clone();
    cx.observe(&settings, move |_, cx| {
        let language = settings_for_observer.read(cx).config().language;
        cx.set_menus(build_menus(language));
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
    selector_search: String,
    selector_search_focus: FocusHandle,
    selector_scroll_handle: ScrollHandle,
    settings_scroll_handle: ScrollHandle,
    selector_bounds: [Option<Bounds<Pixels>>; 3],
    moving_window: bool,
    selected_section: SettingsSection,
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
        Self {
            settings,
            dark_theme_names: theme_catalog().dark_names(),
            light_theme_names: theme_catalog().light_names(),
            font_names,
            colors: UiColors::from_theme(theme),
            open_selector: None,
            selector_search: String::new(),
            selector_search_focus: cx.focus_handle(),
            selector_scroll_handle: ScrollHandle::new(),
            settings_scroll_handle: ScrollHandle::new(),
            selector_bounds: [None; 3],
            moving_window: false,
            selected_section: SettingsSection::default(),
        }
    }

    fn set_theme_mode(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.theme_mode = mode, cx)
        });
    }

    fn set_osc_clipboard_read(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.allow_osc_clipboard_read = enabled, cx)
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
                            settings.selector_search.clear();
                            settings
                                .selector_scroll_handle
                                .set_offset(point(px(0.), px(0.)));
                            settings.selector_search_focus.focus(window, cx);
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

    fn selector_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let handled = if key == "escape" {
            self.open_selector = None;
            true
        } else if key == "backspace" && !modifiers.platform && !modifiers.control {
            self.selector_search.pop();
            true
        } else if modifiers.platform && key.eq_ignore_ascii_case("v") {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.selector_search
                    .push_str(&text.replace(['\r', '\n'], " "));
            }
            true
        } else if !modifiers.platform && !modifiers.control {
            if let Some(text) = event.keystroke.key_char.as_deref() {
                self.selector_search.push_str(text);
                true
            } else {
                false
            }
        } else {
            false
        };

        if handled {
            self.selector_scroll_handle
                .set_offset(gpui::point(px(0.), px(0.)));
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn render_selector_dropdown(
        &self,
        kind: SelectorKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = self.settings.read(cx).config();
        let language = config.language;
        let (search_placeholder, current, items) = match kind {
            SelectorKind::DarkTheme => (
                language.search_dark_themes(),
                config.dark_theme.clone(),
                self.dark_theme_names.clone(),
            ),
            SelectorKind::LightTheme => (
                language.search_light_themes(),
                config.light_theme.clone(),
                self.light_theme_names.clone(),
            ),
            SelectorKind::Font => (
                language.search_fonts(),
                config.font_family.clone(),
                self.font_names.clone(),
            ),
        };
        let query = self.selector_search.trim();
        let items = items
            .into_iter()
            .filter(|item| selector_matches(item, query))
            .collect::<Vec<_>>();
        let has_items = !items.is_empty();
        let search_is_focused = self.selector_search_focus.is_focused(window);

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
                    .when(kind == SelectorKind::Font, |element| {
                        element.font_family(SharedString::from(item.clone()))
                    })
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

        let search_value = if self.selector_search.is_empty() {
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(self.colors.muted))
                .child(search_placeholder)
        } else {
            div()
                .min_w_0()
                .truncate()
                .child(self.selector_search.clone())
        };
        let search_content = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(1.))
            .child(search_value)
            .when(search_is_focused, |element| {
                element.child(
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(16.))
                        .bg(rgb(self.colors.accent)),
                )
            });
        let search = div()
            .id("settings-selector-search")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(36.))
            .mx_3()
            .my_2()
            .px_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(if search_is_focused {
                self.colors.accent
            } else {
                self.colors.border
            }))
            .bg(rgb(self.colors.panel_alt))
            .cursor_text()
            .track_focus(&self.selector_search_focus)
            .on_click(cx.listener(|settings, _, window, cx| {
                settings.selector_search_focus.focus(window, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::selector_search_key_down))
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(self.colors.muted))
                    .child(icon(IconName::Search)),
            )
            .child(search_content);

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
        let font_size_control = self.render_font_size_control(config.font_size, cx);
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
                                                                        language.font_size_row(),
                                                                        language.font_size_description(),
                                                                        font_size_control,
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

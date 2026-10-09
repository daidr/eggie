use std::sync::{Arc, OnceLock};

use crate::{
    app::{EggieApp, NewWindowInit, NotificationRoutes, tooltip_text},
    continuous_corners::paint_continuous_rect,
    icons::{IconName, icon, icon_sized},
    project_store::ProjectStore,
    settings::{
        AppSettings, BellMode, CursorBlink, CursorShapeSetting, DEFAULT_DARK_THEME,
        DEFAULT_LIGHT_THEME, Language, MAX_FONT_SIZE, MAX_MINIMUM_CONTRAST,
        MAX_PROGRESS_TIMEOUT_SECS, MAX_SCROLLBACK_LINES, MAX_TERMINAL_PADDING, MIN_FONT_SIZE,
        MIN_MINIMUM_CONTRAST, MIN_PROGRESS_TIMEOUT_SECS, MIN_SCROLLBACK_LINES,
        MIN_TERMINAL_PADDING, SCROLLBACK_LINES_STEP, SettingsStore, TerminalTheme,
        TerminalThemeSource, ThemeMode, UiColors, UpdateChannel, minimum_contrast_rgb,
        theme_catalog,
    },
    text_input::{TextInput, TextInputEvent, TextInputStyle},
};
use eggie_daemon::DaemonClient;
use eggie_update::UpdateState;
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Entity, Image, ImageFormat, Menu, MenuItem,
    MouseButton, OsAction, Pixels, ScrollHandle, SharedString, SystemMenuType, TitlebarOptions,
    Window, WindowAppearance, WindowBounds, WindowControlArea, WindowOptions, actions, anchored,
    canvas, deferred, div, img, linear_color_stop, linear_gradient, point, prelude::*, px, rgb,
    size,
};

#[cfg(not(target_os = "macos"))]
use gpui::font;

actions!(
    eggie,
    [
        OpenSettings,
        NewWindow,
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
        JumpNextPrompt,
        About,
        CheckForUpdates,
        ToggleSecureKeyboardEntry,
        CloseWindow,
        CloseAllWindows,
        FindNext,
        FindPrevious,
        SplitLeft,
        SplitUp,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen,
        HelpDocs,
        CommandPalette,
        ToggleLeftSidebar,
        ToggleRightSidebar,
        /// Open the profile picker, then create a terminal from the chosen profile (a one-off
        /// override of the project's default profile).
        NewTabWithProfile
    ]
);

/// Destination for the Help menu's documentation item.
// TODO: point at the real Eggie docs/repo once one exists (Cargo.toml has no `repository`).
const EGGIE_HELP_URL: &str = "https://github.com/daidr/eggie";

const SETTINGS_WIDTH: f32 = 920.;
const SETTINGS_HEIGHT: f32 = 740.;
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
const THEME_CARD_WIDTH: f32 = 166.;
const THEME_CARD_HEIGHT: f32 = 110.;
const THEME_CARD_RADIUS: f32 = 40.;
const THEME_CARD_BASE_INSET: f32 = 6.;
/// Width of the compact bell-mode dropdown (trigger and popover).
const BELL_DROPDOWN_WIDTH: f32 = 180.;
const BELL_DROPDOWN_ROW_HEIGHT: f32 = 30.;

/// Text color for a "this input is being ignored" warning. Theme-independent on purpose: it has to
/// read as a problem against every one of the ~200 bundled themes, and a palette-derived red would be
/// the theme's *decorative* red, which in some themes is barely distinguishable from the body text.
const WARNING_TEXT: u32 = 0xff6b6b;

/// The badge colors offered in the profile editor's palette.
///
/// A fixed palette rather than a full color picker: the badge is a 8px dot whose only job is telling
/// terminals apart at a glance, so what matters is that any two choices are actually distinguishable.
/// Hand-picked to stay legible against both light and dark terminal backgrounds.
const BADGE_COLORS: [u32; 10] = [
    0xe5484d, // red
    0xf76b15, // orange
    0xffb224, // amber
    0x46a758, // green
    0x12a594, // teal
    0x0091ff, // blue
    0x3e63dd, // indigo
    0x8e4ec6, // purple
    0xe93d82, // pink
    0x8d8d8d, // gray
];

/// App logo shown at the top of the "About" panel. Embedded so it ships with the
/// binary (the UI registers no image asset source; see `HUGEICONS_FONT`).
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// The decoded logo, built once and reused. GPUI already keys its frame cache on
/// `Image.id` (a hash of the bytes), so the texture would be reused regardless —
/// the `OnceLock` additionally avoids re-hashing and re-allocating the PNG bytes
/// on every repaint. Returns an owned `Arc`, so it never borrows `self`.
fn logo_image() -> Arc<Image> {
    static LOGO: OnceLock<Arc<Image>> = OnceLock::new();
    LOGO.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, LOGO_PNG.to_vec())))
        .clone()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectorKind {
    /// Interface theme for the dark appearance — the app's own chrome.
    InterfaceDark,
    /// Interface theme for the light appearance.
    InterfaceLight,
    /// Terminal theme for the dark appearance. Only reachable while the terminal uses its own pair
    /// rather than following the interface.
    TerminalDark,
    /// Terminal theme for the light appearance. See [`Self::TerminalDark`].
    TerminalLight,
    Font,
    FontBold,
    FontItalic,
    FontBoldItalic,
    /// The selected profile's terminal theme override for the dark appearance, clearable back to
    /// "follow global". A profile never overrides the interface theme: chrome is window-level, and one
    /// window can show tabs from several profiles at once.
    ProfileTerminalDark,
    /// The selected profile's terminal theme override for the light appearance.
    ProfileTerminalLight,
    /// The selected profile's font-family override, clearable back to "follow global".
    ProfileFont,
}

impl SelectorKind {
    /// Every variant, in `index()` order. A test pins that `index()` is a bijection onto
    /// `0..ALL.len()` and that `selector_bounds` is exactly this long — both are hand-maintained, and
    /// an out-of-range `index()` panics when the dropdown reads or writes its bounds slot.
    const ALL: [Self; 11] = [
        Self::InterfaceDark,
        Self::InterfaceLight,
        Self::TerminalDark,
        Self::TerminalLight,
        Self::Font,
        Self::FontBold,
        Self::FontItalic,
        Self::FontBoldItalic,
        Self::ProfileTerminalDark,
        Self::ProfileTerminalLight,
        Self::ProfileFont,
    ];

    const fn index(self) -> usize {
        match self {
            Self::InterfaceDark => 0,
            Self::InterfaceLight => 1,
            Self::TerminalDark => 2,
            Self::TerminalLight => 3,
            Self::Font => 4,
            Self::FontBold => 5,
            Self::FontItalic => 6,
            Self::FontBoldItalic => 7,
            Self::ProfileTerminalDark => 8,
            Self::ProfileTerminalLight => 9,
            Self::ProfileFont => 10,
        }
    }

    /// Whether this selector edits a profile override (writes to the profile store) rather than a
    /// global setting (writes to `AppSettings`).
    const fn is_profile(self) -> bool {
        matches!(
            self,
            Self::ProfileTerminalDark | Self::ProfileTerminalLight | Self::ProfileFont
        )
    }

    /// Whether the dropdown should render each option in the font it names.
    const fn is_font(self) -> bool {
        matches!(
            self,
            Self::Font
                | Self::FontBold
                | Self::FontItalic
                | Self::FontBoldItalic
                | Self::ProfileFont
        )
    }

    /// Whether this selector picks a theme (as opposed to a font). Gates the live hover preview.
    const fn is_theme(self) -> bool {
        !self.is_font()
    }

    /// Whether this selector picks from the dark half of the catalog. Only meaningful for theme
    /// selectors; each half offers only themes that measure that way, so a dark slot can never hold a
    /// light theme.
    const fn is_dark(self) -> bool {
        matches!(
            self,
            Self::InterfaceDark | Self::TerminalDark | Self::ProfileTerminalDark
        )
    }

    /// The preview slot this selector drives, so a hover resolves through the same layering the
    /// committed value would. Profile slots carry the profile being edited.
    fn preview_slot(
        self,
        profile_id: eggie_domain::ProfileId,
    ) -> Option<crate::settings::PreviewSlot> {
        use crate::settings::PreviewSlot;
        match self {
            Self::InterfaceDark => Some(PreviewSlot::InterfaceDark),
            Self::InterfaceLight => Some(PreviewSlot::InterfaceLight),
            Self::TerminalDark => Some(PreviewSlot::TerminalDark),
            Self::TerminalLight => Some(PreviewSlot::TerminalLight),
            Self::ProfileTerminalDark => Some(PreviewSlot::ProfileTerminalDark(profile_id)),
            Self::ProfileTerminalLight => Some(PreviewSlot::ProfileTerminalLight(profile_id)),
            Self::Font
            | Self::FontBold
            | Self::FontItalic
            | Self::FontBoldItalic
            | Self::ProfileFont => None,
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
    Profiles,
    Keybindings,
    Advanced,
    About,
}

impl SettingsSection {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Appearance,
        Self::Profiles,
        Self::Keybindings,
        Self::Advanced,
        Self::About,
    ];
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn install(
    settings: Entity<SettingsStore>,
    project_store: Entity<ProjectStore>,
    profile_store: Entity<crate::profile_store::ProfileStore>,
    snippet_store: Entity<crate::snippet_store::SnippetStore>,
    notes_store: Entity<crate::notes_store::NotesStore>,
    client: DaemonClient,
    notification_routes: NotificationRoutes,
    updates: Entity<crate::update_controller::UpdateController>,
    cx: &mut App,
) {
    let settings_for_action = settings.clone();
    let updates_for_action = updates.clone();
    let profile_store_for_action = profile_store.clone();
    let project_store_for_action = project_store.clone();
    cx.on_action(move |_: &OpenSettings, cx| {
        open_settings_window(
            settings_for_action.clone(),
            updates_for_action.clone(),
            profile_store_for_action.clone(),
            project_store_for_action.clone(),
            cx,
        )
    });
    // New Window is an app-level action (like OpenSettings): it works even when no Eggie window is
    // focused, and opens a fresh main window sharing the same settings/project-store entities.
    let settings_for_new_window = settings.clone();
    let updates_for_new_window = updates.clone();
    let project_store_for_new_window = project_store.clone();
    let profile_store_for_new_window = profile_store.clone();
    let snippet_store_for_new_window = snippet_store.clone();
    let notes_store_for_new_window = notes_store.clone();
    let client_for_new_window = client.clone();
    let routes_for_new_window = notification_routes.clone();
    cx.on_action(move |_: &NewWindow, cx| {
        EggieApp::open_new_window(
            cx,
            client_for_new_window.clone(),
            settings_for_new_window.clone(),
            updates_for_new_window.clone(),
            project_store_for_new_window.clone(),
            profile_store_for_new_window.clone(),
            snippet_store_for_new_window.clone(),
            notes_store_for_new_window.clone(),
            routes_for_new_window.clone(),
            NewWindowInit::Empty,
        );
    });
    // Manual update check: surface the update window (which shows the live state) and kick off
    // a check when idle.
    let updates_for_check = updates.clone();
    let settings_for_check = settings.clone();
    cx.on_action(move |_: &CheckForUpdates, cx| {
        let channel = settings_for_check.read(cx).config().update_channel.slug();
        updates_for_check.update(cx, |controller, cx| {
            // Re-check from any settled state (idle / up-to-date / error);
            // leave in-flight or pending work (checking / available /
            // downloading / ready) untouched — the window shows its progress.
            if matches!(
                controller.state(),
                UpdateState::Idle | UpdateState::UpToDate | UpdateState::Error(_)
            ) {
                controller.check(false, channel, cx);
            }
        });
        // Always surface the window so the user sees the current state,
        // whether that's a fresh check, a pending download, or an error.
        crate::update_window::open_update_window(
            updates_for_check.clone(),
            settings_for_check.clone(),
            cx,
        );
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    // App-level menu commands that don't act on a specific terminal window.
    cx.on_action(|_: &About, _cx| crate::system_menu::show_about_panel());
    cx.on_action(|_: &CloseAllWindows, cx| {
        for handle in cx.windows() {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        }
    });
    cx.on_action(|_: &HelpDocs, cx| cx.open_url(EGGIE_HELP_URL));
    // Toggling Secure Keyboard Entry flips a process-global input mode, then rebuilds the menu
    // bar so the checkmark reflects the new state (menus have no in-place item update API).
    let settings_for_secure_input = settings.clone();
    cx.on_action(move |_: &ToggleSecureKeyboardEntry, cx| {
        crate::system_menu::toggle_secure_input();
        let config = settings_for_secure_input.read(cx).config().clone();
        let lua = settings_for_secure_input
            .read(cx)
            .lua_config()
            .registered
            .clone();
        crate::keybindings::rebuild_keymap(&config, &lua, cx);
        cx.set_menus(build_menus(config.language));
    });
    // Build the keymap from settings (defaults + user overrides). This also
    // registers the text-input bindings in the correct order to preserve the
    // binding-index tiebreak. Action handlers above are registered exactly once
    // and are intentionally *not* rebuilt when the keymap changes.
    let config = settings.read(cx).config().clone();
    let lua = settings.read(cx).lua_config().registered.clone();
    crate::keybindings::rebuild_keymap(&config, &lua, cx);
    let language = config.language;
    cx.set_menus(build_menus(language));

    let settings_for_observer = settings.clone();
    cx.observe(&settings, move |_, cx| {
        let config = settings_for_observer.read(cx).config().clone();
        // Rebuild the keymap first: `set_menus` snapshots the current keymap to resolve each
        // menu item's shortcut, so it must run *after* the keymap reflects this change or the
        // menu bar lags one edit behind.
        let lua = settings_for_observer
            .read(cx)
            .lua_config()
            .registered
            .clone();
        crate::keybindings::rebuild_keymap(&config, &lua, cx);
        cx.set_menus(build_menus(config.language));
    })
    .detach();
}

fn build_menus(language: Language) -> Vec<Menu> {
    // SF Symbol names mirror ghostty's `setupMenuImages` where it has an equivalent item;
    // the rest are chosen to match. Unknown symbols degrade to no icon (see gpui setImage).
    vec![
        Menu::new("Eggie").items([
            MenuItem::action(language.about_eggie(), About).icon("info.circle"),
            MenuItem::separator(),
            MenuItem::action(language.settings_menu_item(), OpenSettings).icon("gear"),
            MenuItem::action(language.secure_keyboard_entry(), ToggleSecureKeyboardEntry)
                .icon("lock.display")
                .checked(crate::system_menu::secure_input_enabled()),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(language.hide_eggie(), Hide),
            MenuItem::action(language.hide_others(), HideOthers),
            MenuItem::action(language.show_all(), ShowAll),
            MenuItem::separator(),
            MenuItem::action(language.quit_eggie(), Quit),
        ]),
        Menu::new(language.file_menu()).items([
            MenuItem::action(language.action_new_tab(), NewTab).icon("macwindow"),
            MenuItem::action(language.new_window_menu_item(), NewWindow)
                .icon("macwindow.badge.plus"),
            MenuItem::separator(),
            MenuItem::action(language.action_split_right(), SplitRight)
                .icon("rectangle.righthalf.inset.filled"),
            MenuItem::action(language.action_split_left(), SplitLeft)
                .icon("rectangle.leadinghalf.inset.filled"),
            MenuItem::action(language.action_split_down(), SplitDown)
                .icon("rectangle.bottomhalf.inset.filled"),
            MenuItem::action(language.action_split_up(), SplitUp)
                .icon("rectangle.tophalf.inset.filled"),
            MenuItem::separator(),
            MenuItem::action(language.action_close_tab(), CloseTab).icon("xmark"),
            MenuItem::action(language.action_close_window(), CloseWindow).icon("xmark.rectangle"),
            MenuItem::action(language.close_all_windows_menu_item(), CloseAllWindows)
                .icon("xmark.rectangle.fill"),
        ]),
        Menu::new(language.edit_menu()).items([
            MenuItem::os_action(language.copy(), TerminalCopy, OsAction::Copy).icon("doc.on.doc"),
            MenuItem::os_action(language.paste(), TerminalPaste, OsAction::Paste)
                .icon("doc.on.clipboard"),
            MenuItem::separator(),
            MenuItem::os_action(
                language.select_all(),
                TerminalSelectAll,
                OsAction::SelectAll,
            )
            .icon("text.justify"),
            MenuItem::separator(),
            MenuItem::action(language.find_menu_item(), TerminalFind).icon("magnifyingglass"),
            MenuItem::action(language.action_find_next(), FindNext).icon("chevron.down"),
            MenuItem::action(language.action_find_previous(), FindPrevious).icon("chevron.up"),
        ]),
        Menu::new(language.view_menu()).items([
            MenuItem::action(language.command_palette_menu_item(), CommandPalette).icon("command"),
            MenuItem::separator(),
            MenuItem::action(language.action_toggle_left_sidebar(), ToggleLeftSidebar)
                .icon("sidebar.left"),
            MenuItem::action(language.action_toggle_right_sidebar(), ToggleRightSidebar)
                .icon("sidebar.right"),
            MenuItem::separator(),
            MenuItem::action(language.action_font_increase(), FontIncrease)
                .icon("textformat.size.larger"),
            MenuItem::action(language.action_font_decrease(), FontDecrease)
                .icon("textformat.size.smaller"),
            MenuItem::action(language.action_font_reset(), FontReset).icon("textformat.size"),
            MenuItem::separator(),
            MenuItem::action(language.action_scroll_top(), ScrollTop).icon("arrow.up.to.line"),
            MenuItem::action(language.action_scroll_bottom(), ScrollBottom)
                .icon("arrow.down.to.line"),
            MenuItem::action(language.action_page_up(), PageUp).icon("arrow.up"),
            MenuItem::action(language.action_page_down(), PageDown).icon("arrow.down"),
            MenuItem::separator(),
            MenuItem::action(language.action_jump_prev_prompt(), JumpPrevPrompt)
                .icon("arrowtriangle.up.circle"),
            MenuItem::action(language.action_jump_next_prompt(), JumpNextPrompt)
                .icon("arrowtriangle.down.circle"),
            MenuItem::separator(),
            MenuItem::action(language.action_clear_screen(), ClearScreen).icon("clear"),
        ]),
        Menu::new(language.window_menu()).items([
            MenuItem::action(language.action_minimize_window(), MinimizeWindow)
                .icon("minus.rectangle"),
            MenuItem::action(language.zoom_menu_item(), ZoomWindow)
                .icon("arrow.up.left.and.arrow.down.right"),
            MenuItem::separator(),
            MenuItem::action(language.action_toggle_full_screen(), ToggleFullScreen)
                .icon("square.arrowtriangle.4.outward"),
            MenuItem::separator(),
            MenuItem::action(language.action_next_tab(), NextTab).icon("chevron.right"),
            MenuItem::action(language.action_prev_tab(), PrevTab).icon("chevron.left"),
        ]),
        Menu::new(language.help_menu()).help().items([
            MenuItem::action(language.help_docs_menu_item(), HelpDocs).icon("questionmark.circle"),
            MenuItem::separator(),
            MenuItem::action(language.check_for_updates_menu_item(), CheckForUpdates)
                .icon("arrow.down.circle"),
        ]),
    ]
}

fn open_settings_window(
    settings: Entity<SettingsStore>,
    updates: Entity<crate::update_controller::UpdateController>,
    profile_store: Entity<crate::profile_store::ProfileStore>,
    project_store: Entity<ProjectStore>,
    cx: &mut App,
) {
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
        move |window, cx| {
            cx.new(|cx| {
                SettingsWindow::new(
                    settings.clone(),
                    updates.clone(),
                    profile_store.clone(),
                    project_store.clone(),
                    fonts,
                    window,
                    cx,
                )
            })
        },
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

fn settings_input_style(colors: UiColors) -> TextInputStyle {
    TextInputStyle {
        text_color: (colors.text << 8) | 0xff,
        placeholder_color: (colors.muted << 8) | 0xff,
        cursor_color: (colors.accent << 8) | 0xff,
        selection_color: (colors.accent << 8) | 0x55,
    }
}

struct SettingsWindow {
    settings: Entity<SettingsStore>,
    updates: Entity<crate::update_controller::UpdateController>,
    /// Shared profile store. The Shell controls edit the built-in Default profile (which owns the
    /// launch fields since profiles landed), writing through to `settings` as well so a downgrade to a
    /// build that predates profiles still sees the user's choice.
    profile_store: Entity<crate::profile_store::ProfileStore>,
    /// Shared project store. Needed only so deleting a profile can clear it from any project that
    /// named it as their default, keeping a dangling id off disk.
    project_store: Entity<ProjectStore>,
    /// The catalog split by measured darkness. Every theme slot — interface, terminal, and per-profile
    /// override — offers exactly one of these two, because each slot is validated against that half.
    dark_theme_names: Vec<String>,
    light_theme_names: Vec<String>,
    font_names: Vec<String>,
    colors: UiColors,
    colors_for_theme: &'static TerminalTheme,
    open_selector: Option<SelectorKind>,
    selector_search_input: Entity<TextInput>,
    /// Persistent inputs for the shell override (Advanced → Shell). Seeded once from config; edits
    /// write straight back to settings. Spawn-only, so no live push to running terminals.
    shell_program_input: Entity<TextInput>,
    shell_args_input: Entity<TextInput>,
    selector_scroll_handle: ScrollHandle,
    settings_scroll_handle: ScrollHandle,
    theme_gallery_scroll_handles: [ScrollHandle; SelectorKind::ALL.len()],
    selector_bounds: [Option<Bounds<Pixels>>; SelectorKind::ALL.len()],
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
    /// The profile the Profiles pane is editing. `None` until the pane is first opened, then always a
    /// live id (a deleted profile hands selection to the built-in Default).
    selected_profile: Option<eggie_domain::ProfileId>,
    /// Per-field inputs for the selected profile. Re-seeded when the selection changes; every edit
    /// writes straight through to the profile store.
    profile_inputs: ProfileInputs,
    /// The profile whose values `profile_inputs` currently hold, so a re-render can tell "the user is
    /// typing" from "the selection moved and the inputs are stale".
    profile_inputs_for: Option<eggie_domain::ProfileId>,
    /// The exact text last seeded into the inputs, in `profile_input_list` order.
    ///
    /// The write-through compares against *this*, not against the stored profile, and that distinction
    /// is load-bearing. `set_content` emits `Changed` whose delivery GPUI defers past the render that
    /// seeded it, so the handler always runs once per selection with text the user never touched. And
    /// the text↔struct round trip is lossy for anything the parser refuses (an invalid env name, an
    /// out-of-range port), so "does the parsed result differ from what is stored?" answers *yes* in
    /// exactly the case that must not be written — silently deleting a hand-edited bad line from
    /// `profiles.json` instead of showing the warning this pane renders. Comparing raw text is
    /// immune: seeded text is byte-identical to itself, so only a real keystroke writes.
    seeded_profile_text: Option<[String; 8]>,
    /// Whether the badge-color picker popover is open, and where to anchor it.
    badge_picker_open: bool,
    badge_picker_bounds: Option<Bounds<Pixels>>,
}

/// The eight text inputs backing the selected profile's fields.
///
/// Grouped in their own struct so seeding them is one call that cannot forget a field — the failure
/// mode of a flat list of eight `Entity<TextInput>` members is a newly added field that is never
/// re-seeded, which shows the *previous* profile's value while writing to the current one.
struct ProfileInputs {
    name: Entity<TextInput>,
    shell_program: Entity<TextInput>,
    shell_args: Entity<TextInput>,
    cwd: Entity<TextInput>,
    env: Entity<TextInput>,
    remote_host: Entity<TextInput>,
    remote_user: Entity<TextInput>,
    remote_port: Entity<TextInput>,
}

impl SettingsWindow {
    fn new(
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        profile_store: Entity<crate::profile_store::ProfileStore>,
        project_store: Entity<ProjectStore>,
        font_names: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        // No explicit `cx.observe(&profile_store)`: `render` reads that entity, so GPUI records it in
        // the window's `tracked_entities` and any `cx.notify()` on it invalidates this view
        // (`App::notify` → `WindowInvalidator::invalidate_view`). Every ProfileStore mutation ends in
        // `persist`, which notifies — so edits from this pane and from another window both repaint.
        // Verified live by mutating the store from outside every settings-window listener.
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let system_is_dark = is_dark_appearance(window.appearance());
        // Chrome only, so this reads the *interface* layer. Still `config` rather than the resolved
        // preview: no hover can be active while the window is being constructed.
        let theme = settings.read(cx).config().interface_theme(system_is_dark);
        let colors = UiColors::from_theme(theme);
        let input_style = settings_input_style(colors);
        let selector_search_input = cx.new(|cx| TextInput::new(window, cx, input_style));
        cx.subscribe_in(
            &selector_search_input,
            window,
            Self::on_selector_search_event,
        )
        .detach();
        let shell_program_input = cx.new(|cx| TextInput::new(window, cx, input_style));
        let shell_args_input = cx.new(|cx| TextInput::new(window, cx, input_style));
        // The Profiles pane's inputs. Created here (not lazily on first open) so the subscriptions are
        // installed exactly once; they stay empty until `sync_profile_inputs` seeds them.
        let profile_inputs = ProfileInputs {
            name: cx.new(|cx| TextInput::new(window, cx, input_style)),
            shell_program: cx.new(|cx| TextInput::new(window, cx, input_style)),
            shell_args: cx.new(|cx| TextInput::new(window, cx, input_style)),
            cwd: cx.new(|cx| TextInput::new(window, cx, input_style)),
            env: cx.new(|cx| TextInput::new(window, cx, input_style).multiline()),
            remote_host: cx.new(|cx| TextInput::new(window, cx, input_style)),
            remote_user: cx.new(|cx| TextInput::new(window, cx, input_style)),
            remote_port: cx.new(|cx| TextInput::new(window, cx, input_style)),
        };
        for input in [
            &profile_inputs.name,
            &profile_inputs.shell_program,
            &profile_inputs.shell_args,
            &profile_inputs.cwd,
            &profile_inputs.env,
            &profile_inputs.remote_host,
            &profile_inputs.remote_user,
            &profile_inputs.remote_port,
        ] {
            // Not `profile_input_list()`: `self` does not exist yet. The compiler still catches a
            // forgotten field here, because `ProfileInputs` has no `Default` — a new field must be
            // named in the initializer above.
            cx.subscribe_in(input, window, Self::on_profile_input_event)
                .detach();
        }
        // Seed the shell inputs exactly once, from the built-in Default profile — it owns the launch
        // fields, so `AppSettings`' copies may be a stale downgrade mirror. `set_content` emits
        // Changed, but that fires before the subscriptions below are installed, so it won't loop back.
        {
            let language = settings.read(cx).config().language;
            let default_profile = profile_store.read(cx).default_profile();
            let program = default_profile.shell_program.clone();
            let args = default_profile.shell_args.join(" ");
            shell_program_input.update(cx, |input, cx| {
                input.set_placeholder(language.shell_program_placeholder());
                input.set_content(program, cx);
            });
            shell_args_input.update(cx, |input, cx| {
                input.set_placeholder(language.shell_args_placeholder());
                input.set_content(args, cx);
            });
        }
        cx.subscribe_in(&shell_program_input, window, Self::on_shell_program_event)
            .detach();
        cx.subscribe_in(&shell_args_input, window, Self::on_shell_args_event)
            .detach();
        // Safety net: if the window closes while a hover preview is still active (e.g. closed via
        // keyboard while the pointer rested on an option), drop it so the main window's terminals
        // don't stay stuck on the previewed theme.
        cx.on_release(|this, cx| {
            this.settings
                .update(cx, |store, cx| store.clear_theme_preview(cx));
        })
        .detach();
        Self {
            settings,
            updates,
            profile_store,
            project_store,
            dark_theme_names: theme_catalog().dark_names(),
            light_theme_names: theme_catalog().light_names(),
            font_names,
            colors,
            colors_for_theme: theme,
            open_selector: None,
            selector_search_input,
            shell_program_input,
            shell_args_input,
            selector_scroll_handle: ScrollHandle::new(),
            settings_scroll_handle: ScrollHandle::new(),
            theme_gallery_scroll_handles: std::array::from_fn(|_| ScrollHandle::new()),
            selector_bounds: [None; SelectorKind::ALL.len()],
            moving_window: false,
            selected_section: SettingsSection::default(),
            recording: None,
            recording_subscription: None,
            recording_conflict: None,
            bell_dropdown_open: false,
            bell_dropdown_bounds: None,
            selected_profile: None,
            profile_inputs,
            profile_inputs_for: None,
            seeded_profile_text: None,
            badge_picker_open: false,
            badge_picker_bounds: None,
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
                self.clear_theme_preview(cx);
                cx.notify();
            }
        }
    }

    /// Apply an edit to the built-in Default profile, which owns the launch fields since profiles
    /// landed. The Shell controls in this window edit that profile — without this write-through they
    /// would persist to `settings.json` and never affect a terminal.
    fn update_default_profile(
        &mut self,
        update: impl FnOnce(&mut eggie_domain::Profile),
        cx: &mut Context<Self>,
    ) {
        self.profile_store.update(cx, |store, cx| {
            store.update(eggie_domain::DEFAULT_PROFILE_ID, update, cx);
        });
    }

    fn on_shell_program_event(
        &mut self,
        input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Persist on any content change (and on Enter). The input is the edit surface; normalize()
        // trims whitespace when the settings are saved.
        if matches!(
            event,
            TextInputEvent::Changed | TextInputEvent::Confirm | TextInputEvent::ConfirmReverse
        ) {
            let program = input.read(cx).content().to_owned();
            // Mirror into AppSettings as well: the field is vestigial for spawning, but it is what a
            // build predating profiles reads, and what the one-time migration would pick up.
            self.settings.update(cx, |settings, cx| {
                settings.update(|settings| settings.shell_program = program.clone(), cx);
            });
            self.update_default_profile(|profile| profile.shell_program = program, cx);
        }
    }

    fn on_shell_args_event(
        &mut self,
        input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            event,
            TextInputEvent::Changed | TextInputEvent::Confirm | TextInputEvent::ConfirmReverse
        ) {
            // Tokenize the single edit line into an argv on whitespace. Quoting is not handled (v1):
            // arguments containing spaces must be split into separate whitespace-free tokens.
            let args = crate::profile_editor::parse_args(input.read(cx).content());
            self.settings.update(cx, |settings, cx| {
                settings.update(|settings| settings.shell_args = args.clone(), cx);
            });
            self.update_default_profile(|profile| profile.shell_args = args, cx);
        }
    }

    /// The option list a selector offers. The single place that decides it, so adding a slot cannot
    /// leave one of the several read sites (Enter-to-commit, the dropdown list) offering the wrong half
    /// of the catalog.
    ///
    /// Each theme slot offers only its own half: the dark slots hold only themes that measure dark, so
    /// the light list would be invalid there and `AppSettings::normalize` would reject it anyway.
    fn selector_names(&self, kind: SelectorKind) -> &[String] {
        match kind {
            SelectorKind::InterfaceDark
            | SelectorKind::TerminalDark
            | SelectorKind::ProfileTerminalDark => &self.dark_theme_names,
            SelectorKind::InterfaceLight
            | SelectorKind::TerminalLight
            | SelectorKind::ProfileTerminalLight => &self.light_theme_names,
            SelectorKind::Font
            | SelectorKind::FontBold
            | SelectorKind::FontItalic
            | SelectorKind::FontBoldItalic
            | SelectorKind::ProfileFont => &self.font_names,
        }
    }

    fn lua_manages_selector(&self, kind: SelectorKind, cx: &Context<Self>) -> bool {
        let settings = self.settings.read(cx);
        let lua = &settings.lua_config().registered;
        match kind {
            SelectorKind::InterfaceDark => lua.ui_dark_theme.is_some(),
            SelectorKind::InterfaceLight => lua.ui_light_theme.is_some(),
            SelectorKind::TerminalDark => lua.terminal_dark_theme.is_some(),
            SelectorKind::TerminalLight => lua.terminal_light_theme.is_some(),
            SelectorKind::Font => lua.terminal_font.is_some(),
            _ => false,
        }
    }

    /// Write `choice` to whatever `kind` edits — a profile override or a global setting. The single
    /// commit path, shared by clicking an option and by Enter-to-commit-first-match, so a new slot
    /// cannot land in one and be forgotten in the other.
    fn commit_selector_choice(
        &mut self,
        kind: SelectorKind,
        choice: String,
        cx: &mut Context<Self>,
    ) {
        if self.lua_manages_selector(kind, cx) {
            return;
        }
        if kind.is_profile() {
            self.apply_profile_selector_choice(kind, Some(choice), cx);
            return;
        }
        self.settings.update(cx, |store, cx| {
            store.update(
                |config| match kind {
                    SelectorKind::InterfaceDark => config.interface_dark_theme = choice.clone(),
                    SelectorKind::InterfaceLight => config.interface_light_theme = choice.clone(),
                    SelectorKind::TerminalDark => config.terminal_dark_theme = choice.clone(),
                    SelectorKind::TerminalLight => config.terminal_light_theme = choice.clone(),
                    SelectorKind::Font => config.font_family = choice.clone(),
                    SelectorKind::FontBold => config.font_family_bold = choice.clone(),
                    SelectorKind::FontItalic => config.font_family_italic = choice.clone(),
                    SelectorKind::FontBoldItalic => config.font_family_bold_italic = choice.clone(),
                    // Handled above: these write to the profile store, not to AppSettings.
                    SelectorKind::ProfileTerminalDark
                    | SelectorKind::ProfileTerminalLight
                    | SelectorKind::ProfileFont => {}
                },
                cx,
            )
        });
    }

    /// Apply the first item matching the current filter for the open selector, then close it.
    fn apply_first_selector_match(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.open_selector else {
            return;
        };
        let query = self
            .selector_search_input
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let Some(choice) = self
            .selector_names(kind)
            .iter()
            .find(|item| selector_matches(item, &query))
            .cloned()
        else {
            return;
        };
        self.commit_selector_choice(kind, choice, cx);
        self.clear_theme_preview(cx);
        self.open_selector = None;
        cx.notify();
    }

    // --- Profiles pane -----------------------------------------------------------------------

    /// Every profile input, in field order. The single place that enumerates them, so language sync,
    /// subscription installation, and seeding cannot fall out of step with the struct.
    fn profile_input_list(&self) -> [&Entity<TextInput>; 8] {
        [
            &self.profile_inputs.name,
            &self.profile_inputs.shell_program,
            &self.profile_inputs.shell_args,
            &self.profile_inputs.cwd,
            &self.profile_inputs.env,
            &self.profile_inputs.remote_host,
            &self.profile_inputs.remote_user,
            &self.profile_inputs.remote_port,
        ]
    }

    /// Write an appearance override on the selected profile. `None` clears it back to "follow the
    /// global setting".
    fn apply_profile_selector_choice(
        &mut self,
        kind: SelectorKind,
        choice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_profile else {
            return;
        };
        self.profile_store.update(cx, |store, cx| {
            store.update(
                id,
                // Exhaustive on purpose: a catch-all arm would let a newly added profile selector
                // compile and then silently write nothing.
                |profile| match kind {
                    SelectorKind::ProfileTerminalDark => {
                        profile.appearance.terminal_dark_theme = choice
                    }
                    SelectorKind::ProfileTerminalLight => {
                        profile.appearance.terminal_light_theme = choice
                    }
                    SelectorKind::ProfileFont => profile.appearance.font_family = choice,
                    // Global selectors write to AppSettings; they never reach here.
                    SelectorKind::InterfaceDark
                    | SelectorKind::InterfaceLight
                    | SelectorKind::TerminalDark
                    | SelectorKind::TerminalLight
                    | SelectorKind::Font
                    | SelectorKind::FontBold
                    | SelectorKind::FontItalic
                    | SelectorKind::FontBoldItalic => {}
                },
                cx,
            );
        });
    }

    /// The profile the Profiles pane is editing, resolving a stale selection to the built-in Default.
    ///
    /// Never returns `None` once the store exists: the store's own invariant guarantees a built-in
    /// Default, and every stale selection (deleted in this window, or in another one) lands there
    /// rather than leaving the pane blank.
    fn selected_profile_id(&self, cx: &App) -> eggie_domain::ProfileId {
        self.profile_store
            .read(cx)
            .resolve(self.selected_profile)
            .id
    }

    /// Re-seed the profile inputs from the store when the selection has moved.
    ///
    /// Called from `render`, which is what makes it correct across *both* ways the selection can
    /// change: a click in this window, and a profile deleted elsewhere. The second works because
    /// `render` reads the profile store, so GPUI tracks that entity for this window and every
    /// `ProfileStore` mutation (each ends in `persist`, which notifies) invalidates this view — the
    /// now-stale id then resolves to the built-in Default here.
    ///
    /// Guarded on `profile_inputs_for`, deliberately *not* on a "currently seeding" flag: GPUI's
    /// `cx.emit` queues the event in `pending_effects` and delivers it after this call returns
    /// (`vendor/zed-gpui/.../app/context.rs`), so any flag cleared at the end of this function would
    /// already be false when `Changed` arrives. That deferral is also why the write-through compares
    /// against `seeded_profile_text` rather than trying to suppress the echo here.
    fn sync_profile_inputs(&mut self, cx: &mut Context<Self>) {
        let id = self.selected_profile_id(cx);
        if self.profile_inputs_for == Some(id) {
            return;
        }
        let Some(profile) = self.profile_store.read(cx).profile(id).cloned() else {
            return;
        };
        let language = self.settings.read(cx).config().language;
        let fields = crate::profile_editor::ProfileFields::from_profile(&profile);
        // Record the owner *before* seeding. `set_content` emits `Changed`, and although delivery is
        // deferred, it must find the inputs already attributed to this profile — otherwise the
        // write-through would attribute the seeded text to whichever profile was selected before.
        self.profile_inputs_for = Some(id);
        self.selected_profile = Some(id);
        let seeds: [(&Entity<TextInput>, String, &'static str); 8] = [
            (&self.profile_inputs.name, fields.name, ""),
            (
                &self.profile_inputs.shell_program,
                fields.shell_program,
                language.shell_program_placeholder(),
            ),
            (
                &self.profile_inputs.shell_args,
                fields.shell_args,
                language.shell_args_placeholder(),
            ),
            (
                &self.profile_inputs.cwd,
                fields.cwd,
                language.profile_cwd_placeholder(),
            ),
            (
                &self.profile_inputs.env,
                fields.env,
                language.profile_env_placeholder(),
            ),
            (
                &self.profile_inputs.remote_host,
                fields.remote_host,
                language.profile_remote_host_placeholder(),
            ),
            (&self.profile_inputs.remote_user, fields.remote_user, ""),
            (&self.profile_inputs.remote_port, fields.remote_port, ""),
        ];
        for (input, value, placeholder) in seeds {
            input.update(cx, |input, cx| {
                input.set_placeholder(placeholder);
                input.set_content(value, cx);
            });
        }
        // Snapshot what was just seeded. Read back from the inputs rather than reusing `fields`, so the
        // record is exactly what the write-through will read — if `set_content` ever normalized its
        // argument, comparing against the pre-normalization string would make every open look edited.
        self.seeded_profile_text = Some(self.profile_text(cx));
    }

    /// The current text of all eight profile inputs, in `profile_input_list` order.
    fn profile_text(&self, cx: &App) -> [String; 8] {
        self.profile_input_list()
            .map(|input| input.read(cx).content().to_owned())
    }

    /// Write every profile input back to the selected profile.
    ///
    /// Reads all eight inputs on each edit rather than tracking which one fired: the alternative is
    /// eight near-identical handlers, and this one runs only on keystrokes in a settings pane.
    ///
    /// Ignores events that arrive while the inputs belong to a different profile than the current
    /// selection — that is a seed in flight, and applying it would copy one profile's values onto
    /// another.
    fn on_profile_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            event,
            TextInputEvent::Changed | TextInputEvent::Confirm | TextInputEvent::ConfirmReverse
        ) {
            return;
        }
        let Some(id) = self.selected_profile else {
            return;
        };
        // The inputs must already be attributed to this profile. A mismatch means `sync_profile_inputs`
        // is mid-flight (its deferred `Changed` events are arriving), and the text belongs to neither
        // profile yet.
        if self.profile_inputs_for != Some(id) {
            return;
        }
        // Nothing to write unless the *text* differs from what seeding put there.
        //
        // Compared against the seeded text, not against the stored profile. `set_content` emits
        // `Changed` whose delivery GPUI defers past the render that seeded it, so this handler always
        // runs once per selection on text the user never touched — and comparing the *parsed* result to
        // the stored struct answers "changed" in exactly the case that must not be written, because the
        // text↔struct round trip is lossy for input the parser refuses. Verified against a real
        // `profiles.json`: with the struct comparison alone, merely opening this pane deleted a
        // hand-edited `"BAD KEY"` env entry from disk. Raw text is immune — seeded text equals itself.
        let current = self.profile_text(cx);
        let Some(seeded) = self.seeded_profile_text.clone() else {
            // Not seeded yet, so there is no baseline to diff against and nothing can be attributed to
            // the user. `sync_profile_inputs` always seeds before the pane can be interacted with.
            return;
        };
        if seeded == current {
            return;
        }

        let Some(stored) = self.profile_store.read(cx).profile(id).cloned() else {
            return;
        };
        // Apply only the fields whose *own* text changed, field by field.
        //
        // Rebuilding the whole struct from all eight inputs is wrong, even behind the guard above:
        // these inputs are seeded once per selection, so another surface writing the same profile
        // leaves them stale — and a whole-struct write then reverts that surface's edit. Reproduced
        // live: the Advanced → Shell pane sets `shell_program` to `/bin/bash`, then typing a single
        // character into this pane's *Name* field wrote `shell_program: ""` back over it, because the
        // Profiles pane's shell input still held the value from when it was seeded. Diffing per field
        // makes a stale input inert: it matches its own seed, so it is never applied.
        let mut edited = stored.clone();
        let changed = |index: usize| seeded[index] != current[index];
        // Indices follow `profile_input_list` order; `profile_text` builds `current` from it.
        if changed(0) {
            // An all-whitespace name would render as a blank row in the picker with nothing to click;
            // keep the stored name until the user types something real.
            let name = current[0].trim();
            if !name.is_empty() {
                edited.name = name.to_owned();
            }
        }
        if changed(1) {
            edited.shell_program = current[1].trim().to_owned();
        }
        if changed(2) {
            edited.shell_args = crate::profile_editor::parse_args(&current[2]);
        }
        if changed(3) {
            // Empty means "inherit" (active terminal's cwd, then the project root), which is `None` —
            // not `Some("")`, which would be a path that never exists.
            let cwd = current[3].trim();
            edited.cwd = (!cwd.is_empty()).then(|| std::path::PathBuf::from(cwd));
        }
        if changed(4) {
            let (mut env, _rejected) = crate::profile_editor::parse_env(&current[4]);
            // Carry back the entries the editor cannot show (a value containing a newline). They are
            // valid POSIX values, just not expressible one-per-line — without this, editing any other
            // variable would delete them. Parsed entries win on a key collision: that key is visible in
            // the editor, so what the user sees is what they get.
            for (key, value) in crate::profile_editor::unrepresentable_env(&stored.env) {
                env.entry(key).or_insert(value);
            }
            edited.env = env;
        }
        // The three remote fields build one value, so any of them moving re-derives it from all three.
        if changed(5) || changed(6) || changed(7) {
            edited.remote =
                crate::profile_editor::parse_remote(&current[5], &current[6], &current[7]);
        }
        // Baseline moves only now, and only for text that was actually consumed above. Set before the
        // no-op return below so a keystroke that parses to the same profile (typing then deleting a
        // character) still updates the baseline instead of re-diffing against an ever-older seed.
        self.seeded_profile_text = Some(current);
        // A keystroke can leave the parsed profile identical (whitespace, a refused env line); don't
        // churn the file for it.
        if edited == stored {
            return;
        }
        self.profile_store.update(cx, |store, cx| {
            store.update(id, |profile| *profile = edited, cx);
        });
        cx.notify();
    }

    /// Create a profile, select it, and focus its name field so it can be renamed immediately.
    fn add_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.settings.read(cx).config().language;
        let profile = eggie_domain::Profile::new(language.new_profile_name().to_owned());
        let id = profile.id;
        self.profile_store
            .update(cx, |store, cx| store.add(profile, cx));
        self.select_profile(id, cx);
        // The name is a placeholder, so put the caret in it rather than making the user hunt for it.
        self.profile_inputs
            .name
            .read(cx)
            .focus_handle()
            .focus(window, cx);
    }

    /// Point the pane at `id`. The inputs are re-seeded by the next `render` (see
    /// [`Self::sync_profile_inputs`]), which is also the path a selection change from another window
    /// travels, so there is only one seeding code path.
    fn select_profile(&mut self, id: eggie_domain::ProfileId, cx: &mut Context<Self>) {
        if self.selected_profile == Some(id) {
            return;
        }
        self.selected_profile = Some(id);
        self.open_selector = None;
        self.badge_picker_open = false;
        cx.notify();
    }

    /// Confirm, then delete the profile. The built-in Default is not deletable and never reaches here.
    ///
    /// Projects naming it as their default are cleaned up too. That is belt-and-braces —
    /// `ProfileStore::resolve` already sends a stale id to Default — but it keeps a dangling id off
    /// disk, where a future profile could otherwise inherit it (import, restore from backup) and
    /// silently become some project's default.
    fn prompt_delete_profile(
        &mut self,
        id: eggie_domain::ProfileId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profile_store.read(cx).profile(id) else {
            return;
        };
        if profile.builtin {
            return;
        }
        let language = self.settings.read(cx).config().language;
        let message = language.delete_profile_message(&profile.name);
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            language.delete_profile_title(),
            Some(&message),
            &[language.delete(), language.cancel()],
            cx,
        );
        let settings_window = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            settings_window
                .update(cx, |settings, cx| {
                    let removed = settings
                        .profile_store
                        .update(cx, |store, cx| store.remove(id, cx));
                    if !removed {
                        return;
                    }
                    // Clear the deleted id from any project that named it as their default. The
                    // "anything changed" return is deliberately ignored: the store already skips the
                    // write and the notify when no project referenced this profile, and there is
                    // nothing further for this callsite to do either way.
                    settings.project_store.update(cx, |store, cx| {
                        let _ = store.forget_profile(id, cx);
                    });
                    // Hand the pane to the built-in Default rather than leaving it on a dead id.
                    let default = settings.profile_store.read(cx).default_profile().id;
                    settings.select_profile(default, cx);
                })
                .ok();
        })
        .detach();
    }

    fn set_theme_mode(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.theme_mode = mode, cx)
        });
    }

    /// Switch between "the terminal follows the interface theme" and "the terminal has its own pair".
    ///
    /// Opting into the custom pair seeds it from the interface pair (see
    /// [`AppSettings::use_custom_terminal_themes`]) so the toggle itself changes nothing on screen.
    /// Going back keeps the custom pair on disk, so toggling twice does not discard the choice.
    fn set_terminal_theme_source(&mut self, custom: bool, cx: &mut Context<Self>) {
        let lua = &self.settings.read(cx).lua_config().registered;
        if lua.terminal_light_theme.is_some() || lua.terminal_dark_theme.is_some() {
            return;
        }
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |config| {
                    if custom {
                        config.use_custom_terminal_themes();
                    } else {
                        config.terminal_theme_source = TerminalThemeSource::FollowInterface;
                    }
                },
                cx,
            )
        });
    }

    /// Light up `name` as a transient hover preview across the app (real terminals + the settings
    /// preview), attributed to `kind`'s slot so it resolves through the same layering the committed
    /// value would. No-op if the name doesn't resolve to a known theme, or if `kind` picks a font.
    fn preview_theme(&mut self, kind: SelectorKind, name: &str, cx: &mut Context<Self>) {
        if self.lua_manages_selector(kind, cx) {
            return;
        }
        let profile_id = self.selected_profile_id(cx);
        let Some(slot) = kind.preview_slot(profile_id) else {
            return;
        };
        if let Some(theme) = theme_catalog().theme_by_name(name) {
            self.settings
                .update(cx, |store, cx| store.set_theme_preview(slot, theme, cx));
        }
    }

    /// Drop any active hover preview, restoring the persisted theme everywhere.
    fn clear_theme_preview(&mut self, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |store, cx| store.clear_theme_preview(cx));
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

    fn set_shell_integration_path(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.shell_integration_path = enabled, cx)
        });
        self.update_default_profile(|profile| profile.shell_integration_path = enabled, cx);
    }

    fn set_copy_on_select(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.copy_on_select = enabled, cx)
        });
    }

    fn set_auto_check_updates(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.auto_check_updates = enabled, cx)
        });
    }

    fn set_update_channel(&mut self, channel: UpdateChannel, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.update_channel = channel, cx)
        });
        // Re-check against the newly selected channel's feed so a switch is
        // reflected immediately (silent: a failure just stays quiet).
        self.updates.update(cx, |controller, cx| {
            controller.check(true, channel.slug(), cx)
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
                    settings.font_thicken_strength =
                        (i32::from(settings.font_thicken_strength) + delta).clamp(0, 255) as u8;
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
        if self
            .settings
            .read(cx)
            .lua_config()
            .registered
            .terminal_font
            .is_some()
        {
            return;
        }
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

    fn change_scrollback_lines(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let next = (settings.scrollback_lines as i64 + delta)
                        .clamp(MIN_SCROLLBACK_LINES as i64, MAX_SCROLLBACK_LINES as i64);
                    settings.scrollback_lines = next as usize;
                },
                cx,
            )
        });
    }

    /// Shared renderer for the settings' segmented button groups (the "pill" control): a rounded
    /// container holding one clickable pill per option, with the selected pill highlighted.
    ///
    /// Each option is `(id_suffix, label, selected)`; the pill's element id is
    /// `format!("{id_prefix}-{id_suffix}")`, matching the ids each call site built by hand.
    /// `on_pick` receives the picked option's index (matching `options` order) so the caller can
    /// map it back to its enum/bool and call the appropriate setter.
    fn render_segmented(
        &self,
        id_prefix: &str,
        options: Vec<(SharedString, SharedString, bool)>,
        on_pick: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_pick = std::rc::Rc::new(on_pick);
        options
            .into_iter()
            .enumerate()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (index, (id_suffix, label, selected))| {
                    let on_pick = on_pick.clone();
                    control.child(
                        div()
                            .id(format!("{id_prefix}-{id_suffix}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(
                                cx.listener(move |settings, _, _, cx| on_pick(settings, index, cx)),
                            )
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    /// A bordered −/value/+ stepper. The caller pre-formats the current value into `display`,
    /// picks the box width, and supplies `on_step`, which receives the direction (-1 or +1) and
    /// applies the control's own delta before invoking the matching change setter.
    fn render_stepper(
        &self,
        dec_id: &str,
        inc_id: &str,
        display: String,
        can_dec: bool,
        can_inc: bool,
        value_box_width: f32,
        on_step: impl Fn(&mut Self, i8, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_step = std::rc::Rc::new(on_step);
        let button = |id: SharedString,
                      label: &'static str,
                      enabled: bool,
                      dir: i8,
                      on_step: std::rc::Rc<dyn Fn(&mut Self, i8, &mut Context<Self>)>,
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
                        .on_click(cx.listener(move |settings, _, _, cx| on_step(settings, dir, cx)))
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
                SharedString::from(dec_id.to_owned()),
                "−",
                can_dec,
                -1,
                on_step.clone(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(value_box_width))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(display),
            )
            .child(button(
                SharedString::from(inc_id.to_owned()),
                "+",
                can_inc,
                1,
                on_step,
                cx,
            ))
            .into_any_element()
    }

    fn render_mode_control(&self, selected: ThemeMode, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = ThemeMode::ALL
            .into_iter()
            .map(|mode| {
                let label = match mode {
                    ThemeMode::Dark => language.theme_mode_dark(),
                    ThemeMode::Light => language.theme_mode_light(),
                    ThemeMode::System => language.theme_mode_system(),
                };
                (
                    SharedString::from(mode.label()),
                    SharedString::from(label),
                    selected == mode,
                )
            })
            .collect();
        self.render_segmented(
            "theme-mode",
            options,
            |settings, index, cx| settings.set_theme_mode(ThemeMode::ALL[index], cx),
            cx,
        )
    }

    /// "Follow the interface theme" vs "use a custom pair" for the terminal.
    fn render_terminal_theme_source_control(
        &self,
        selected: TerminalThemeSource,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = TerminalThemeSource::ALL
            .into_iter()
            .map(|source| {
                let label = match source {
                    TerminalThemeSource::FollowInterface => {
                        language.terminal_theme_follow_interface()
                    }
                    TerminalThemeSource::Custom => language.terminal_theme_custom(),
                };
                (
                    SharedString::from(format!("{source:?}")),
                    SharedString::from(label),
                    selected == source,
                )
            })
            .collect();
        self.render_segmented(
            "terminal-theme-source",
            options,
            |settings, index, cx| {
                settings.set_terminal_theme_source(
                    TerminalThemeSource::ALL[index] == TerminalThemeSource::Custom,
                    cx,
                )
            },
            cx,
        )
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
        let options =
            BellMode::ALL
                .into_iter()
                .fold(div().flex().flex_col().py_1(), |list, mode| {
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
                });
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
        let options = [(false, language.block()), (true, language.allow())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "osc-clipboard-read",
            options,
            |settings, index, cx| settings.set_osc_clipboard_read(index == 1, cx),
            cx,
        )
    }

    fn render_detect_urls_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "detect-urls",
            options,
            |settings, index, cx| settings.set_detect_urls(index == 1, cx),
            cx,
        )
    }

    fn render_auto_check_updates_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "auto-check-updates",
            options,
            |settings, index, cx| settings.set_auto_check_updates(index == 1, cx),
            cx,
        )
    }

    fn render_update_channel_control(
        &self,
        selected: UpdateChannel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = UpdateChannel::ALL
            .into_iter()
            .map(|channel| {
                (
                    SharedString::from(channel.slug()),
                    SharedString::from(language.update_channel_label(channel)),
                    selected == channel,
                )
            })
            .collect();
        self.render_segmented(
            "update-channel",
            options,
            |settings, index, cx| settings.set_update_channel(UpdateChannel::ALL[index], cx),
            cx,
        )
    }

    fn render_shell_integration_path_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "shell-integration-path",
            options,
            |settings, index, cx| settings.set_shell_integration_path(index == 1, cx),
            cx,
        )
    }

    fn render_synthetic_style_control(
        &self,
        kind: SyntheticStyleKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            &format!("synthetic-{}", kind.slug()),
            options,
            move |settings, index, cx| settings.set_synthetic_style(kind, index == 1, cx),
            cx,
        )
    }

    fn render_ligatures_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "ligatures",
            options,
            |settings, index, cx| settings.set_ligatures(index == 1, cx),
            cx,
        )
    }

    fn render_shaping_break_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "shaping-break",
            options,
            |settings, index, cx| settings.set_shaping_break_cursor(index == 1, cx),
            cx,
        )
    }

    fn render_font_thicken_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "font-thicken",
            options,
            |settings, index, cx| settings.set_font_thicken(index == 1, cx),
            cx,
        )
    }

    fn render_font_thicken_strength_control(
        &self,
        strength: u8,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_stepper(
            "decrease-font-thicken-strength",
            "increase-font-thicken-strength",
            format!("{strength}"),
            strength > 0,
            strength < 255,
            54.,
            |settings, dir, cx| settings.change_font_thicken_strength(i32::from(dir) * 16, cx),
            cx,
        )
    }

    fn render_copy_on_select_control(&self, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "copy-on-select",
            options,
            |settings, index, cx| settings.set_copy_on_select(index == 1, cx),
            cx,
        )
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
        let options = CursorBlink::ALL
            .into_iter()
            .map(|blink| {
                (
                    SharedString::from(blink.slug()),
                    SharedString::from(Self::cursor_blink_label(language, blink)),
                    selected == blink,
                )
            })
            .collect();
        self.render_segmented(
            "cursor-blink",
            options,
            |settings, index, cx| settings.set_cursor_blink(CursorBlink::ALL[index], cx),
            cx,
        )
    }

    fn render_language_control(&self, selected: Language, cx: &mut Context<Self>) -> AnyElement {
        let options = Language::ALL
            .into_iter()
            .map(|language| {
                (
                    SharedString::from(language.label()),
                    SharedString::from(language.label()),
                    selected == language,
                )
            })
            .collect();
        self.render_segmented(
            "language",
            options,
            |settings, index, cx| settings.set_language(Language::ALL[index], cx),
            cx,
        )
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
        let managed = self.lua_manages_selector(kind, cx);
        let dropdown_position = self.selector_dropdown_position(kind, window);
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
                    .when(!managed, |element| {
                        element
                            .cursor_pointer()
                            .hover(|element| element.border_color(rgb(self.colors.accent)))
                    })
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        if settings.lua_manages_selector(kind, cx) {
                            return;
                        }
                        if settings.open_selector == Some(kind) {
                            settings.open_selector = None;
                            settings.clear_theme_preview(cx);
                        } else {
                            settings.open_selector = Some(kind);
                            let language = settings.settings.read(cx).config().language;
                            let placeholder = match kind {
                                // Every theme slot searches one half of the catalog, so the placeholder
                                // follows the slot's darkness rather than naming the layer.
                                SelectorKind::InterfaceDark
                                | SelectorKind::InterfaceLight
                                | SelectorKind::TerminalDark
                                | SelectorKind::TerminalLight
                                | SelectorKind::ProfileTerminalDark
                                | SelectorKind::ProfileTerminalLight => {
                                    if kind.is_dark() {
                                        language.search_dark_themes()
                                    } else {
                                        language.search_light_themes()
                                    }
                                }
                                SelectorKind::Font
                                | SelectorKind::FontBold
                                | SelectorKind::FontItalic
                                | SelectorKind::FontBoldItalic
                                | SelectorKind::ProfileFont => language.search_fonts(),
                            };
                            settings.selector_search_input.update(cx, |input, cx| {
                                input.set_placeholder(placeholder);
                                input.set_content("", cx);
                            });
                            settings
                                .selector_scroll_handle
                                .set_offset(point(px(0.), px(0.)));
                            let handle = settings.selector_search_input.read(cx).focus_handle();
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
                            .child(if managed {
                                format!("{value} · init.lua")
                            } else {
                                value
                            }),
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

    fn selector_dropdown_position(
        &self,
        kind: SelectorKind,
        window: &Window,
    ) -> Option<gpui::Point<Pixels>> {
        self.selector_bounds[kind.index()].map(|bounds| {
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
        })
    }

    fn render_selector_dropdown(
        &self,
        kind: SelectorKind,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = self.settings.read(cx).config();
        let language = config.language;
        // The option list comes from `selector_names` (the single source), so this only has to answer
        // "what is the current value". Exhaustive on purpose: a catch-all would let a new slot read
        // another slot's field, which compiles and silently shows the wrong selection.
        let current = match kind {
            SelectorKind::InterfaceDark => config.interface_dark_theme.clone(),
            SelectorKind::InterfaceLight => config.interface_light_theme.clone(),
            SelectorKind::TerminalDark => config.terminal_dark_theme.clone(),
            SelectorKind::TerminalLight => config.terminal_light_theme.clone(),
            SelectorKind::Font => config.font_family.clone(),
            SelectorKind::FontBold => config.font_family_bold.clone(),
            SelectorKind::FontItalic => config.font_family_italic.clone(),
            SelectorKind::FontBoldItalic => config.font_family_bold_italic.clone(),
            // Profile overrides read from the store, not from `config`. An unset override is an empty
            // string here, which is what makes the "follow global setting" entry render as selected.
            SelectorKind::ProfileTerminalDark
            | SelectorKind::ProfileTerminalLight
            | SelectorKind::ProfileFont => {
                let id = self.selected_profile_id(cx);
                let appearance = self
                    .profile_store
                    .read(cx)
                    .profile(id)
                    .map(|profile| profile.appearance.clone())
                    .unwrap_or_default();
                match kind {
                    SelectorKind::ProfileTerminalDark => {
                        appearance.terminal_dark_theme.unwrap_or_default()
                    }
                    SelectorKind::ProfileTerminalLight => {
                        appearance.terminal_light_theme.unwrap_or_default()
                    }
                    _ => appearance.font_family.unwrap_or_default(),
                }
            }
        };
        let items = self.selector_names(kind).to_vec();
        let query_owned = self
            .selector_search_input
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let items = items
            .into_iter()
            .filter(|item| selector_matches(item, &query_owned))
            .collect::<Vec<_>>();
        let has_items = !items.is_empty();

        // Per-style families (bold/italic/bold-italic) can be cleared back to "follow the regular
        // family", and every profile override back to "follow the global setting". Offer that as a
        // pinned first entry when not actively searching, so a set value is always reversible (an
        // empty stored value means "inherit"). The global regular Font and the global themes always
        // resolve to a concrete value, so they get no clear entry.
        let clearable = matches!(
            kind,
            SelectorKind::FontBold | SelectorKind::FontItalic | SelectorKind::FontBoldItalic
        ) || kind.is_profile();
        let show_clear = clearable && query_owned.is_empty();
        let clear_selected = show_clear && current.trim().is_empty();
        // The two clear entries say different things: a style font falls back to the regular family,
        // a profile override falls back to the global setting.
        let clear_label = if kind.is_profile() {
            language.profile_appearance_inherit()
        } else {
            language.font_family_use_regular()
        };

        let mut list = div()
            .id("settings-selector-list")
            .flex()
            .flex_1()
            .min_h_0()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.selector_scroll_handle)
            .py_1();
        if show_clear {
            list = list.child(
                div()
                    .id("settings-selector-clear")
                    .flex()
                    .items_center()
                    .min_w_0()
                    .flex_none()
                    .h(px(32.))
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .when(clear_selected, |element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.accent))
                    })
                    .when(!clear_selected, |element| {
                        element.text_color(rgb(self.colors.muted))
                    })
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        if kind.is_profile() {
                            settings.apply_profile_selector_choice(kind, None, cx);
                        } else {
                            settings.settings.update(cx, |store, cx| {
                                store.update(
                                    // Exhaustive on purpose: the non-clearable slots have to be listed
                                    // rather than swept into a catch-all, or a new clearable slot would
                                    // compile with a clear button that silently does nothing.
                                    |config| match kind {
                                        SelectorKind::FontBold => config.font_family_bold.clear(),
                                        SelectorKind::FontItalic => {
                                            config.font_family_italic.clear()
                                        }
                                        SelectorKind::FontBoldItalic => {
                                            config.font_family_bold_italic.clear()
                                        }
                                        // Not clearable: these always resolve to a concrete value, so
                                        // `clearable` never offers this row for them.
                                        SelectorKind::InterfaceDark
                                        | SelectorKind::InterfaceLight
                                        | SelectorKind::TerminalDark
                                        | SelectorKind::TerminalLight
                                        | SelectorKind::Font => {}
                                        // Handled above: these write to the profile store.
                                        SelectorKind::ProfileTerminalDark
                                        | SelectorKind::ProfileTerminalLight
                                        | SelectorKind::ProfileFont => {}
                                    },
                                    cx,
                                )
                            });
                        }
                        settings.open_selector = None;
                        cx.notify();
                    }))
                    .child(div().min_w_0().truncate().child(clear_label)),
            );
        }
        // Theme selectors get a live hover preview: resting on an option lights the theme up across
        // the real terminals and the preview swatch. Font selectors don't (fonts can't preview live).
        // Every theme slot previews — including the per-profile overrides, which reach the same live
        // path but apply only to that profile's terminals.
        let is_theme_selector = kind.is_theme();
        for (index, item) in items.into_iter().enumerate() {
            let selected = item == current;
            let selected_item = item.clone();
            let hover_item = item.clone();
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
                    .when(kind.is_font(), |element| {
                        element.font_family(SharedString::from(item.clone()))
                    })
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .when(is_theme_selector, |element| {
                        element.on_hover(cx.listener(move |settings, hovered: &bool, _, cx| {
                            // Preview on enter only; leaving an option to enter the next one must not
                            // flip back to the persisted theme mid-glide. The dropdown container's
                            // own on_hover clears the preview once the pointer leaves the list.
                            if *hovered {
                                settings.preview_theme(kind, &hover_item, cx);
                            }
                        }))
                    })
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        // Committing supersedes the preview: clear it so the resolved theme falls back
                        // to the now-persisted value (identical pixels, but no stale override left).
                        settings.clear_theme_preview(cx);
                        settings.commit_selector_choice(kind, selected_item.clone(), cx);
                        settings.open_selector = None;
                        cx.notify();
                    }))
                    .child(div().min_w_0().truncate().child(item)),
            );
        }
        if !has_items && !show_clear {
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
            .when(is_theme_selector, |element| {
                // Pointer left the dropdown entirely (or entered its blank gaps between options):
                // drop the hover preview so the persisted theme comes back. Option-to-option glides
                // stay previewed because each option re-arms the preview on enter.
                element.on_hover(cx.listener(move |settings, hovered: &bool, _, cx| {
                    if !*hovered {
                        settings.clear_theme_preview(cx);
                    }
                }))
            })
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |settings, _, cx| {
                    if settings.open_selector == Some(kind) {
                        settings.open_selector = None;
                        settings.clear_theme_preview(cx);
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
            settings.update(
                |config| {
                    config.keybindings.remove(&id);
                },
                cx,
            )
        });
    }

    fn reset_all_bindings(&mut self, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|config| config.keybindings.clear(), cx)
        });
    }

    /// The "About" panel: a branded header (logo + name + version), a
    /// check-for-updates button, build metadata, and the update preferences that
    /// used to live under "Advanced".
    fn render_about_section(&self, language: Language, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = self.colors;
        // Copy the two update-preference values out so the `settings` borrow is
        // released before the `&mut cx` render calls below.
        let (auto_check_updates, update_channel) = {
            let config = self.settings.read(cx).config();
            (config.auto_check_updates, config.update_channel)
        };
        let auto_check_updates_control =
            self.render_auto_check_updates_control(auto_check_updates, cx);
        let update_channel_control = self.render_update_channel_control(update_channel, cx);

        let build_mode = if cfg!(debug_assertions) {
            language.build_mode_debug()
        } else {
            language.build_mode_release()
        };

        // A metadata line: label on the left, value right-aligned and muted.
        let metadata_row = |label: &'static str, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_6()
                .min_h(px(44.))
                .px_4()
                .py_2()
                .child(div().flex_none().text_color(rgb(colors.text)).child(label))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .text_right()
                        .child(value),
                )
                .into_any_element()
        };

        let header = div()
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .gap_2()
            .py_4()
            .child(img(logo_image()).w(px(72.)).h(px(72.)))
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.text))
                    .child("Eggie"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(format!(
                        "{} {}",
                        language.version_row(),
                        env!("CARGO_PKG_VERSION")
                    )),
            )
            .child(div().pt_2().child(crate::update_window::button(
                language.check_for_updates_button(),
                true,
                colors,
                cx.listener(|window, _, _, cx| {
                    let channel = window.settings.read(cx).config().update_channel.slug();
                    window.updates.update(cx, |controller, cx| {
                        if matches!(
                            controller.state(),
                            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Error(_)
                        ) {
                            controller.check(false, channel, cx);
                        }
                    });
                    crate::update_window::open_update_window(
                        window.updates.clone(),
                        window.settings.clone(),
                        cx,
                    );
                }),
            )))
            .into_any_element();

        let metadata = settings_section(
            Some(language.build_info_section()),
            vec![
                metadata_row(language.version_row(), env!("CARGO_PKG_VERSION").to_owned()),
                metadata_row(
                    language.protocol_version_row(),
                    eggie_protocol::PROTOCOL_VERSION.to_string(),
                ),
                metadata_row(
                    language.build_date_row(),
                    env!("EGGIE_BUILD_DATE").to_owned(),
                ),
                metadata_row(language.build_id_row(), env!("EGGIE_BUILD_ID").to_owned()),
                metadata_row(language.build_mode_row(), build_mode.to_owned()),
            ],
            colors,
        );

        let updates = settings_section(
            Some(language.updates_section()),
            vec![
                settings_row(
                    language.auto_check_updates_row(),
                    language.auto_check_updates_description(),
                    auto_check_updates_control,
                    colors,
                ),
                settings_row(
                    language.update_channel_row(),
                    language.update_channel_description(),
                    update_channel_control,
                    colors,
                ),
            ],
            colors,
        );

        vec![header, metadata, updates]
    }

    /// The Profiles pane: the profile list, then the selected profile's fields.
    fn render_profiles_section(
        &self,
        language: Language,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let selected_id = self.selected_profile_id(cx);
        let store = self.profile_store.read(cx);
        let Some(profile) = store.profile(selected_id).cloned() else {
            return Vec::new();
        };
        let entries: Vec<(eggie_domain::ProfileId, String, bool, Option<u32>)> = store
            .profiles()
            .iter()
            .map(|profile| {
                (
                    profile.id,
                    profile.name.clone(),
                    profile.builtin,
                    profile.badge_color,
                )
            })
            .collect();

        let mut list_rows = Vec::with_capacity(entries.len());
        for (id, name, builtin, badge_color) in entries {
            let selected = id == selected_id;
            list_rows.push(
                div()
                    .id(("settings-profile-row", id.as_u128() as usize))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_3()
                    .cursor_pointer()
                    .when(selected, |element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.accent))
                    })
                    .when(!selected, |element| {
                        element.hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    })
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.select_profile(id, cx);
                    }))
                    // The badge dot doubles as the list's leading marker, so the color a tab will
                    // carry is visible while picking the profile, not only after launching it.
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .when_some(badge_color, |element, color| element.bg(rgb(color)))
                            .when(badge_color.is_none(), |element| {
                                element.border_1().border_color(rgb(self.colors.border))
                            }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .when(builtin, |element| {
                        element.child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(self.colors.muted))
                                .child(language.builtin_profile_marker()),
                        )
                    })
                    .into_any_element(),
            );
        }

        // The list's caption is omitted for the same reason as the General pane's first section: the
        // pane title right above it already reads "Profiles". The add/delete buttons sit *below* the
        // card, matching how macOS Settings places the controls for a list (Login Items, Users &
        // Groups) rather than leaving a button row floating in empty space above it.
        let list = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .child(settings_card(list_rows, self.colors))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .px_1()
                    .child(
                        div()
                            .id("settings-profile-add")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(self.colors.muted))
                            .hover(|element| {
                                element
                                    .bg(rgb(self.colors.panel_alt))
                                    .text_color(rgb(self.colors.text))
                            })
                            .tooltip(tooltip_text(
                                SharedString::from(language.add_profile_tooltip()),
                                self.colors,
                            ))
                            .on_click(cx.listener(|settings, _, window, cx| {
                                settings.add_profile(window, cx);
                            }))
                            .child(icon(IconName::Add)),
                    )
                    // Deleting the built-in Default is not offered at all: it is the fallback
                    // every stale profile reference resolves to, so the control is absent
                    // rather than present-and-inert.
                    .when(!profile.builtin, |element| {
                        element.child(
                            div()
                                .id("settings-profile-delete")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(24.))
                                .rounded_md()
                                .cursor_pointer()
                                .text_color(rgb(self.colors.muted))
                                .hover(|element| {
                                    element
                                        .bg(rgb(self.colors.panel_alt))
                                        .text_color(rgb(self.colors.text))
                                })
                                .tooltip(tooltip_text(
                                    SharedString::from(language.delete_profile_tooltip()),
                                    self.colors,
                                ))
                                .on_click(cx.listener(move |settings, _, window, cx| {
                                    settings.prompt_delete_profile(selected_id, window, cx);
                                }))
                                .child(icon(IconName::Close)),
                        )
                    }),
            )
            .into_any_element();

        // --- The selected profile's fields ---
        let name_row = settings_row(
            language.profile_name_row(),
            language.profile_name_description(),
            self.render_profile_input("settings-profile-name", &self.profile_inputs.name, 220.),
            self.colors,
        );

        let launch_rows = vec![
            settings_row(
                language.shell_program_row(),
                language.shell_program_description(),
                self.render_profile_input(
                    "settings-profile-shell-program",
                    &self.profile_inputs.shell_program,
                    220.,
                ),
                self.colors,
            ),
            settings_row(
                language.shell_args_row(),
                language.shell_args_description(),
                self.render_profile_input(
                    "settings-profile-shell-args",
                    &self.profile_inputs.shell_args,
                    220.,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_cwd_row(),
                language.profile_cwd_description(),
                self.render_profile_input("settings-profile-cwd", &self.profile_inputs.cwd, 220.),
                self.colors,
            ),
            settings_row(
                language.shell_integration_path_row(),
                language.shell_integration_path_description(),
                self.render_profile_path_control(profile.shell_integration_path, language, cx),
                self.colors,
            ),
            // The env editor is multi-line, so it gets a full-width row of its own rather than a
            // squeezed control column.
            self.render_profile_env_row(language, cx),
        ];

        let remote_target = crate::profile_editor::parse_remote(
            self.profile_inputs.remote_host.read(cx).content(),
            self.profile_inputs.remote_user.read(cx).content(),
            self.profile_inputs.remote_port.read(cx).content(),
        );
        // Surface the same judgement the launch path makes: an unusable target is dropped there, so
        // saying so here is the difference between "my profile connects" and a silent local shell.
        let remote_warning = remote_target
            .as_ref()
            .is_some_and(|target| !target.is_usable())
            .then(|| language.profile_remote_invalid());
        let remote_rows = vec![
            self.render_profile_remote_host_row(language, remote_warning),
            settings_row(
                language.profile_remote_user_row(),
                language.profile_remote_user_description(),
                self.render_profile_input(
                    "settings-profile-remote-user",
                    &self.profile_inputs.remote_user,
                    160.,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_remote_port_row(),
                language.profile_remote_port_description(),
                self.render_profile_input(
                    "settings-profile-remote-port",
                    &self.profile_inputs.remote_port,
                    80.,
                ),
                self.colors,
            ),
        ];

        let appearance_rows = vec![
            settings_row(
                language.profile_terminal_dark_theme_row(),
                language.profile_terminal_dark_theme_description(),
                self.render_selector(
                    "settings-profile-terminal-dark-theme",
                    profile
                        .appearance
                        .terminal_dark_theme
                        .clone()
                        .unwrap_or_else(|| language.profile_appearance_inherit().to_owned()),
                    None,
                    SelectorKind::ProfileTerminalDark,
                    window,
                    cx,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_terminal_light_theme_row(),
                language.profile_terminal_light_theme_description(),
                self.render_selector(
                    "settings-profile-terminal-light-theme",
                    profile
                        .appearance
                        .terminal_light_theme
                        .clone()
                        .unwrap_or_else(|| language.profile_appearance_inherit().to_owned()),
                    None,
                    SelectorKind::ProfileTerminalLight,
                    window,
                    cx,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_font_row(),
                language.profile_font_description(),
                self.render_selector(
                    "settings-profile-font",
                    profile
                        .appearance
                        .font_family
                        .clone()
                        .unwrap_or_else(|| language.profile_appearance_inherit().to_owned()),
                    // Render the trigger in the chosen font, as the global font selector does.
                    profile.appearance.font_family.clone(),
                    SelectorKind::ProfileFont,
                    window,
                    cx,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_font_size_row(),
                language.profile_font_size_description(),
                self.render_profile_font_size_control(&profile, language, cx),
                self.colors,
            ),
            settings_row(
                language.profile_badge_row(),
                language.profile_badge_description(),
                self.render_profile_badge_control(&profile, language, cx),
                self.colors,
            ),
        ];

        vec![
            list,
            settings_section(
                Some(language.profile_name_row()),
                vec![name_row],
                self.colors,
            ),
            settings_section(
                Some(language.profile_launch_section()),
                launch_rows,
                self.colors,
            ),
            settings_section(
                Some(language.profile_remote_section()),
                remote_rows,
                self.colors,
            ),
            settings_section(
                Some(language.profile_appearance_section()),
                appearance_rows,
                self.colors,
            ),
        ]
    }

    /// A bordered single-line input for a profile field, `width` px wide.
    fn render_profile_input(
        &self,
        id: &'static str,
        input: &Entity<TextInput>,
        width: f32,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .w(px(width))
            .h(px(30.))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel_alt))
            .text_size(px(12.))
            .text_color(rgb(self.colors.text))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(input.clone()),
            )
            .into_any_element()
    }

    /// The remote host row, with the "this target is ignored" warning appended when it applies.
    fn render_profile_remote_host_row(
        &self,
        language: Language,
        warning: Option<&'static str>,
    ) -> AnyElement {
        let control = self.render_profile_input(
            "settings-profile-remote-host",
            &self.profile_inputs.remote_host,
            220.,
        );
        let colors = self.colors;
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
                    .child(language.profile_remote_host_row())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(language.profile_remote_host_description()),
                    )
                    .when_some(warning, |element, warning| {
                        // Tinted with the bell/attention color so a rejected target reads as a problem
                        // rather than as more description.
                        element.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(WARNING_TEXT))
                                .child(warning),
                        )
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    /// The multi-line environment editor, plus a count of ignored lines when the text has any.
    fn render_profile_env_row(&self, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        // Recompute from the live text rather than from `profile.env`: the stored map has already had
        // the bad lines dropped, so it cannot say how many there were.
        let (_, rejected) =
            crate::profile_editor::parse_env(self.profile_inputs.env.read(cx).content());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(language.profile_env_row())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(language.profile_env_description()),
                    ),
            )
            .child(
                div()
                    .id("settings-profile-env")
                    .w_full()
                    .h(px(96.))
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.panel_alt))
                    .text_size(px(12.))
                    .text_color(rgb(colors.text))
                    .overflow_hidden()
                    .child(self.profile_inputs.env.clone()),
            )
            .when(rejected > 0, |element| {
                element.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(WARNING_TEXT))
                        .child(language.profile_env_rejected(rejected)),
                )
            })
            .into_any_element()
    }

    fn render_profile_path_control(
        &self,
        enabled: bool,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = vec![
            (
                SharedString::from("on"),
                SharedString::from(language.cursor_blink_on()),
                enabled,
            ),
            (
                SharedString::from("off"),
                SharedString::from(language.cursor_blink_off()),
                !enabled,
            ),
        ];
        self.render_segmented(
            "settings-profile-path",
            options,
            move |settings, index, cx| {
                let Some(id) = settings.selected_profile else {
                    return;
                };
                settings.profile_store.update(cx, |store, cx| {
                    store.update(
                        id,
                        |profile| profile.shell_integration_path = index == 0,
                        cx,
                    );
                });
            },
            cx,
        )
    }

    /// The per-profile font-size stepper. Steps from the *resolved* size, so the first click moves
    /// relative to what the terminal currently shows rather than jumping to a fixed number; a
    /// dedicated entry clears the override back to the global size.
    fn render_profile_font_size_control(
        &self,
        profile: &eggie_domain::Profile,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let global = self.settings.read(cx).config().font_size;
        let override_size = profile.appearance.font_size;
        let effective = override_size.unwrap_or(global);
        // An unset override displays the inherited value in parentheses, so "follow global" is visible
        // as a state rather than looking identical to an override that happens to match.
        let display = match override_size {
            Some(size) => format!("{size:.0}"),
            None => format!("({global:.0})"),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.render_stepper(
                "settings-profile-font-size-dec",
                "settings-profile-font-size-inc",
                display,
                effective > MIN_FONT_SIZE,
                effective < MAX_FONT_SIZE,
                56.,
                move |settings, dir, cx| {
                    let Some(id) = settings.selected_profile else {
                        return;
                    };
                    let next = (effective + f32::from(dir)).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                    settings.profile_store.update(cx, |store, cx| {
                        store.update(id, |profile| profile.appearance.font_size = Some(next), cx);
                    });
                },
                cx,
            ))
            .when(override_size.is_some(), |element| {
                element.child(
                    div()
                        .id("settings-profile-font-size-clear")
                        .flex()
                        .items_center()
                        .h(px(30.))
                        .px_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(self.colors.muted))
                        .hover(|element| {
                            element
                                .bg(rgb(self.colors.panel_alt))
                                .text_color(rgb(self.colors.text))
                        })
                        .on_click(cx.listener(|settings, _, _, cx| {
                            let Some(id) = settings.selected_profile else {
                                return;
                            };
                            settings.profile_store.update(cx, |store, cx| {
                                store.update(id, |profile| profile.appearance.font_size = None, cx);
                            });
                        }))
                        .child(language.profile_appearance_inherit()),
                )
            })
            .into_any_element()
    }

    /// The badge color picker: a swatch that opens a small palette popover.
    fn render_profile_badge_control(
        &self,
        profile: &eggie_domain::Profile,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = profile.badge_color;
        let dropdown_position = self
            .badge_picker_bounds
            .map(|bounds| point(bounds.left(), bounds.bottom() + px(SELECTOR_DROPDOWN_GAP)));
        let settings_entity = cx.entity().downgrade();
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let settings_entity = settings_entity.clone();
                cx.defer(move |cx| {
                    settings_entity
                        .update(cx, |settings, cx| {
                            if settings.badge_picker_bounds != Some(bounds) {
                                settings.badge_picker_bounds = Some(bounds);
                                if settings.badge_picker_open {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .relative()
            .child(
                div()
                    .id("settings-profile-badge")
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(self.colors.panel_alt))
                    .cursor_pointer()
                    .hover(|element| element.border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.badge_picker_open = !settings.badge_picker_open;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size(px(12.))
                            .rounded_full()
                            .when_some(current, |element, color| element.bg(rgb(color)))
                            .when(current.is_none(), |element| {
                                element.border_1().border_color(rgb(self.colors.border))
                            }),
                    )
                    .child(div().text_size(px(12.)).child(match current {
                        Some(color) => format!("#{color:06X}"),
                        None => language.profile_badge_none().to_owned(),
                    })),
            )
            .when(self.badge_picker_open, |element| {
                let mut popover = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    popover = popover.position(position);
                }
                element.child(
                    deferred(popover.child(self.render_badge_palette(language, cx))).priority(2),
                )
            })
            .into_any_element()
    }

    fn render_badge_palette(&self, language: Language, cx: &mut Context<Self>) -> AnyElement {
        let mut swatches = div().flex().flex_wrap().gap_2().max_w(px(180.));
        for (index, color) in BADGE_COLORS.iter().copied().enumerate() {
            swatches = swatches.child(
                div()
                    .id(("settings-badge-swatch", index))
                    .size(px(20.))
                    .rounded_full()
                    .bg(rgb(color))
                    .cursor_pointer()
                    .hover(|element| element.border_2().border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.set_badge_color(Some(color), cx);
                    })),
            );
        }
        div()
            .id("settings-badge-palette")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .shadow_lg()
            // Floating overlay: block events for the rows painted behind it, so a click in the
            // popover's padding cannot bubble through and hit a control underneath.
            .occlude()
            .on_mouse_down_out(cx.listener(|settings, _, window, cx| {
                cx.defer_in(window, |settings, _, cx| {
                    if settings.badge_picker_open {
                        settings.badge_picker_open = false;
                        cx.notify();
                    }
                });
                let _ = settings;
            }))
            .child(swatches)
            .child(
                div()
                    .id("settings-badge-clear")
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(self.colors.muted))
                    .hover(|element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.text))
                    })
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.set_badge_color(None, cx);
                    }))
                    .child(language.profile_badge_none()),
            )
            .into_any_element()
    }

    fn set_badge_color(&mut self, color: Option<u32>, cx: &mut Context<Self>) {
        let Some(id) = self.selected_profile else {
            return;
        };
        self.profile_store.update(cx, |store, cx| {
            store.update(id, |profile| profile.badge_color = color, cx);
        });
        self.badge_picker_open = false;
        cx.notify();
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
            // Same reason as the General pane: the pane title already reads "Keyboard Shortcuts".
            settings_section(None, rows, colors),
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
        let effective =
            crate::keybindings::effective_keystroke(config, id).unwrap_or(spec.default_keystroke);
        let lua = &self.settings.read(cx).lua_config().registered;
        let lua_managed = lua.bindings.iter().any(|operation| match operation {
            eggie_config::BindingOp::Bind { key, action, .. } => key == effective || action == id,
            eggie_config::BindingOp::Unbind { key } => key == effective,
        });
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
            let shortcut = crate::command_palette::display_binding(config, lua, id)
                .unwrap_or_else(|| "Unbound".to_owned());
            if lua_managed {
                format!("{shortcut} · init.lua")
            } else {
                shortcut
            }
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
            .when(!lua_managed, |element| element.cursor_pointer())
            .when(!is_recording, |element| {
                element.hover(|element| element.bg(rgb(colors.panel_alt)))
            })
            .when(is_recording, |element| element.bg(rgb(colors.panel_alt)))
            .child(cell_text)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |settings, _, _, cx| {
                if lua_managed {
                    return;
                }
                if settings.recording.as_deref() == Some(recorded_id.as_str()) {
                    settings.cancel_recording(cx);
                } else {
                    settings.start_recording(recorded_id.clone(), cx);
                }
            }));

        let reset_id = id.to_string();
        let reset_button = div()
            .flex_none()
            .when(is_overridden && !lua_managed, |element| {
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
        let managed = self
            .settings
            .read(cx)
            .lua_config()
            .registered
            .terminal_font
            .is_some();
        self.render_stepper(
            "decrease-font-size",
            "increase-font-size",
            if managed {
                format!("{font_size:.0} · init.lua")
            } else {
                format!("{font_size:.0}")
            },
            !managed && font_size > MIN_FONT_SIZE,
            !managed && font_size < MAX_FONT_SIZE,
            54.,
            |settings, dir, cx| settings.change_font_size(f32::from(dir), cx),
            cx,
        )
    }

    fn render_minimum_contrast_control(
        &self,
        minimum_contrast: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_stepper(
            "decrease-minimum-contrast",
            "increase-minimum-contrast",
            format!("{minimum_contrast:.1}"),
            minimum_contrast > MIN_MINIMUM_CONTRAST,
            minimum_contrast < MAX_MINIMUM_CONTRAST,
            54.,
            |settings, dir, cx| settings.change_minimum_contrast(f32::from(dir) * 0.1, cx),
            cx,
        )
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
        self.render_stepper(
            decrease_id,
            increase_id,
            format!("{value:.0}"),
            value > MIN_TERMINAL_PADDING,
            value < MAX_TERMINAL_PADDING,
            54.,
            move |settings, dir, cx| settings.change_terminal_padding(axis, f32::from(dir), cx),
            cx,
        )
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
        let decrease_id = format!("decrease-adjust-{}", kind.slug());
        let increase_id = format!("increase-adjust-{}", kind.slug());
        self.render_stepper(
            &decrease_id,
            &increase_id,
            display,
            current > -64,
            current < 64,
            54.,
            move |settings, dir, cx| settings.change_metric_adjustment(kind, i32::from(dir), cx),
            cx,
        )
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
        self.render_stepper(
            decrease_id,
            increase_id,
            format!("{value}s"),
            value > MIN_PROGRESS_TIMEOUT_SECS,
            value < MAX_PROGRESS_TIMEOUT_SECS,
            62.,
            move |settings, dir, cx| {
                settings.change_progress_timeout(kind, i32::from(dir) * step, cx)
            },
            cx,
        )
    }

    fn render_scrollback_control(&self, value: usize, cx: &mut Context<Self>) -> AnyElement {
        self.render_stepper(
            "decrease-scrollback-lines",
            "increase-scrollback-lines",
            value.to_string(),
            value > MIN_SCROLLBACK_LINES,
            value < MAX_SCROLLBACK_LINES,
            72.,
            |settings, dir, cx| {
                settings.change_scrollback_lines(i64::from(dir) * SCROLLBACK_LINES_STEP, cx)
            },
            cx,
        )
    }

    /// Wrap a persistent shell `TextInput` entity in a bordered, fixed-width control box for a
    /// settings row. `id` disambiguates the two boxes (program vs args).
    fn render_shell_input_control(
        &self,
        id: &'static str,
        input: &Entity<TextInput>,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .w(px(220.))
            .h(px(30.))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel_alt))
            .text_size(px(12.))
            .text_color(rgb(self.colors.text))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(input.clone()),
            )
            .into_any_element()
    }

    fn render_theme_search(
        &self,
        kind: SelectorKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dropdown_position = self.selector_dropdown_position(kind, window);
        let selector = cx.entity().downgrade();
        let language = self.settings.read(cx).config().language;
        let search_label = if kind.is_dark() {
            language.search_dark_themes()
        } else {
            language.search_light_themes()
        };
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
            .id(format!("theme-search-container-{}", kind.index()))
            .relative()
            .flex_none()
            .size(px(26.))
            .child(
                div()
                    .id(format!("theme-search-{}", kind.index()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .rounded_md()
                    .text_color(rgb(self.colors.muted))
                    .cursor_pointer()
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .tooltip(tooltip_text(search_label, self.colors))
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        if settings.lua_manages_selector(kind, cx) {
                            return;
                        }
                        if settings.open_selector == Some(kind) {
                            settings.open_selector = None;
                            settings.clear_theme_preview(cx);
                        } else {
                            settings.open_selector = Some(kind);
                            settings.selector_search_input.update(cx, |input, cx| {
                                input.set_placeholder(search_label);
                                input.set_content("", cx);
                            });
                            settings
                                .selector_scroll_handle
                                .set_offset(point(px(0.), px(0.)));
                            settings
                                .selector_search_input
                                .read(cx)
                                .focus_handle()
                                .focus(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(icon_sized(IconName::Search, 14.)),
            )
            .when(self.open_selector == Some(kind), |element| {
                let mut dropdown = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    dropdown = dropdown.position(position);
                }
                element.child(
                    deferred(dropdown.child(self.render_selector_dropdown(kind, window, cx)))
                        .priority(2),
                )
            })
            .into_any_element()
    }

    fn render_theme_card(
        &self,
        kind: SelectorKind,
        name: &str,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let managed = self.lua_manages_selector(kind, cx);
        let theme = theme_catalog()
            .theme_by_name(name)
            .expect("catalog name must resolve to a theme");
        let colors = self.colors;
        let card_fill = theme.background;
        let card_edge = blend_rgb(card_fill, theme.foreground, 0.2);
        let sample_fill = blend_rgb(card_fill, theme.foreground, 0.055);
        let chip_fill = theme.palette[0];
        let chip_text = readable_on(chip_fill);
        let name_owned = name.to_owned();
        let hover_name = name_owned.clone();
        let selected_name = name_owned.clone();
        div()
            .id(format!("theme-card-{}-{name}", kind.index()))
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .w(px(THEME_CARD_WIDTH))
            .when(!managed, |element| element.cursor_pointer())
            .when(managed, |element| element.opacity(0.58))
            .on_hover(cx.listener(move |settings, hovered: &bool, _, cx| {
                if *hovered {
                    settings.preview_theme(kind, &hover_name, cx);
                }
            }))
            .on_click(cx.listener(move |settings, _, _, cx| {
                if settings.lua_manages_selector(kind, cx) {
                    return;
                }
                settings.clear_theme_preview(cx);
                settings.commit_selector_choice(kind, selected_name.clone(), cx);
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .w(px(THEME_CARD_WIDTH))
                    .h(px(THEME_CARD_HEIGHT))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                if selected {
                                    paint_continuous_rect(
                                        window,
                                        bounds,
                                        0.,
                                        THEME_CARD_RADIUS,
                                        colors.accent,
                                    );
                                    paint_continuous_rect(
                                        window,
                                        bounds,
                                        3.,
                                        THEME_CARD_RADIUS,
                                        colors.panel,
                                    );
                                }
                                paint_continuous_rect(
                                    window,
                                    bounds,
                                    THEME_CARD_BASE_INSET,
                                    THEME_CARD_RADIUS,
                                    card_edge,
                                );
                                paint_continuous_rect(
                                    window,
                                    bounds,
                                    THEME_CARD_BASE_INSET + 1.,
                                    THEME_CARD_RADIUS,
                                    card_fill,
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_between()
                            .absolute()
                            .top(px(THEME_CARD_BASE_INSET + 11.))
                            .bottom(px(THEME_CARD_BASE_INSET + 11.))
                            .left(px(THEME_CARD_BASE_INSET + 8.))
                            .right(px(THEME_CARD_BASE_INSET + 8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .h(px(42.))
                                    .px_1()
                                    .rounded_xl()
                                    .bg(rgb(sample_fill))
                                    .child(
                                        div()
                                            .text_size(px(26.))
                                            .text_color(rgb(theme.foreground))
                                            .child("Aa"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(34.))
                                            .h(px(29.))
                                            .rounded_md()
                                            .bg(rgb(chip_fill))
                                            .font_family("Menlo")
                                            .text_size(px(14.))
                                            .text_color(rgb(chip_text))
                                            .child("~/"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_end()
                                            .justify_center()
                                            .w(px(15.))
                                            .h(px(29.))
                                            .pb(px(3.))
                                            .rounded_md()
                                            .bg(rgb(theme.cursor))
                                            .child(
                                                div()
                                                    .w(px(7.))
                                                    .h(px(2.))
                                                    .bg(rgb(theme.cursor_text)),
                                            ),
                                    ),
                            )
                            .child(div().flex().items_center().children(
                                theme.palette[..8].iter().enumerate().map(|(index, color)| {
                                    div()
                                        .flex_none()
                                        .size(px(21.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgb(card_fill))
                                        .bg(rgb(*color))
                                        .when(index > 0, |element| element.ml(px(-5.)))
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_center()
                    .text_size(px(12.))
                    .text_color(rgb(if selected { colors.text } else { colors.muted }))
                    .when(selected, |element| {
                        element.font_weight(gpui::FontWeight::SEMIBOLD)
                    })
                    .child(name_owned),
            )
            .into_any_element()
    }

    fn render_theme_gallery(
        &self,
        kind: SelectorKind,
        label: &'static str,
        description: &'static str,
        selected: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let managed = self.lua_manages_selector(kind, cx);
        let scroll_handle = &self.theme_gallery_scroll_handles[kind.index()];
        let offset_x = -f32::from(scroll_handle.offset().x);
        let max_offset_x = f32::from(scroll_handle.max_offset().x);
        let viewport_width = match f32::from(scroll_handle.bounds().size.width) {
            width if width > 0. => width,
            _ => 720.,
        };
        let card_step = THEME_CARD_WIDTH + 12.;
        let preload = card_step * 2.;
        let visible_start = (offset_x - preload).max(0.);
        let visible_end = offset_x + viewport_width + preload;
        let default = if kind.is_dark() {
            DEFAULT_DARK_THEME
        } else {
            DEFAULT_LIGHT_THEME
        };
        let names = self.selector_names(kind);
        let has_default = names.iter().any(|name| name == default);
        let total_width = names.len() as f32 * card_step + if has_default { 13. } else { 0. } - 12.;
        let mut visible_items = Vec::new();
        let mut item_x = 0.;
        if has_default {
            if item_x + THEME_CARD_WIDTH >= visible_start && item_x <= visible_end {
                visible_items.push((
                    item_x,
                    self.render_theme_card(kind, default, selected == default, cx),
                ));
            }
            item_x += card_step;
            if item_x + 1. >= visible_start && item_x <= visible_end {
                visible_items.push((
                    item_x,
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(THEME_CARD_HEIGHT - 8.))
                        .mt_1()
                        .bg(rgb(self.colors.border))
                        .into_any_element(),
                ));
            }
            item_x += 13.;
        }
        for name in names.iter().filter(|name| *name != default) {
            if item_x + THEME_CARD_WIDTH >= visible_start && item_x <= visible_end {
                visible_items.push((
                    item_x,
                    self.render_theme_card(kind, name, name == selected, cx),
                ));
            }
            item_x += card_step;
        }
        let mut content = div()
            .flex()
            .flex_none()
            .items_start()
            .gap(px(12.))
            .w(px(total_width));
        if let Some((first_x, _)) = visible_items.first()
            && *first_x > 0.
        {
            content = content.child(div().flex_none().w(px(first_x - 12.)).h(px(1.)));
        }
        for (_, item) in visible_items {
            content = content.child(item);
        }
        let strip = div()
            .id(format!("theme-gallery-{}", kind.index()))
            .min_w_0()
            .w_full()
            .overflow_x_scroll()
            .track_scroll(scroll_handle)
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(|settings, hovered: &bool, _, cx| {
                if !*hovered {
                    settings.clear_theme_preview(cx);
                }
            }))
            .child(content);
        let panel_color = rgb(self.colors.panel);
        let mut strip_wrapper = div().relative().min_w_0().w_full().child(strip);
        if offset_x > 0.5 {
            strip_wrapper =
                strip_wrapper.child(div().absolute().left_0().top_0().bottom_0().w(px(28.)).bg(
                    linear_gradient(
                        90.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    ),
                ));
        }
        if offset_x < max_offset_x - 0.5 {
            strip_wrapper =
                strip_wrapper.child(div().absolute().right_0().top_0().bottom_0().w(px(28.)).bg(
                    linear_gradient(
                        270.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    ),
                ));
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            .w_full()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .child(
                        div()
                            .id(format!("theme-gallery-label-{}", kind.index()))
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .tooltip(tooltip_text(description, self.colors))
                            .child(if managed {
                                format!("{label} · init.lua")
                            } else {
                                label.to_owned()
                            }),
                    )
                    .child(self.render_theme_search(kind, window, cx)),
            )
            .child(strip_wrapper)
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
        let config_error = self.settings.read(cx).lua_config().error.clone();
        let lua_path = self
            .settings
            .read(cx)
            .lua_config()
            .exists
            .then(|| self.settings.read(cx).lua_config().path.clone());
        let system_is_dark = is_dark_appearance(window.appearance());
        // The settings window's own chrome follows the resolved *interface* theme, while the preview
        // swatch below follows the resolved *terminal* theme — so hovering an interface option repaints
        // the window, hovering a terminal option repaints only the swatch, and each preview shows
        // exactly what committing it would do.
        let interface_theme = self
            .settings
            .read(cx)
            .resolved_interface_theme(system_is_dark);
        let terminal_theme = self
            .settings
            .read(cx)
            .resolved_terminal_theme(system_is_dark);
        if !std::ptr::eq(self.colors_for_theme, interface_theme) {
            let colors = UiColors::from_theme(interface_theme);
            let style = settings_input_style(colors);
            self.selector_search_input
                .update(cx, |input, cx| input.set_style(style, cx));
            self.shell_program_input
                .update(cx, |input, cx| input.set_style(style, cx));
            self.shell_args_input
                .update(cx, |input, cx| input.set_style(style, cx));
            for input in self.profile_input_list() {
                input.update(cx, |input, cx| input.set_style(style, cx));
            }
            self.colors = colors;
            self.colors_for_theme = interface_theme;
        }

        let language = config.language;
        // Keep every input field's edit-menu labels in sync with the current language.
        self.selector_search_input
            .update(cx, |input, _| input.set_language(language));
        self.shell_program_input
            .update(cx, |input, _| input.set_language(language));
        self.shell_args_input
            .update(cx, |input, _| input.set_language(language));
        for input in self.profile_input_list() {
            input.update(cx, |input, _| input.set_language(language));
        }
        // Re-seed the profile fields whenever the selection has moved — including when it moved because
        // the selected profile was deleted elsewhere (reading the store here is what subscribes this
        // view to it, so that mutation repaints us and the stale id resolves to the built-in Default).
        self.sync_profile_inputs(cx);
        let language_control = self.render_language_control(language, cx);
        let bell_control = self.render_bell_control(config.bell_mode, window, cx);
        let mode_control = self.render_mode_control(config.theme_mode, cx);
        let interface_light_gallery = self.render_theme_gallery(
            SelectorKind::InterfaceLight,
            language.interface_light_theme_row(),
            language.interface_light_theme_description(),
            &config.interface_light_theme,
            window,
            cx,
        );
        let interface_dark_gallery = self.render_theme_gallery(
            SelectorKind::InterfaceDark,
            language.interface_dark_theme_row(),
            language.interface_dark_theme_description(),
            &config.interface_dark_theme,
            window,
            cx,
        );
        let terminal_theme_source_control =
            self.render_terminal_theme_source_control(config.terminal_theme_source, cx);
        // Only built when the terminal uses its own pair: under `FollowInterface` these two rows are
        // omitted entirely rather than shown disabled, so the panel states what is actually in effect.
        let terminal_theme_galleries =
            (config.terminal_theme_source == TerminalThemeSource::Custom).then(|| {
                (
                    self.render_theme_gallery(
                        SelectorKind::TerminalLight,
                        language.terminal_light_theme_row(),
                        language.terminal_light_theme_description(),
                        &config.terminal_light_theme,
                        window,
                        cx,
                    ),
                    self.render_theme_gallery(
                        SelectorKind::TerminalDark,
                        language.terminal_dark_theme_row(),
                        language.terminal_dark_theme_description(),
                        &config.terminal_dark_theme,
                        window,
                        cx,
                    ),
                )
            });
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
        // The Theme section, assembled here rather than inline because two of its rows are conditional
        // and the section list sits several levels deep in the element tree.
        //
        // Order tells the story of the layering: the interface pair first (it decides the chrome, and
        // the terminal follows it by default), then the terminal's own source and — only when it has
        // one — its pair.
        let mut theme_rows = vec![
            settings_row(
                language.theme_row(),
                language.theme_description(),
                mode_control,
                self.colors,
            ),
            interface_light_gallery,
            interface_dark_gallery,
            settings_row(
                language.terminal_theme_source_row(),
                language.terminal_theme_source_description(),
                terminal_theme_source_control,
                self.colors,
            ),
        ];
        if let Some((terminal_light_gallery, terminal_dark_gallery)) = terminal_theme_galleries {
            theme_rows.push(terminal_light_gallery);
            theme_rows.push(terminal_dark_gallery);
        }
        theme_rows.push(settings_row(
            language.minimum_contrast_row(),
            language.minimum_contrast_description(),
            minimum_contrast_control,
            self.colors,
        ));
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
        let copy_on_select_control = self.render_copy_on_select_control(config.copy_on_select, cx);
        let scrollback_control = self.render_scrollback_control(config.scrollback_lines, cx);
        let shell_program_control =
            self.render_shell_input_control("settings-shell-program", &self.shell_program_input);
        let shell_args_control =
            self.render_shell_input_control("settings-shell-args", &self.shell_args_input);
        let shell_integration_path_control = self.render_shell_integration_path_control(
            // Read from the Default profile, which owns this field — `config` keeps only the
            // downgrade mirror, so reading it would show a stale value after an edit elsewhere.
            self.profile_store
                .read(cx)
                .default_profile()
                .shell_integration_path,
            cx,
        );
        let terminal_preview = self.render_terminal_preview(&config, terminal_theme);

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
                    .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::SEMIBOLD).child(language.settings_title()))
                    .when_some(lua_path, |element, path| {
                        element.child(
                            div()
                                .id("settings-open-lua-config")
                                .ml_auto()
                                .cursor_pointer()
                                .text_color(rgb(self.colors.accent))
                                .hover(|element| element.text_color(rgb(self.colors.text)))
                                .on_click(move |_, _, _| {
                                    #[cfg(target_os = "macos")]
                                    let opener = "open";
                                    #[cfg(not(target_os = "macos"))]
                                    let opener = "xdg-open";
                                    if let Err(error) = std::process::Command::new(opener).arg(&path).spawn() {
                                        eprintln!("failed to open {}: {error}", path.display());
                                    }
                                })
                                .child(match language {
                                    Language::English => "Open init.lua",
                                    Language::SimplifiedChinese => "打开 init.lua",
                                }),
                        )
                    }),
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
                                    SettingsSection::Profiles => (
                                        "settings-sidebar-profiles",
                                        language.profiles_sidebar(),
                                        IconName::Terminal,
                                    ),
                                    SettingsSection::Keybindings => (
                                        "settings-sidebar-keybindings",
                                        language.keybindings_sidebar(),
                                        IconName::Keyboard,
                                    ),
                                    SettingsSection::Advanced => (
                                        "settings-sidebar-advanced",
                                        language.advanced_sidebar(),
                                        IconName::Sliders,
                                    ),
                                    SettingsSection::About => (
                                        "settings-sidebar-about",
                                        language.about_sidebar(),
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
                                                    .when_some(config_error, |element, error| {
                                                        element.child(
                                                            div()
                                                                .rounded_lg()
                                                                .p_3()
                                                                .bg(rgb(self.colors.panel_alt))
                                                                .text_color(rgb(self.colors.accent))
                                                                .child(format!("init.lua: {error}")),
                                                        )
                                                    })
                                                    // The "About" panel has its own branded header
                                                    // (logo + name), so it skips the shared section
                                                    // title to avoid a redundant "About Eggie" line.
                                                    .when(
                                                        self.selected_section != SettingsSection::About,
                                                        |element| {
                                                            element.child(
                                                                div()
                                                                    .text_size(px(18.))
                                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                                    .child(match self.selected_section {
                                                                        SettingsSection::General => language.general_section(),
                                                                        SettingsSection::Appearance => language.appearance_section(),
                                                                        SettingsSection::Profiles => language.profiles_section(),
                                                                        SettingsSection::Keybindings => language.keybindings_section(),
                                                                        SettingsSection::Advanced => language.advanced_section(),
                                                                        SettingsSection::About => language.about_eggie(),
                                                                    }),
                                                            )
                                                        },
                                                    )
                                                    .children(match self.selected_section {
                                                        SettingsSection::General => vec![
                                                            settings_section(
                                                                // The pane title above already reads
                                                                // "General"; a caption here would
                                                                // repeat it on the very next line.
                                                                None,
                                                                vec![settings_row(
                                                                    language.language_row(),
                                                                    language.language_description(),
                                                                    language_control,
                                                                    self.colors,
                                                                )],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                Some(language.terminal_behavior_section()),
                                                                vec![settings_row(
                                                                    language.scrollback_lines_row(),
                                                                    language.scrollback_lines_description(),
                                                                    scrollback_control,
                                                                    self.colors,
                                                                )],
                                                                self.colors,
                                                            ),
                                                        ],
                                                        SettingsSection::Appearance => vec![
                                                            settings_section(
                                                                Some(language.theme_section()),
                                                                theme_rows,
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                Some(language.terminal_text_section()),
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
                                                                Some(language.cursor_section()),
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
                                                                Some(language.terminal_layout_section()),
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
                                                                Some(language.font_metrics_section()),
                                                                metric_rows,
                                                                self.colors,
                                                            ),
                                                        ],
                                                        SettingsSection::Advanced => vec![
                                                            settings_section(
                                                                Some(language.progress_indicators_section()),
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
                                                                Some(language.terminal_behavior_section()),
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
                                                                Some(language.terminal_security_section()),
                                                                vec![settings_row(
                                                                    language.osc_clipboard_read_row(),
                                                                    language.osc_clipboard_read_description(),
                                                                    osc_clipboard_read_control,
                                                                    self.colors,
                                                                )],
                                                                self.colors,
                                                            ),
                                                            settings_section(
                                                                Some(language.shell_section()),
                                                                vec![
                                                                    settings_row(
                                                                        language.shell_program_row(),
                                                                        language.shell_program_description(),
                                                                        shell_program_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.shell_args_row(),
                                                                        language.shell_args_description(),
                                                                        shell_args_control,
                                                                        self.colors,
                                                                    ),
                                                                    settings_row(
                                                                        language.shell_integration_path_row(),
                                                                        language.shell_integration_path_description(),
                                                                        shell_integration_path_control,
                                                                        self.colors,
                                                                    ),
                                                                ],
                                                                self.colors,
                                                            ),
                                                        ],
                                                        SettingsSection::Keybindings => {
                                                            self.render_keybindings_section(language, cx)
                                                        }
                                                        SettingsSection::Profiles => {
                                                            self.render_profiles_section(language, window, cx)
                                                        }
                                                        SettingsSection::About => {
                                                            self.render_about_section(language, cx)
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

fn blend_rgb(a: u32, b: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let a = ((a >> shift) & 0xff) as f32;
        let b = ((b >> shift) & 0xff) as f32;
        (a + (b - a) * amount).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn readable_on(color: u32) -> u32 {
    let red = (color >> 16) & 0xff;
    let green = (color >> 8) & 0xff;
    let blue = color & 0xff;
    if (red * 299 + green * 587 + blue * 114) / 1000 < 140 {
        0xffffff
    } else {
        0x171c20
    }
}

/// A settings section: an optional caption above a rounded card holding the rows.
///
/// The caption sits *outside* the card, matching how macOS and iOS Settings group their rows. Pass
/// `None` when the pane title already says the same thing — the 18px pane title is drawn right above
/// the first section, so repeating it would put two identical lines next to each other.
fn settings_section(
    title: Option<&'static str>,
    rows: Vec<AnyElement>,
    colors: UiColors,
) -> AnyElement {
    let card = settings_card(rows, colors);
    let Some(title) = title else {
        return card;
    };
    div()
        .flex()
        .flex_none()
        .flex_col()
        // Tighter than the `gap_5` between sections, so a caption reads as belonging to the card
        // below it rather than floating between two cards.
        .gap_2()
        .child(
            div()
                // Aligns the caption with the card's rounded corner instead of its outer edge.
                .px_1()
                .text_size(px(12.))
                .text_color(rgb(colors.muted))
                .child(title),
        )
        .child(card)
        .into_any_element()
}

/// The rounded card every settings section is built from: one border, one background, and a
/// separator between rows but never after the last one.
///
/// `overflow_hidden` is load-bearing — a row's background is square, so without it the first and
/// last rows paint over the card's rounded corners.
fn settings_card(rows: Vec<AnyElement>, colors: UiColors) -> AnyElement {
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
                .bg(rgb(colors.panel)),
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
    use super::{SelectorKind, selector_matches};

    #[test]
    fn selector_indices_are_a_bijection_onto_the_bounds_array() {
        // `index()`, `ALL`, and the `selector_bounds` array length are three hand-maintained facts that
        // must agree. They have drifted before (see CAPABILITIES_TODO), and the failure mode is a panic
        // when the dropdown reads or writes its bounds slot — invisible until that selector is opened.
        let mut seen = [false; SelectorKind::ALL.len()];
        for kind in SelectorKind::ALL {
            let index = kind.index();
            assert!(
                index < seen.len(),
                "{kind:?} indexes {index}, past the end of selector_bounds ({})",
                seen.len()
            );
            assert!(!seen[index], "{kind:?} reuses index {index}");
            seen[index] = true;
        }
        assert!(
            seen.into_iter().all(|hit| hit),
            "every bounds slot must be claimed by exactly one selector"
        );
    }

    #[test]
    fn every_theme_selector_belongs_to_exactly_one_half() {
        // Each theme slot offers one half of the catalog, so `is_dark` has to partition them. A font
        // selector is not a theme selector and never previews.
        for kind in SelectorKind::ALL {
            if kind.is_font() {
                assert!(
                    !kind.is_theme(),
                    "{kind:?} cannot be both a font and a theme selector"
                );
            } else {
                assert!(kind.is_theme(), "{kind:?} must be a theme selector");
            }
        }
        assert!(SelectorKind::InterfaceDark.is_dark());
        assert!(!SelectorKind::InterfaceLight.is_dark());
        assert!(SelectorKind::TerminalDark.is_dark());
        assert!(!SelectorKind::TerminalLight.is_dark());
        assert!(SelectorKind::ProfileTerminalDark.is_dark());
        assert!(!SelectorKind::ProfileTerminalLight.is_dark());
    }

    #[test]
    fn only_theme_selectors_have_a_preview_slot() {
        let profile_id = eggie_domain::ProfileId::new_v4();
        for kind in SelectorKind::ALL {
            assert_eq!(
                kind.preview_slot(profile_id).is_some(),
                kind.is_theme(),
                "{kind:?}: a preview slot exists exactly for theme selectors"
            );
        }
    }

    #[test]
    fn selector_search_is_case_insensitive_and_supports_partial_matches() {
        assert!(selector_matches("Maple Mono NF CN", "maple"));
        assert!(selector_matches("Builtin Dark", "IN DA"));
        assert!(!selector_matches("Menlo", "mono"));
        assert!(selector_matches("Menlo", ""));
    }
}

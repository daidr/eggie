use eggie_ui_kit::continuous_corners::div;
mod about;
mod controls;
mod editing;
mod gallery;
mod keybindings;
mod profiles;
mod search;
mod sections;
mod selector;
mod ssh;
use std::sync::{Arc, OnceLock};

use crate::{
    app::tooltip_text,
    continuous_corners::paint_continuous_rect,
    icons::{IconName, icon, icon_sized},
    project_store::ProjectStore,
    settings::{
        AppSettings, BellMode, CursorBlink, CursorShapeSetting, DEFAULT_DARK_THEME,
        DEFAULT_LIGHT_THEME, Language, MAX_FONT_SIZE, MAX_MINIMUM_CONTRAST,
        MAX_PROGRESS_TIMEOUT_SECS, MAX_SCROLLBACK_LINES, MAX_TERMINAL_PADDING, MIN_FONT_SIZE,
        MIN_MINIMUM_CONTRAST, MIN_PROGRESS_TIMEOUT_SECS, MIN_SCROLLBACK_LINES,
        MIN_TERMINAL_PADDING, PastePreviewMode, SCROLLBACK_LINES_STEP, SettingsStore,
        SidebarLayout, TerminalExitBehavior, TerminalTheme, TerminalThemeSource, ThemeMode,
        UiColors, UpdateChannel, theme_catalog,
    },
    text_input::{TextInput, TextInputEvent, TextInputStyle},
};
use eggie_update::UpdateState;
use gpui::{
    Anchor, AnyElement, App, Bounds, Context, Entity, Image, ImageFormat, MouseButton, Pixels,
    ScrollHandle, SharedString, TitlebarOptions, Window, WindowBounds, WindowControlArea,
    WindowOptions, anchored, canvas, deferred, img, linear_color_stop, linear_gradient, point,
    prelude::*, px, rgb, size,
};

#[cfg(not(target_os = "macos"))]
use gpui::font;

/// Destination for the Help menu's documentation item.
// TODO: point at the real Eggie docs/repo once one exists (Cargo.toml has no `repository`).

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
    SshHost,
}

impl SelectorKind {
    /// Every variant, in `index()` order. A test pins that `index()` is a bijection onto
    /// `0..ALL.len()` and that `selector_bounds` is exactly this long — both are hand-maintained, and
    /// an out-of-range `index()` panics when the dropdown reads or writes its bounds slot.
    const ALL: [Self; 12] = [
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
        Self::SshHost,
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
            Self::SshHost => 11,
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
        !self.is_font() && !matches!(self, Self::SshHost)
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
            | Self::ProfileFont
            | Self::SshHost => None,
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
pub(crate) fn open_settings_window(
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
            buffering: gpui::WindowBuffering::Double,
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
    macos_monospace_font_names()
}

#[cfg(target_os = "macos")]
fn macos_monospace_font_names() -> Vec<String> {
    use core_foundation::{
        array::CFArray, base::TCFType, dictionary::CFDictionary, number::CFNumber, set::CFSet,
        string::CFString,
    };
    use core_text::font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCreateMatchingFontDescriptors, SymbolicTraitAccessors,
        TraitAccessors, kCTFontMonoSpaceTrait, kCTFontSymbolicTrait, kCTFontTraitsAttribute,
        new_from_attributes,
    };
    use std::collections::HashSet;

    // Ask CoreText for monospace faces directly instead of materializing every installed face.
    let descriptors: Option<CFArray<CTFontDescriptor>> = unsafe {
        let traits_key = CFString::wrap_under_get_rule(kCTFontTraitsAttribute);
        let symbolic_key = CFString::wrap_under_get_rule(kCTFontSymbolicTrait);
        let traits = CFDictionary::from_CFType_pairs(&[(
            symbolic_key,
            CFNumber::from(kCTFontMonoSpaceTrait as i64).as_CFType(),
        )]);
        let attributes =
            CFDictionary::from_CFType_pairs(&[(traits_key.clone(), traits.as_CFType())]);
        let query = new_from_attributes(&attributes);
        let mandatory = CFSet::from_slice(&[traits_key]);
        let array_ref = CTFontDescriptorCreateMatchingFontDescriptors(
            query.as_concrete_TypeRef(),
            mandatory.as_concrete_TypeRef(),
        );
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
    ssh_host_names: Vec<String>,
    ssh_hosts_error: Option<String>,
    ssh_hosts_task: Option<gpui::Task<()>>,
    ssh_config_expanded: bool,
    ssh_config_task: Option<gpui::Task<()>>,
    ssh_config_result: Option<(
        eggie_domain::ProfileId,
        eggie_domain::RemoteTarget,
        Result<eggie_protocol::SshEffectiveConfig, String>,
    )>,
    colors: UiColors,
    colors_for_theme: &'static TerminalTheme,
    open_selector: Option<SelectorKind>,
    selector_search_input: Entity<TextInput>,
    settings_search_input: Entity<TextInput>,
    settings_search_selected: usize,
    _settings_search_navigation: gpui::Subscription,
    search_target: std::rc::Rc<std::cell::RefCell<Option<&'static str>>>,
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
        let settings_search_input = cx.new(|cx| TextInput::new(window, cx, input_style));
        cx.subscribe_in(&settings_search_input, window, Self::on_settings_search)
            .detach();
        let weak = cx.weak_entity();
        let settings_search_navigation = cx.intercept_keystrokes(move |event, window, cx| {
            if weak
                .update(cx, |settings, cx| {
                    settings.handle_ssh_host_key(&event.keystroke, window, cx)
                        || settings.navigate_settings_search(event.keystroke.clone(), window, cx)
                })
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        });
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
            ssh_host_names: Vec::new(),
            ssh_hosts_error: None,
            ssh_hosts_task: None,
            ssh_config_expanded: false,
            ssh_config_task: None,
            ssh_config_result: None,
            colors,
            colors_for_theme: theme,
            open_selector: None,
            selector_search_input,
            settings_search_input,
            settings_search_selected: 0,
            _settings_search_navigation: settings_search_navigation,
            search_target: Default::default(),
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ssh_host = self.open_selector == Some(SelectorKind::SshHost);
        match event {
            TextInputEvent::Changed => {
                self.selector_scroll_handle
                    .set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }
            TextInputEvent::Confirm | TextInputEvent::ConfirmReverse => {
                self.apply_first_selector_match(cx);
                if ssh_host && self.open_selector.is_none() {
                    self.profile_inputs
                        .remote_host
                        .read(cx)
                        .focus_handle()
                        .focus(window, cx);
                }
            }
            TextInputEvent::Cancel => {
                self.open_selector = None;
                self.clear_theme_preview(cx);
                if ssh_host {
                    self.profile_inputs
                        .remote_host
                        .read(cx)
                        .focus_handle()
                        .focus(window, cx);
                }
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
            SelectorKind::SshHost => &self.ssh_host_names,
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
        if kind == SelectorKind::SshHost {
            self.profile_inputs
                .remote_host
                .update(cx, |input, cx| input.set_content(choice, cx));
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
                    | SelectorKind::ProfileFont
                    | SelectorKind::SshHost => {}
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

    // Every profile input, in field order. The single place that enumerates them, so language sync,
    // subscription installation, and seeding cannot fall out of step with the struct.
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
        // Settings chrome follows the interface layer, including a live interface hover preview.
        let interface_theme = self
            .settings
            .read(cx)
            .resolved_interface_theme(system_is_dark);
        if !std::ptr::eq(self.colors_for_theme, interface_theme) {
            let colors = UiColors::from_theme(interface_theme);
            let style = settings_input_style(colors);
            self.selector_search_input
                .update(cx, |input, cx| input.set_style(style, cx));
            self.settings_search_input
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
        self.settings_search_input.update(cx, |input, _| {
            input.set_language(language);
            input.set_placeholder(if language == Language::SimplifiedChinese {
                "搜索设置"
            } else {
                "Search settings"
            });
        });
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
        let searching = !self
            .settings_search_input
            .read(cx)
            .content()
            .trim()
            .is_empty();
        let section_content = if searching {
            self.render_settings_results(language, cx)
        } else {
            self.render_current_section(&config, window, cx)
        };
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
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(language.settings_title()),
                    )
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
                                    if let Err(error) =
                                        std::process::Command::new(opener).arg(&path).spawn()
                                    {
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
                            .child(
                                div()
                                    .id("settings-search")
                                    .h(px(30.))
                                    .px_2()
                                    .py_1()
                                    .mb_2()
                                    .rounded_md()
                                    .bg(rgb(self.colors.panel_alt))
                                    .child(self.settings_search_input.clone()),
                            )
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
                                        element
                                            .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                                    })
                                    .on_click(cx.listener(move |settings, _, _, cx| {
                                        if settings.recording.is_some() {
                                            settings.cancel_recording(cx);
                                        }
                                        settings.bell_dropdown_open = false;
                                        settings.settings_search_input.update(cx, |input, cx| {
                                            input.set_content(String::new(), cx)
                                        });
                                        settings.search_target.borrow_mut().take();
                                        settings.selected_section = section;
                                        cx.notify();
                                    }))
                                    .child(icon(icon_name))
                                    .child(label)
                            })),
                    )
                    .child(
                        div().flex().flex_1().min_w_0().min_h_0().flex_col().child(
                            div().relative().flex_1().min_h_0().child(
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
                                                !searching
                                                    && self.selected_section
                                                        != SettingsSection::About,
                                                |element| {
                                                    element.child(
                                                        div()
                                                            .text_size(px(18.))
                                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                                            .child(match self.selected_section {
                                                                SettingsSection::General => {
                                                                    language.general_section()
                                                                }
                                                                SettingsSection::Appearance => {
                                                                    language.appearance_section()
                                                                }
                                                                SettingsSection::Profiles => {
                                                                    language.profiles_section()
                                                                }
                                                                SettingsSection::Keybindings => {
                                                                    language.keybindings_section()
                                                                }
                                                                SettingsSection::Advanced => {
                                                                    language.advanced_section()
                                                                }
                                                                SettingsSection::About => {
                                                                    language.about_eggie()
                                                                }
                                                            }),
                                                    )
                                                },
                                            )
                                            .children(section_content),
                                    ),
                            ),
                        ),
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
    description: Option<&'static str>,
    control: AnyElement,
    colors: UiColors,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_6()
        .min_h(px(if description.is_some() { 64. } else { 52. }))
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
                .when_some(description, |element, description| {
                    element.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(description),
                    )
                }),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

use eggie_ui_kit::is_dark_appearance;

#[cfg(test)]
mod tests;

fn selector_matches(item: &str, query: &str) -> bool {
    query.is_empty() || item.to_lowercase().contains(&query.to_lowercase())
}

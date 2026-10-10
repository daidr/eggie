use eggie_ui_kit::is_dark_appearance;
mod view_state;
use view_state::*;
pub(crate) use view_state::{NewWindowInit, NotificationRoutes, tooltip_text};
mod native_alert;
use native_alert::*;
mod chrome;
mod hover;
mod layout;
mod palette;
mod projects;
mod right_sidebar;
mod search;
mod session_feed;
mod sidebar;
mod startup;
mod state;
mod tabs;
mod terminal_io;
mod workspace_controller;
use state::*;
mod helpers;
use crate::actions::{
    ClearScreen, CloseTab, CloseWindow, CommandPalette, FindNext, FindPrevious, FontDecrease,
    FontIncrease, FontReset, JumpNextPrompt, JumpPrevPrompt, MinimizeWindow, NewTab,
    NewTabWithProfile, NextTab, PageDown, PageUp, PrevTab, ScrollBottom, ScrollTop, SplitDown,
    SplitLeft, SplitRight, SplitUp, TerminalCopy, TerminalFind, TerminalPaste, TerminalSelectAll,
    ToggleFullScreen, ToggleLeftSidebar, ToggleRightSidebar, ZoomWindow,
};
use crate::command_history::{CommandStatus, format_relative_time};
use crate::command_palette::{PaletteEntry, command_matches, palette_entries};
use crate::icons::{IconName, icon, icon_sized};
use crate::input_latency::InputLatencyTracker;
use crate::keybindings::LuaAction;
use crate::native_menu::{
    NativeProcessMenuCommand, NativeProfileMenuCommand, NativeProjectMenuCommand,
    NativeSnippetMenuCommand, NativeTabMenuCommand, prepare_process_menu, prepare_profile_menu,
    prepare_project_menu, prepare_snippet_menu, prepare_tab_menu,
};
use crate::notes_store::NotesStore;
use crate::popup_window::{PopupPlacement, PopupWindow, PopupWindowOptions};
use crate::process_icons::{self, ProcessIcon};
use crate::profile_store::ProfileStore;
use crate::project_drag::{self, DraggedProject, ProjectDragState, ProjectDropTarget};
use crate::project_store::ProjectStore;
use crate::settings::{
    CursorBlink, CursorShapeSetting, DEFAULT_FONT_SIZE, Language, OpenWith, SettingsStore,
    TerminalTheme, UiColors,
};
use crate::snippet_store::{Snippet, SnippetStore};
use crate::terminal_renderer::{
    MetalTerminalRenderer, PixelStore, TerminalImageData, TerminalImeState, TerminalInputContext,
    TerminalPoint, TerminalRenderOptions, TerminalSearchHighlights, TerminalSelection,
    TerminalTextureKey, terminal_background, terminal_cell_metrics,
};
use crate::text_input::{TextInput, TextInputEvent, TextInputStyle};
use crate::workspace::{SidebarRestoreState, WindowProjectView, WorkspaceRestoreState};
use eggie_domain::{
    Direction, GroupId, ItemId, ItemKind, LayoutNode, Profile, Project, ProjectId, SessionId,
    SplitAxis, SplitId, TabGroup, TabItem, WindowId,
};
use eggie_ipc::{DaemonClient, DaemonConnection, DaemonInputSender, TerminalImageFrame};
use eggie_protocol::{
    ClientRequest, DaemonResponse, ListeningPort, ProcessInfo, SessionInspection, SessionStatus,
    SessionSummary, TERMINAL_SCROLL_DELTA_SCALE, TerminalAppearance, TerminalAttentionRequest,
    TerminalClipboardContent, TerminalClipboardSelection, TerminalCursorShape,
    TerminalImageDescriptor, TerminalModifiers, TerminalMouseAction, TerminalMouseButton,
    TerminalMouseEvent, TerminalMouseTracking, TerminalOscEventPayload, TerminalProgramState,
    TerminalProgramStatus, TerminalProgress, TerminalProgressState, TerminalProgressTimeouts,
    TerminalProgressUpdate, TerminalScrollCommand, TerminalScrollDelta, TerminalScrollEvent,
    TerminalScrollPhase, TerminalScrollUnit, TerminalSearchDirection, TerminalSearchRequest,
    TerminalSearchResult, TerminalSelectionKind, TerminalShellIntegrationState, TerminalSize,
    TerminalSnapshot,
};
use gpui::{
    Anchor, Animation, AnimationExt, AnyElement, App, Bounds, ClipboardEntry, ClipboardItem,
    ClipboardString, Context, Div, DragMoveEvent, Entity, FocusHandle, Image, ImageFormat,
    KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathBuilder, PathPromptOptions, Pixels, Point, PromptLevel, Role, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Stateful, Subscription, SystemNotification,
    SystemNotificationAction, TitlebarOptions, TouchPhase, WeakEntity, Window, WindowBounds,
    WindowControlArea, WindowOptions, anchored, canvas, deferred, div, img, linear_color_stop,
    linear_gradient, point, prelude::*, px, quad, relative, rgb, rgba, size,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[cfg(target_os = "macos")]
use block::ConcreteBlock;
#[cfg(target_os = "macos")]
use cocoa::{
    base::{id, nil},
    foundation::{NSInteger, NSPoint, NSRect, NSSize, NSString},
};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

mod terminal_input;
use terminal_input::*;

const TAB_BAR_HEIGHT: f32 = 36.;
const TITLEBAR_HEIGHT: f32 = TAB_BAR_HEIGHT;
const TAB_MIN_WIDTH: f32 = 140.;
const TAB_MAX_WIDTH: f32 = 220.;
const TAB_BAR_PADDING: f32 = 5.;
const TAB_GAP: f32 = 5.;
// The card artwork includes 2px of transparent space, so 1px keeps its visible inset even.
const TAB_ICON_LEADING_PADDING: f32 = 1.;
const TAB_TITLE_FONT_SIZE: f32 = 11.;
const TAB_CLOSE_ICON_SIZE: f32 = 10.;
/// Diameter of the profile badge dot on a tab. Small enough to read as a marker beside the title
/// rather than as another control, and it only appears when the profile defines a color.
const TAB_BADGE_SIZE: f32 = 6.;
const TAB_DROP_INDICATOR_WIDTH: f32 = 2.;
const TAB_DROP_INDICATOR_OFFSET: f32 = (TAB_GAP + TAB_DROP_INDICATOR_WIDTH) / 2.;
const TOP_BAR_ACTION_SLOT_WIDTH: f32 = 28.;
const SIDEBAR_CARD_SIZE: f32 = 18.;
const PROJECT_CARD_SIZE: f32 = 21.;
const PROJECT_MAX_ICON_COUNT: usize = 3;
const PROJECT_ICON_TRANSITION: Duration = Duration::from_millis(340);
const SIDEBAR_SESSION_ROW_HEIGHT: f32 = 24.;
const SIDEBAR_STATUS_ROW_HEIGHT: f32 = 42.;
const SIDEBAR_STATUS_TRANSITION: Duration = Duration::from_millis(260);
const PROJECT_POPOVER_LEAVE_DELAY: Duration = Duration::from_millis(220);
const PROJECT_POPOVER_ROW_HEIGHT: f32 = 40.;
const SIDEBAR_PREVIEW_HOVER_DELAY: Duration = Duration::from_millis(850);
const SIDEBAR_PREVIEW_LEAVE_DELAY: Duration = Duration::from_millis(180);

pub struct EggieApp {
    workspace_state: WorkspaceState,
    sidebar: SidebarState,
    session_feed: SessionFeedState,
    client: DaemonClient,
    input_sender: DaemonInputSender,
    input_latency: InputLatencyTracker,
    settings: Entity<SettingsStore>,
    updates: Entity<crate::update_controller::UpdateController>,
    colors: UiColors,
    interface_theme: &'static TerminalTheme,
    terminal_theme: &'static TerminalTheme,
    use_dark: bool,
    global_appearance: TerminalAppearance,
    session_appearances: HashMap<SessionId, TerminalAppearance>,
    #[allow(clippy::type_complexity)]
    applied_appearance_inputs: Option<(
        TerminalAppearance,
        u64,
        u128,
        bool,
        Option<(crate::settings::PreviewSlot, usize)>,
    )>,
    terminal_padding_x: f32,
    terminal_padding_y: f32,
    terminal_minimum_contrast: f32,
    terminal_metric_adjustments: crate::settings::ResolvedMetricAdjustments,
    terminal_font_features: std::sync::Arc<[crate::settings::FontFeature]>,
    terminal_ligatures: bool,
    terminal_shaping_break_cursor: bool,
    terminal_font_variations: std::sync::Arc<[crate::settings::FontVariation]>,
    terminal_font_thicken: Option<u8>,
    terminal_font_codepoint_map: std::sync::Arc<[crate::settings::CodepointMapEntry]>,
    applied_config: Option<crate::settings::AppSettings>,
    terminal_renderer: MetalTerminalRenderer,
    terminal_images_in_flight: HashSet<TerminalImageStreamKey>,
    terminal_frame_scheduled: bool,
    progress_frame_scheduled: bool,
    progress_animation_scheduled: bool,
    progress_animation_epoch: Instant,
    bell_flash: HashMap<SessionId, Instant>,
    bell_flash_scheduled: bool,
    cursor_blink_epoch: Instant,
    cursor_blink_scheduled: bool,
    progress_timeouts: TerminalProgressTimeouts,
    window_id: WindowId,
    project_store: Entity<ProjectStore>,
    // TODO(profiles step 2): read by spawn-config resolution and the tab badge.
    #[allow(dead_code)]
    profile_store: Entity<ProfileStore>,
    snippet_store: Entity<SnippetStore>,
    notes_store: Entity<NotesStore>,
    detached_popover_open: bool,
    detached_toggle_bounds: Option<Bounds<Pixels>>,
    update_popover_open: bool,
    update_toggle_bounds: Option<Bounds<Pixels>>,
    open_with_dropdown: Option<DirectoryField>,
    open_with_dropdown_bounds: HashMap<DirectoryField, Bounds<Pixels>>,
    terminal_sizes: HashMap<SessionId, TerminalSize>,
    terminal_resize_in_flight: HashMap<SessionId, TerminalSize>,
    terminal_viewports: HashMap<GroupId, TerminalViewport>,
    terminal_selection_drag: Option<TerminalSelectionDrag>,
    terminal_search: Option<TerminalSearchUi>,
    command_palette: Option<CommandPaletteUi>,
    hyperlink_mouse_down: bool,
    terminal_hovered_link: Option<(SessionId, eggie_protocol::TerminalLinkRange)>,
    terminal_has_focus: bool,
    terminal_focused_session: Option<SessionId>,
    terminal_ime_states: HashMap<SessionId, TerminalImeState>,
    right_tab: RightTab,
    command_history_collapsed: bool,
    snippets_collapsed: bool,
    notes_editor: Option<NotesEditor>,
    left_sidebar_width: f32,
    right_sidebar_width: f32,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
    resizing_sidebar: Option<SidebarEdge>,
    resizing_split: Option<SplitId>,
    // Shared with the split container's prepaint listener, which records the on-screen bounds of
    // each split so `resize_split` can turn a mouse position into a ratio. This MUST be an `Rc` so
    // the render-time `.clone()` shares the same map; cloning a bare `RefCell` would copy the map
    // and the listener would write into a throwaway the resize handler never sees.
    split_bounds: Rc<RefCell<HashMap<SplitId, Bounds<Pixels>>>>,
    dragging_scrollbar: Option<ScrollbarDrag>,
    // On-screen bounds of each group's scrollbar track, recorded by the track's prepaint listener.
    // The drag handler maps a pointer Y into an absolute scroll offset against these bounds, and the
    // renderer sizes/places the thumb from them. Same `Rc<RefCell>` sharing rationale as
    // `split_bounds`: cloning a bare `RefCell` at render time would detach the listener's writes.
    scrollbar_bounds: Rc<RefCell<HashMap<GroupId, Bounds<Pixels>>>>,
    moving_window: bool,
    tab_drop_target: Option<TabDropTarget>,
    right_sidebar_scroll_handle: ScrollHandle,
    tab_bar_scroll_handles: RefCell<HashMap<GroupId, ScrollHandle>>,
    focus_handle: FocusHandle,
    notification_routes: NotificationRoutes,
    allow_osc_clipboard_read: bool,
    detect_urls: bool,
    copy_on_select: bool,
    cursor_shape: CursorShapeSetting,
    scrollback_limit: usize,
    cursor_blink: CursorBlink,
    language: Language,
    closing: bool,
    poll_error: Option<String>,
}

/// A session produced by window seeding, plus the profile it was launched from.
///
/// `profile` is `Some` only for sessions this window just created — an adopted one (claimed back from
/// the daemon after a restart, or inherited from a closed window) was launched by an earlier run, and
/// the restored layout already records whatever profile its tab carried. Keeping the two apart is what
/// lets a freshly seeded tab render its profile's appearance instead of the global one.

struct SeededSession {
    summary: SessionSummary,
    profile: Option<eggie_domain::ProfileId>,
}

impl SeededSession {
    fn launched(summary: SessionSummary, profile: eggie_domain::ProfileId) -> Self {
        Self {
            summary,
            profile: Some(profile),
        }
    }

    fn adopted(summary: SessionSummary) -> Self {
        Self {
            summary,
            profile: None,
        }
    }
}

impl gpui::Render for EggieApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read the *resolved* themes (persisted pair, or the settings window's live hover preview if one
        // is active) so a hovered theme lights up the real chrome and terminals here, not just the
        // settings preview. The two layers resolve independently: the interface drives chrome, the
        // terminal drives cells, and they coincide only while the terminal follows the interface.
        let system_is_dark = is_dark_appearance(window.appearance());
        let settings = self.settings.read(cx);
        self.interface_theme = settings.resolved_interface_theme(system_is_dark);
        self.terminal_theme = settings.resolved_terminal_theme(system_is_dark);
        self.use_dark = settings.config().use_dark(system_is_dark);
        let theme_preview_key = settings.theme_preview_key();
        self.global_appearance = self.terminal_theme.appearance();
        // Push each session its *own* resolved palette. The daemon needs the real colors (it resolves
        // palette-dependent output such as minimum-contrast and selection), and with per-profile themes
        // a single window-wide broadcast would paint every terminal the active window's colors.
        //
        // Gated on something actually having changed. The reconcile is O(sessions × tabs) — each
        // session's profile is found by walking the layouts — and `render` also runs for cursor blink
        // and every snapshot frame, so doing this unconditionally would burn that walk 60×/second for
        // nothing. The inputs are the global palette, the profile definitions, and which sessions this
        // window owns; a tab moving between groups cannot change a session's profile (it is recorded on
        // the tab and travels with it), so the session set is the only layout fact that matters.
        //
        // The session set is keyed by a wrapping sum of its ids rather than a count: closing one tab and
        // opening another between frames leaves the count identical, which would skip the reconcile and
        // leave the new terminal on the previous palette.
        //
        // `use_dark` is an input in its own right, not merely implied by the global palette: a profile's
        // overrides are a dark/light pair too, so a system flip must re-resolve them even when the global
        // palette is unchanged (`theme_mode` pinned to Dark/Light, or both halves naming the same theme).
        // Likewise the preview key — a profile preview changes no global palette, so without it the
        // reconcile would skip the very repaint the preview exists to show.
        let appearance_inputs = (
            self.global_appearance,
            self.profile_store.read(cx).revision(),
            session_set_key(self.session_feed.sessions.iter().map(|session| session.id)),
            self.use_dark,
            theme_preview_key,
        );
        if self.applied_appearance_inputs != Some(appearance_inputs) {
            self.applied_appearance_inputs = Some(appearance_inputs);
            self.reconcile_session_appearances(cx);
        }
        self.colors = UiColors::from_theme(self.interface_theme);
        // Re-derive config-dependent state (font resolution, per-session daemon policy) only when the
        // config actually changed since last applied. The steady-state repaint — cursor blink,
        // snapshot stream, hover — leaves the config untouched, so this skips a 51-field clone plus
        // re-parsing every font feature/variation/codepoint string on every frame. `settings`
        // changes flow through `cx.observe` (which notifies), so a real edit still re-renders here.
        let config_changed = self.applied_config.as_ref() != Some(self.settings.read(cx).config());
        if config_changed {
            let config = self.settings.read(cx).config().clone();
            self.terminal_font_features = config.resolved_font_features().into();
            self.terminal_ligatures = config.ligatures;
            self.terminal_shaping_break_cursor = config.font_shaping_break_cursor;
            self.terminal_font_variations = config.resolved_font_variations().into();
            self.terminal_font_thicken =
                config.font_thicken.then_some(config.font_thicken_strength);
            self.terminal_font_codepoint_map = config.resolved_codepoint_map().into();
            self.terminal_padding_x = config.terminal_padding_x;
            self.terminal_padding_y = config.terminal_padding_y;
            self.terminal_minimum_contrast = config.minimum_contrast;
            self.terminal_metric_adjustments = config.font_metrics.resolve();
            let progress_timeouts = TerminalProgressTimeouts {
                completed_ms: config.progress_complete_timeout_secs.saturating_mul(1_000),
                stale_ms: config.progress_stale_timeout_secs.saturating_mul(1_000),
            };
            if progress_timeouts != self.progress_timeouts {
                self.progress_timeouts = progress_timeouts;
                for session_id in self.window_session_ids() {
                    self.configure_session_progress(session_id);
                }
            }
            if config.allow_osc_clipboard_read != self.allow_osc_clipboard_read {
                self.allow_osc_clipboard_read = config.allow_osc_clipboard_read;
                for session_id in self.window_session_ids() {
                    self.configure_session_osc_policy(session_id);
                }
            }
            if config.detect_urls != self.detect_urls {
                self.detect_urls = config.detect_urls;
                for session_id in self.window_session_ids() {
                    self.configure_session_url_detection(session_id);
                }
            }
            if config.cursor_shape != self.cursor_shape {
                self.cursor_shape = config.cursor_shape;
                for session_id in self.window_session_ids() {
                    self.configure_session_cursor_shape(session_id);
                }
            }
            if config.scrollback_lines != self.scrollback_limit {
                self.scrollback_limit = config.scrollback_lines;
                for session_id in self.window_session_ids() {
                    self.configure_session_scrollback(session_id);
                }
            }
            self.cursor_blink = config.cursor_blink;
            self.language = config.language;
            self.copy_on_select = config.copy_on_select;
            // Keep the search input's edit-menu labels in sync with the current language. Cheap:
            // clone the handle first to avoid overlapping the `self.terminal_search` borrow with `cx`.
            if let Some(input) = self.terminal_search.as_ref().map(|s| s.input.clone()) {
                input.update(cx, |input, _| input.set_language(config.language));
            }
            self.applied_config = Some(config);
        }
        // Keep the blink timer running while the active cursor blinks; it self-terminates otherwise.
        self.ensure_cursor_blink(cx);
        div()
            .id("eggie-workspace")
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && app.sidebar.project_drag.source.is_some() {
                    cx.stop_active_drag(window);
                    app.clear_project_drag(cx);
                    cx.stop_propagation();
                } else {
                    Self::key_down(app, event, window, cx);
                }
            }))
            .on_key_up(cx.listener(Self::key_up))
            .on_action(cx.listener(|app, _: &TerminalCopy, _, cx| {
                if app.copy_active_terminal_selection(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &TerminalPaste, _, cx| {
                if app.paste_into_active_terminal(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &TerminalSelectAll, _, cx| {
                if app.select_all_active_terminal(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &TerminalFind, window, cx| {
                if app.open_terminal_search(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &NewTab, window, cx| {
                if app.new_tab_in_active_group(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &CloseTab, _, cx| {
                if app.close_active_tab(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &NextTab, window, cx| {
                if app.activate_relative_tab(1, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &PrevTab, window, cx| {
                if app.activate_relative_tab(-1, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &SplitRight, window, cx| {
                if app.split_active_group(Direction::Right, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &SplitDown, window, cx| {
                if app.split_active_group(Direction::Down, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &ClearScreen, _, cx| {
                if app.clear_active_terminal(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &ScrollTop, _, cx| {
                if app.scroll_active_terminal(TerminalScrollCommand::Top, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &ScrollBottom, _, cx| {
                if app.scroll_active_terminal(TerminalScrollCommand::Bottom, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &PageUp, _, cx| {
                if app.scroll_active_terminal(TerminalScrollCommand::PageUp, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &PageDown, _, cx| {
                if app.scroll_active_terminal(TerminalScrollCommand::PageDown, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &FontIncrease, _, cx| {
                if app.adjust_font_size(1., cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &FontDecrease, _, cx| {
                if app.adjust_font_size(-1., cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &FontReset, _, cx| {
                if app.reset_font_size(cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &JumpPrevPrompt, _, cx| {
                if app.scroll_active_terminal(TerminalScrollCommand::PrevPrompt, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &JumpNextPrompt, _, cx| {
                if app.scroll_active_terminal(TerminalScrollCommand::NextPrompt, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &SplitLeft, window, cx| {
                if app.split_active_group(Direction::Left, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &SplitUp, window, cx| {
                if app.split_active_group(Direction::Up, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &FindNext, _, cx| {
                // No-op when the search bar is closed; always consume so the key never
                // reaches the terminal grid.
                app.navigate_terminal_search(TerminalSearchDirection::Forward, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|app, _: &FindPrevious, _, cx| {
                app.navigate_terminal_search(TerminalSearchDirection::Backward, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, cx| {
                window.minimize_window();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &ZoomWindow, window, cx| {
                window.zoom_window();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &ToggleFullScreen, window, cx| {
                window.toggle_fullscreen();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|_, _: &CloseWindow, window, cx| {
                // Direct close: bypasses the in-app close-confirmation prompt
                // (`install_close_confirmation`), matching the menu's "close now" intent.
                window.remove_window();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|app, _: &CommandPalette, window, cx| {
                app.toggle_command_palette(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|app, action: &LuaAction, window, cx| {
                let request = ClientRequest::InvokeConfigAction {
                    revision: app.settings.read(cx).lua_config().revision,
                    name: action.name.clone(),
                    context: eggie_config::ActionContext {
                        cwd: Some(app.workspace_state.active_project_data().effective_root()),
                        session_id: app.active_session_id().map(|id| id.to_string()),
                        window_id: Some(app.window_id.to_string()),
                    },
                    args: action.args.clone(),
                };
                let client = app.client.clone();
                let executor = cx.background_executor().clone();
                cx.spawn_in(window, async move |_, cx| {
                    let result = executor.spawn(async move { client.request(request) }).await;
                    match result {
                        Ok(DaemonResponse::ConfigActionResult { actions }) => {
                            let _ = cx.update(|window, cx| {
                                for action in actions {
                                    let built_in: Option<Box<dyn gpui::Action>> =
                                        match action.name.as_str() {
                                            "pane.split" => match action
                                                .args
                                                .get("direction")
                                                .and_then(serde_json::Value::as_str)
                                            {
                                                Some("right") => Some(Box::new(SplitRight)),
                                                Some("left") => Some(Box::new(SplitLeft)),
                                                Some("up") => Some(Box::new(SplitUp)),
                                                Some("down") => Some(Box::new(SplitDown)),
                                                _ => None,
                                            },
                                            "tab.new" => Some(Box::new(NewTab)),
                                            name => crate::keybindings::spec_by_id(name)
                                                .map(|spec| (spec.make_action)()),
                                        };
                                    if let Some(built_in) = built_in {
                                        window.dispatch_action(built_in, cx);
                                    }
                                }
                            });
                        }
                        Ok(DaemonResponse::Error { message }) => {
                            eprintln!("Lua action failed: {message}")
                        }
                        Ok(response) => eprintln!("unexpected Lua action response: {response:?}"),
                        Err(error) => eprintln!("Lua action request failed: {error:#}"),
                    }
                })
                .detach();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|app, _: &NewTabWithProfile, window, cx| {
                app.open_profile_picker(window, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|app, _: &ToggleLeftSidebar, _, cx| {
                app.toggle_left_sidebar(cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|app, _: &ToggleRightSidebar, _, cx| {
                app.toggle_right_sidebar(cx);
                cx.stop_propagation();
            }))
            .on_mouse_move(cx.listener(|app, event, window, cx| {
                app.resize_sidebar(event, window, cx);
                app.resize_split(event, window, cx);
                app.drag_scrollbar(event, cx);
                app.update_terminal_selection_drag(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|app, _, _, cx| {
                    app.resizing_sidebar = None;
                    app.resizing_split = None;
                    app.dragging_scrollbar = None;
                    app.clear_tab_drop_target(cx);
                    app.clear_project_drag(cx);
                    app.finish_terminal_selection_drag(cx);
                }),
            )
            .flex()
            .size_full()
            .min_w_0()
            .bg(rgb(self.colors.background))
            .text_color(rgb(self.colors.text))
            .text_sm()
            .when(!self.left_sidebar_collapsed, |element| {
                element
                    .child(self.render_left_sidebar(cx))
                    .child(self.render_sidebar_resize_handle(SidebarEdge::Left, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_layout(&self.workspace_state.workspace().layout, cx)),
            )
            .when(!self.right_sidebar_collapsed, |element| {
                element
                    .child(self.render_sidebar_resize_handle(SidebarEdge::Right, cx))
                    .child(self.render_right_sidebar(cx))
            })
            // Centered modal overlay, painted last so it sits above the whole workspace.
            .children(self.render_command_palette(cx))
    }
}

use helpers::*;
#[cfg(test)]
mod tests;

impl eggie_terminal_view::terminal_renderer::TerminalInputTarget for EggieApp {
    fn ime_state(&self, session: SessionId) -> Option<TerminalImeState> {
        self.terminal_ime_state(session).cloned()
    }
    fn commit_text(&mut self, session: SessionId, text: &str, cx: &mut Context<Self>) {
        self.commit_terminal_text(session, text, cx);
    }
    fn mark_text(
        &mut self,
        session: SessionId,
        text: &str,
        selection: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.set_terminal_marked_text(session, text, selection, cx);
    }
    fn clear_marked_text(&mut self, session: SessionId, cx: &mut Context<Self>) {
        self.clear_terminal_marked_text(session, cx);
    }
}

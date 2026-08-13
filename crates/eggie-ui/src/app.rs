use crate::icons::{IconName, icon, icon_sized};
use crate::command_palette::{PaletteEntry, command_matches, palette_entries};
use crate::input_latency::InputLatencyTracker;
use crate::native_menu::{
    NativeProcessMenuCommand, NativeProjectMenuCommand, NativeTabMenuCommand,
    prepare_process_menu, prepare_project_menu, prepare_tab_menu,
};
use crate::project_store::ProjectStore;
use crate::settings::{
    CursorBlink, CursorShapeSetting, DEFAULT_FONT_SIZE, Language, OpenWith, SettingsStore,
    TerminalTheme, UiColors,
};
use crate::settings_window::{
    ClearScreen, CloseTab, CloseWindow, CommandPalette, FindNext, FindPrevious, FontDecrease,
    FontIncrease, FontReset, JumpNextPrompt, JumpPrevPrompt, MinimizeWindow, NewTab, NextTab,
    PageDown, PageUp, PrevTab, ScrollBottom, ScrollTop, SplitDown, SplitLeft, SplitRight, SplitUp,
    TerminalCopy, TerminalFind, TerminalPaste, TerminalSelectAll, ToggleFullScreen,
    ToggleLeftSidebar, ToggleRightSidebar, ZoomWindow, is_dark_appearance,
};
use crate::text_input::{TextInput, TextInputEvent, TextInputStyle};
use crate::terminal_renderer::{
    MetalTerminalRenderer, TerminalImageData, TerminalImeState, TerminalInputContext,
    TerminalPoint, TerminalRenderOptions, TerminalSearchHighlights, TerminalSelection,
    TerminalTextureKey,
    terminal_background, terminal_cell_metrics,
};
use eggie_daemon::{DaemonClient, DaemonInputSender};
use eggie_domain::{
    Direction, GroupId, ItemId, ItemKind, LayoutNode, Project, ProjectId, SessionId, SplitAxis,
    SplitId, TabGroup, TabItem, WindowId,
};
use eggie_protocol::{
    ClientRequest, DaemonResponse, ListeningPort, ProcessInfo, SessionInspection, SessionSummary,
    TERMINAL_SCROLL_DELTA_SCALE, TerminalAppearance, TerminalAttentionRequest,
    TerminalClipboardContent, TerminalClipboardSelection, TerminalCursorShape,
    TerminalImageDescriptor,
    TerminalModifiers, TerminalMouseAction, TerminalMouseButton, TerminalMouseEvent,
    TerminalMousePosition, TerminalMouseTracking, TerminalOscEventPayload, TerminalProgress,
    TerminalProgressState, TerminalProgressTimeouts, TerminalProgressUpdate, TerminalScrollCommand,
    TerminalScrollDelta, TerminalScrollEvent, TerminalScrollPhase, TerminalScrollUnit,
    TerminalSearchDirection,
    TerminalSearchRequest, TerminalSearchResult, TerminalSelectionKind, TerminalSelectionSide,
    TerminalSize, TerminalSnapshot,
};
use gpui::{
    Anchor, AnyElement, App, Bounds, ClipboardEntry, ClipboardItem, ClipboardString, Context, Div,
    DragMoveEvent, Entity, FocusHandle, Image, ImageFormat, KeyDownEvent, KeyUpEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, PathPromptOptions,
    Pixels, Point, PromptLevel, Role, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString,
    Stateful, Subscription, SystemNotification, SystemNotificationAction, TitlebarOptions,
    TouchPhase, WeakEntity, Window, WindowBounds, WindowControlArea, WindowOptions, anchored,
    canvas, deferred, div, linear_color_stop, linear_gradient, point, prelude::*, px, quad,
    relative, rgb, rgba, size,
};
use std::{
    borrow::Cow,
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
use cocoa::{
    base::{id, nil},
    foundation::{NSInteger, NSPoint, NSRect, NSString, NSSize},
};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use block::ConcreteBlock;
#[cfg(target_os = "macos")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const TAB_BAR_HEIGHT: f32 = 36.;
const TITLEBAR_HEIGHT: f32 = TAB_BAR_HEIGHT;
const TAB_MIN_WIDTH: f32 = 140.;
const TAB_MAX_WIDTH: f32 = 220.;
const TAB_BAR_PADDING: f32 = 5.;
const TAB_GAP: f32 = 5.;
const TAB_TITLE_FONT_SIZE: f32 = 11.;
const TAB_CLOSE_ICON_SIZE: f32 = 10.;
const TAB_DROP_INDICATOR_WIDTH: f32 = 2.;
const TAB_DROP_INDICATOR_OFFSET: f32 = (TAB_GAP + TAB_DROP_INDICATOR_WIDTH) / 2.;
const TOP_BAR_ACTION_SLOT_WIDTH: f32 = 28.;
const ITEM_ICON_SLOT_SIZE: f32 = 16.;
const SIDEBAR_SESSION_ROW_HEIGHT: f32 = 24.;
const CONTENT_SPLIT_EDGE_FACTOR: f32 = 0.1;
const LEFT_SIDEBAR_DEFAULT_WIDTH: f32 = 230.;
const LEFT_SIDEBAR_MIN_WIDTH: f32 = 180.;
const LEFT_SIDEBAR_MAX_WIDTH: f32 = 420.;
const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 300.;
const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 220.;
const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 520.;
const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 5.;
// The split divider has a wide invisible grab area for easy clicking, and a thin visible line.
const SPLIT_DIVIDER_HANDLE_WIDTH: f32 = 8.;
const SPLIT_DIVIDER_LINE_WIDTH: f32 = 2.;
const SIDEBAR_SCROLLBAR_WIDTH: f32 = 8.;
const SIDEBAR_SCROLLBAR_THUMB_WIDTH: f32 = 4.;
const SIDEBAR_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 24.;
// The terminal scrollbar occupies a thin strip on the right edge of each pane. It is always shown
// while there is scrollback, and darkens slightly on hover/drag. The track is a wider transparent
// grab/hit area; the visible thumb is narrower.
const TERMINAL_SCROLLBAR_WIDTH: f32 = 12.;
const TERMINAL_SCROLLBAR_THUMB_WIDTH: f32 = 6.;
const TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 24.;
const SESSION_INSPECTION_INTERVAL: Duration = Duration::from_secs(1);
const SESSION_LIST_INTERVAL: Duration = Duration::from_millis(500);
const SNAPSHOT_WAIT_TIMEOUT: Duration = Duration::from_millis(50);
const PROGRESS_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const OSC_EVENT_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const PROCESS_ROW_HEIGHT: f32 = 20.;
const TERMINAL_IMAGE_CHUNK_SIZE: u32 = 16 * 1024 * 1024;
/// A bell triggers a single smooth pulse: the tab tint fades in, then back out, over this span.
const BELL_FLASH_DURATION: Duration = Duration::from_millis(420);
/// Repaint cadence during the pulse — small enough for a continuous fade.
const BELL_FLASH_STEP: Duration = Duration::from_millis(16);
/// Peak alpha of the accent tint at the middle of the pulse (0x00..=0xff).
const BELL_FLASH_PEAK_ALPHA: f32 = 0x66 as f32;
/// Full cursor blink cycle (on + off). The cursor is solid for the first half and hidden for the
/// second. ~1s matches the customary terminal blink rate.
const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TerminalImageStreamKey {
    session_id: SessionId,
    image_id: u32,
}

fn fetch_terminal_image(
    client: DaemonClient,
    session_id: SessionId,
    descriptor: TerminalImageDescriptor,
) -> anyhow::Result<TerminalImageData> {
    let mut last_error = None;
    for attempt in 0..3 {
        match fetch_terminal_image_once(client.clone(), session_id, descriptor.clone()) {
            Ok(image) => return Ok(image),
            Err(error) => last_error = Some(error),
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
        }
    }
    Err(last_error.expect("terminal image transfer attempted at least once"))
}

fn fetch_terminal_image_once(
    client: DaemonClient,
    session_id: SessionId,
    descriptor: TerminalImageDescriptor,
) -> anyhow::Result<TerminalImageData> {
    let expected_length = descriptor
        .width
        .checked_mul(descriptor.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("terminal image dimensions overflow"))?;
    let mut connection = client.connect()?;
    let mut pixels = Vec::with_capacity(expected_length as usize);
    while pixels.len() < expected_length as usize {
        let offset = pixels.len() as u32;
        let metadata = connection.append_terminal_image_chunk(
            session_id,
            descriptor.key,
            offset,
            TERMINAL_IMAGE_CHUNK_SIZE,
            &mut pixels,
        )?;
        if metadata.key != descriptor.key
            || metadata.width != descriptor.width
            || metadata.height != descriptor.height
            || metadata.total_length != expected_length
            || metadata.offset != offset
        {
            anyhow::bail!("terminal image metadata changed during transfer");
        }
        if metadata.chunk_length == 0 {
            anyhow::bail!("terminal image transfer ended before all pixels arrived");
        }
        if pixels.len() > expected_length as usize {
            anyhow::bail!("terminal image transfer exceeded its declared length");
        }
    }
    Ok(TerminalImageData {
        key: TerminalTextureKey {
            session_id,
            image: descriptor.key,
        },
        width: descriptor.width,
        height: descriptor.height,
        pixels: Arc::new(pixels),
    })
}
const PROCESS_TREE_LEVEL_WIDTH: f32 = 16.;
const PROCESS_TREE_LINE_X: f32 = 3.;
const PROCESS_TREE_ARM_LENGTH: f32 = 9.;
const PROCESS_TREE_CORNER_RADIUS: f32 = 3.;
const TRAFFIC_LIGHT_DIAMETER: f32 = 14.;
const TRAFFIC_LIGHT_INSET: f32 = (TITLEBAR_HEIGHT - TRAFFIC_LIGHT_DIAMETER) / 2.;
const TRAFFIC_LIGHT_INSET_WIDTH: f32 = 72.;
const HUGEICONS_FONT: &[u8] = include_bytes!("../assets/hgi-stroke-rounded.ttf");
static NEXT_INPUT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct NotificationRoute {
    app: WeakEntity<EggieApp>,
    client: DaemonClient,
    session_id: SessionId,
    notification_id: String,
}

pub(crate) type NotificationRoutes = Rc<RefCell<HashMap<String, NotificationRoute>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RightTab {
    #[default]
    Info,
    Files,
    Git,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarEdge {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabDropZone {
    Center,
    Edge(Direction),
    TabBar { insertion_index: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabDropTarget {
    group_id: GroupId,
    zone: TabDropZone,
}

#[derive(Clone, Copy)]
struct TerminalViewport {
    session_id: SessionId,
    bounds: Bounds<Pixels>,
    cell_width: f32,
    line_height: f32,
    rows: u16,
    columns: u16,
}

#[derive(Clone, Copy)]
struct TerminalSelectionDrag {
    group_id: GroupId,
    session_id: SessionId,
    dragged: bool,
}

/// State for an in-progress scrollbar thumb drag. `grab_offset` is the pointer's Y distance from the
/// thumb's top at grab time, so the thumb doesn't jump under the cursor. `pending_offset` is the
/// last offset we asked the daemon for; it drives optimistic thumb rendering so the thumb tracks the
/// pointer instantly rather than waiting for the round-tripped snapshot.
#[derive(Clone, Copy)]
struct ScrollbarDrag {
    group_id: GroupId,
    session_id: SessionId,
    grab_offset: f32,
    pending_offset: u32,
}

/// State for the in-terminal search bar (⌘F). At most one search is active at a time, targeting the
/// currently focused session. Closing the bar or switching sessions resets it.
struct TerminalSearchUi {
    /// The session the search bar is bound to.
    session_id: SessionId,
    /// The reusable text-input component holding the query text, selection, cursor, and focus.
    input: Entity<TextInput>,
    /// Whether the query is interpreted as a regular expression.
    regex: bool,
    /// The most recent result from the daemon, used to render highlights and the match counter.
    result: Option<TerminalSearchResult>,
    /// Subscription to the input's events; dropped (unsubscribed) when the search bar closes.
    _subscription: Subscription,
}

/// State for the command palette (⌘⇧P). A centered modal listing every command; the query filters
/// it, ↑/↓ move the selection, Enter/click runs the selected command, Esc closes. At most one is
/// open at a time. The command data comes from [`crate::command_palette::palette_entries`].
struct CommandPaletteUi {
    /// The reusable text-input component holding the filter query, cursor, and focus.
    input: Entity<TextInput>,
    /// Index of the highlighted row within the *filtered* list (clamped on every change).
    selected_index: usize,
    /// Scroll handle for the results list, so the selected row can be kept in view.
    scroll_handle: ScrollHandle,
    /// Subscription to the input's events; dropped (unsubscribed) when the palette closes.
    _subscription: Subscription,
}

#[derive(Clone, Debug)]
struct DraggedTab {
    source_group_id: GroupId,
    item_id: ItemId,
    item_kind: ItemKind,
    title: SharedString,
    active: bool,
    colors: UiColors,
}

impl DraggedTab {
    fn preview_element(&self) -> AnyElement {
        div()
            .flex()
            .items_center()
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .h(px(TAB_BAR_HEIGHT - TAB_BAR_PADDING * 2.))
            .overflow_hidden()
            .pl_2()
            .pr_1()
            .gap_2()
            .text_size(px(TAB_TITLE_FONT_SIZE))
            .rounded_md()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(if self.active {
                self.colors.panel_alt
            } else {
                self.colors.panel
            }))
            .text_color(rgb(self.colors.text))
            .shadow_lg()
            // Semi-transparent so the drop target and tabs underneath the cursor stay visible while
            // dragging.
            .opacity(0.7)
            .child(
                div()
                    .flex_none()
                    .child(icon(item_kind_icon(self.item_kind))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.title.clone()),
            )
            .into_any_element()
    }
}

struct DraggedTabPreview {
    tab: DraggedTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalContainerCorners {
    top_left: bool,
    top_right: bool,
    bottom_left: bool,
    bottom_right: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalContainerBorders {
    left: bool,
    right: bool,
    bottom: bool,
}

struct ProgressTooltip {
    text: SharedString,
    colors: UiColors,
}

impl gpui::Render for ProgressTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel_alt))
            .text_color(rgb(self.colors.text))
            .text_size(px(11.))
            .shadow_md()
            .child(self.text.clone())
    }
}

/// A reusable single-line text tooltip. Shares [`ProgressTooltip`]'s styling but is generic over any
/// label, so buttons can attach a hover hint with [`tooltip_text`].
struct TextTooltip {
    text: SharedString,
    colors: UiColors,
}

impl gpui::Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel_alt))
            .text_color(rgb(self.colors.text))
            .text_size(px(11.))
            .shadow_md()
            .child(self.text.clone())
    }
}

/// Build a `.tooltip(...)` callback showing `text`. Use as `element.tooltip(tooltip_text(label, colors))`.
fn tooltip_text(
    text: impl Into<SharedString>,
    colors: UiColors,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| TextTooltip {
            text: text.clone(),
            colors,
        })
        .into()
    }
}

impl gpui::Render for DraggedTabPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.tab.preview_element()
    }
}

/// Per-window, per-project view state. The `Project` itself (id/name/root) lives in the shared
/// [`ProjectStore`] so it syncs across windows; the layout tree and the active group are local to
/// this window because sessions are not shared between windows.
struct WindowProjectView {
    layout: LayoutNode,
    active_group_id: GroupId,
}

/// Identifies one of the two directory rows in the right sidebar's Info tab. Used as the key for the
/// per-row "open with" dropdown state so each row's split-button anchors its own dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DirectoryField {
    Current,
    Initial,
}

impl DirectoryField {
    /// A stable ascii slug for element ids.
    fn slug(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Initial => "initial",
        }
    }
}

impl WindowProjectView {
    /// A view for a project that currently has no terminal in this window: an empty layout whose
    /// single empty group is the active group (and a valid drop target for the first new terminal).
    fn empty() -> Self {
        let layout = LayoutNode::empty();
        let active_group_id = layout.first_group_id();
        Self {
            layout,
            active_group_id,
        }
    }
}

/// How a freshly opened window should seed its initial project/session state.
pub(crate) enum NewWindowInit {
    /// The process's first window (cold start). Reconcile the persisted project list with the
    /// daemon's existing sessions: adopt a project for `project_root`, claim surviving sessions into
    /// this window, and open a default terminal if the daemon had nothing.
    ColdStart { project_root: PathBuf },
    /// A `New Window` (Cmd+N) opened at runtime. The shared project list already exists; activate the
    /// first project and show it empty (D7: no terminal is spawned until the user acts).
    Empty,
    /// A single-instance wake (`eggie <dir>`). Activate `project_id` and open one default terminal in
    /// it, since running `eggie <dir>` is an explicit request for a terminal there.
    OpenDir { project_id: ProjectId },
}

/// Fallback working directory when a project's root can't be resolved: the user's home, or `.`.
fn fallback_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub struct EggieApp {
    client: DaemonClient,
    input_sender: DaemonInputSender,
    input_latency: InputLatencyTracker,
    settings: Entity<SettingsStore>,
    updates: Entity<crate::update_controller::UpdateController>,
    colors: UiColors,
    terminal_theme: &'static TerminalTheme,
    terminal_appearance: TerminalAppearance,
    terminal_font_family: SharedString,
    terminal_font_size: f32,
    terminal_padding_x: f32,
    terminal_padding_y: f32,
    terminal_minimum_contrast: f32,
    terminal_metric_adjustments: crate::settings::ResolvedMetricAdjustments,
    terminal_font_families: crate::settings::ResolvedFontFamilies,
    terminal_font_features: std::sync::Arc<[crate::settings::FontFeature]>,
    terminal_ligatures: bool,
    terminal_shaping_break_cursor: bool,
    terminal_font_variations: std::sync::Arc<[crate::settings::FontVariation]>,
    terminal_font_thicken: Option<u8>,
    terminal_font_codepoint_map: std::sync::Arc<[crate::settings::CodepointMapEntry]>,
    terminal_renderer: MetalTerminalRenderer,
    terminal_images_in_flight: HashSet<TerminalImageStreamKey>,
    terminal_frame_scheduled: bool,
    progress_frame_scheduled: bool,
    progress_animation_scheduled: bool,
    progress_animation_epoch: Instant,
    /// Sessions currently flashing from a terminal bell, keyed by the flash's start time.
    bell_flash: HashMap<SessionId, Instant>,
    /// Whether the bell-flash animation loop is already running (prevents duplicate timers).
    bell_flash_scheduled: bool,
    /// Phase origin for the terminal cursor blink; reset so the cursor is solid right after typing.
    cursor_blink_epoch: Instant,
    /// Whether the cursor-blink animation loop is already running (prevents duplicate timers).
    cursor_blink_scheduled: bool,
    progress_timeouts: TerminalProgressTimeouts,
    snapshot_watchers: HashSet<SessionId>,
    progress_watchers: HashSet<SessionId>,
    osc_watchers: HashSet<SessionId>,
    progress_updates: HashMap<SessionId, TerminalProgressUpdate>,
    pending_progress_updates: HashMap<SessionId, TerminalProgressUpdate>,
    /// Identity of this window, generated once at construction. Every session this window creates is
    /// tagged with it in the daemon, so `ListSessions` results can be filtered to this window's own
    /// sessions (plus detached ones). Not persisted: a window id is meaningless once the window closes.
    window_id: WindowId,
    /// Shared, cross-window project list (name/root/order). Cloned handle of the one `Entity` all
    /// windows observe; mutating it re-renders every window and re-syncs their `project_views`.
    project_store: Entity<ProjectStore>,
    /// Local ordered mirror of `project_store.projects()`, refreshed by `reconcile_project_views`.
    /// Kept so render/iteration can read projects without holding a `cx` borrow on the entity.
    projects: Vec<Project>,
    /// Per-window layout/active-group for each project, keyed by project id. Rebuilt to track the
    /// shared list: a project synced in from another window starts as an empty view (no session).
    project_views: HashMap<ProjectId, WindowProjectView>,
    /// The project this window currently shows. A `ProjectId` (not an index) because the shared list
    /// can be reordered by another window, which would invalidate an index.
    active_project: ProjectId,
    collapsed_projects: HashSet<ProjectId>,
    /// Sessions owned by this window (daemon `window_id == self.window_id`), refreshed by polling.
    sessions: Vec<SessionSummary>,
    /// Detached sessions (daemon `window_id == None`): survive their closed window and are claimable
    /// from any window. Surfaced through the sidebar's floating-session popover (not the project tree).
    detached_sessions: Vec<SessionSummary>,
    /// Whether the detached-session popover is open.
    detached_popover_open: bool,
    /// On-screen bounds of the detached-sessions toggle button, captured at paint so the popover can
    /// anchor just below it (like the settings dropdowns) instead of snapping to the window corner.
    detached_toggle_bounds: Option<Bounds<Pixels>>,
    /// Whether the update popover (download progress / ready-to-restart) is open.
    update_popover_open: bool,
    /// On-screen bounds of the update indicator button, for popover anchoring.
    update_toggle_bounds: Option<Bounds<Pixels>>,
    /// Which directory field's "open with" dropdown is currently open (right sidebar), if any.
    open_with_dropdown: Option<DirectoryField>,
    /// On-screen bounds of each directory field's split-button, captured at paint so its dropdown can
    /// anchor just below the button.
    open_with_dropdown_bounds: HashMap<DirectoryField, Bounds<Pixels>>,
    snapshots: HashMap<SessionId, Arc<TerminalSnapshot>>,
    terminal_sizes: HashMap<SessionId, TerminalSize>,
    terminal_resize_in_flight: HashMap<SessionId, TerminalSize>,
    terminal_viewports: HashMap<GroupId, TerminalViewport>,
    terminal_selection_drag: Option<TerminalSelectionDrag>,
    terminal_search: Option<TerminalSearchUi>,
    /// The command palette (⌘⇧P) overlay, when open.
    command_palette: Option<CommandPaletteUi>,
    hyperlink_mouse_down: bool,
    /// The auto-detected URL currently under the mouse, if any, for hover underline + pointer cursor.
    terminal_hovered_link: Option<(SessionId, eggie_protocol::TerminalLinkRange)>,
    terminal_has_focus: bool,
    terminal_focused_session: Option<SessionId>,
    terminal_ime_states: HashMap<SessionId, TerminalImeState>,
    session_inspections: HashMap<SessionId, SessionInspection>,
    session_inspection_errors: HashMap<SessionId, String>,
    process_metrics_system: System,
    right_tab: RightTab,
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
    /// The scrollbar thumb currently being dragged, if any.
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
    /// Mirror of `config.scrollback_lines`, so `render` can diff it and push runtime changes to all
    /// live sessions (the shell fields are spawn-only and need no mirror).
    scrollback_limit: usize,
    cursor_blink: CursorBlink,
    language: Language,
    closing: bool,
    poll_error: Option<String>,
}

/// Show a native macOS text-input alert and return the entered string, or `None` if cancelled.
///
/// Uses `beginSheetModalForWindow` (asynchronous) so it does not block GPUI's event loop or
/// re-enter the app's `RefCell` borrow while a mouse event is being dispatched.
#[cfg(target_os = "macos")]
fn prompt_for_text(
    window: &Window,
    title: &str,
    message: &str,
    placeholder: Option<&str>,
    ok_label: &str,
    cancel_label: &str,
) -> futures::channel::oneshot::Receiver<Option<String>> {
    let (tx, rx) = futures::channel::oneshot::channel();

    unsafe {
        let alert: id = msg_send![class!(NSAlert), alloc];
        let alert: id = msg_send![alert, init];
        let _: () = msg_send![
            alert,
            setMessageText: NSString::alloc(nil).init_str(title)
        ];
        let _: () = msg_send![
            alert,
            setInformativeText: NSString::alloc(nil).init_str(message)
        ];

        let text_field: id = msg_send![class!(NSTextField), alloc];
        let text_field: id = msg_send![
            text_field,
            initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(260., 24.))
        ];
        if let Some(placeholder) = placeholder {
            let _: () = msg_send![
                text_field,
                setPlaceholderString: NSString::alloc(nil).init_str(placeholder)
            ];
        }
        let _: () = msg_send![alert, setAccessoryView: text_field];

        let _: id = msg_send![alert, addButtonWithTitle: NSString::alloc(nil).init_str(ok_label)];
        let cancel_button: id =
            msg_send![alert, addButtonWithTitle: NSString::alloc(nil).init_str(cancel_label)];
        let _: () = msg_send![
            cancel_button,
            setKeyEquivalent: NSString::alloc(nil).init_str("\u{1b}")
        ];

        let native_window = match HasWindowHandle::window_handle(window) {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::AppKit(appkit) => {
                    let ns_view = appkit.ns_view.as_ptr() as id;
                    if ns_view == nil {
                        nil
                    } else {
                        msg_send![ns_view, window]
                    }
                }
                _ => nil,
            },
            Err(_) => nil,
        };

        if native_window == nil {
            let _ = tx.send(None);
            return rx;
        }

        let tx = std::cell::Cell::new(Some(tx));
        let block = ConcreteBlock::new(move |response: NSInteger| {
            let result = if response == 1000 {
                let value: id = msg_send![text_field, stringValue];
                let bytes: *const std::os::raw::c_char = msg_send![value, UTF8String];
                if bytes.is_null() {
                    None
                } else {
                    Some(std::ffi::CStr::from_ptr(bytes).to_string_lossy().into_owned())
                }
            } else {
                None
            };
            if let Some(tx) = tx.take() {
                let _ = tx.send(result);
            }
        });
        let block = block.copy();

        let _: () = msg_send![
            alert,
            beginSheetModalForWindow: native_window
            completionHandler: block
        ];
    }

    rx
}

#[cfg(not(target_os = "macos"))]
fn prompt_for_text(
    _window: &Window,
    _title: &str,
    _message: &str,
    _placeholder: Option<&str>,
    _ok_label: &str,
    _cancel_label: &str,
) -> futures::channel::oneshot::Receiver<Option<String>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let _ = tx.send(None);
    rx
}

impl EggieApp {
    pub fn launch(project_root: PathBuf, client: DaemonClient) {
        let settings_store = SettingsStore::load();
        let project_store = ProjectStore::load();

        gpui_platform::application().run(move |cx: &mut App| {
            cx.set_app_identity("me.daidr.eggie", "Eggie");
            let notification_routes: NotificationRoutes = Rc::new(RefCell::new(HashMap::new()));
            let response_routes = notification_routes.clone();
            cx.on_system_notification_response(move |response, cx| {
                let tag = response.tag.to_string();
                let Some(route) = response_routes.borrow().get(&tag).cloned() else {
                    return;
                };
                let activated = response.action_id.as_deref() != Some("close");
                let _ = route.client.request(ClientRequest::NotificationResponse {
                    session_id: route.session_id,
                    notification_id: route.notification_id.clone(),
                    activated,
                });
                cx.dismiss_system_notification(&tag);
                response_routes.borrow_mut().remove(&tag);
                if activated {
                    cx.activate(true);
                    let _ = route.app.update_in(cx, |app, window, cx| {
                        window.activate_window();
                        app.activate_session_by_id(route.session_id, window, cx);
                    });
                }
            });
            cx.text_system()
                .add_fonts(vec![Cow::Borrowed(HUGEICONS_FONT)])
                .expect("failed to register the embedded Hugeicons font");
            let settings = cx.new(|_| settings_store);
            let project_store = cx.new(|_| project_store);
            let updates = cx.new(|_| crate::update_controller::UpdateController::new());
            crate::settings_window::install(
                settings.clone(),
                project_store.clone(),
                client.clone(),
                notification_routes.clone(),
                updates.clone(),
                cx,
            );
            Self::open_new_window(
                cx,
                client.clone(),
                settings.clone(),
                updates.clone(),
                project_store.clone(),
                notification_routes.clone(),
                NewWindowInit::ColdStart {
                    project_root: project_root.clone(),
                },
            );
            // Single-instance listener: bind the GUI wake socket so subsequent `eggie` invocations
            // open a window here instead of starting a second process.
            Self::spawn_wake_listener(
                cx,
                client.clone(),
                settings.clone(),
                updates.clone(),
                project_store.clone(),
                notification_routes.clone(),
            );
            // Automatic update check, shortly after launch (staggered to avoid the startup
            // scramble). Silent: failures stay in the log, successes light up the sidebar.
            {
                let updates = updates.clone();
                let settings = settings.clone();
                let executor = cx.background_executor().clone();
                cx.spawn(async move |cx| {
                    executor.timer(Duration::from_secs(3)).await;
                    let _ = cx.update(|cx| {
                        if settings.read(cx).config().auto_check_updates {
                            updates.update(cx, |controller, cx| controller.check(true, cx));
                        }
                    });
                })
                .detach();
            }
            // D9: quitting when the last main window closes. The daemon and its sessions live on
            // independently; the next `eggie` invocation reopens a window and reclaims them. The
            // settings window alone must not keep the process alive, so only count `EggieApp` windows.
            cx.on_window_closed(|cx, _closed_window_id| {
                let has_main_window = cx
                    .windows()
                    .into_iter()
                    .any(|window| window.downcast::<EggieApp>().is_some());
                if !has_main_window {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    }

    /// Bind the GUI single-instance socket and serve wake requests. A blocking `accept` loop runs on
    /// a dedicated std thread (the socket API is blocking); each decoded [`GuiWakeMessage`] is pushed
    /// onto a channel that a foreground task drains, so window creation happens on the main thread.
    fn spawn_wake_listener(
        cx: &mut App,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        project_store: Entity<ProjectStore>,
        notification_routes: NotificationRoutes,
    ) {
        crate::gui_ipc::reap_stale_gui_sockets();
        let listener = match crate::gui_ipc::bind_listener() {
            Ok(listener) => listener,
            Err(error) => {
                // Losing the bind race just means another instance is the primary; degrade to a
                // standalone window rather than failing.
                eprintln!("single-instance listener unavailable: {error:#}");
                return;
            }
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<PathBuf>();
        std::thread::Builder::new()
            .name("eggie-gui-wake".to_owned())
            .spawn(move || {
                crate::gui_ipc::serve(listener, |message| match message {
                    crate::gui_ipc::GuiWakeMessage::OpenWindow { cwd } => {
                        let _ = tx.unbounded_send(cwd);
                    }
                });
            })
            .expect("failed to start GUI wake listener thread");

        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            use futures::StreamExt;
            while let Some(cwd) = rx.next().await {
                let client = client.clone();
                let settings = settings.clone();
                let updates = updates.clone();
                let project_store = project_store.clone();
                let notification_routes = notification_routes.clone();
                cx.update(|cx| {
                    // D4: reuse a project with the same root, otherwise add one, then open a window
                    // focused on it.
                    let (project_id, _created) = project_store
                        .update(cx, |store, cx| store.upsert_by_root(cwd.clone(), cx));
                    Self::open_new_window(
                        cx,
                        client,
                        settings,
                        updates,
                        project_store.clone(),
                        notification_routes,
                        NewWindowInit::OpenDir { project_id },
                    );
                    cx.activate(true);
                });
            }
        })
        .detach();
    }

    /// Open a new main window on an already-running app. Called for the first (cold-start) window
    /// and for every subsequent `New Window` / single-instance wake. All windows share the same
    /// `settings` and `project_store` entities, so their project lists stay in sync.
    pub(crate) fn open_new_window(
        cx: &mut App,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        project_store: Entity<ProjectStore>,
        notification_routes: NotificationRoutes,
        init: NewWindowInit,
    ) {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(
                        px(TRAFFIC_LIGHT_INSET),
                        px(TRAFFIC_LIGHT_INSET),
                    )),
                }),
                app_owns_titlebar_drag: true,
                window_min_size: Some(size(px(760.), px(480.))),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    Self::new(
                        WindowId::new_v4(),
                        client,
                        settings,
                        updates,
                        project_store,
                        notification_routes,
                        init,
                        window,
                        cx,
                    )
                })
            },
        )
        .expect("failed to open Eggie window");
    }

    /// Resolve a [`NewWindowInit`] into this window's `(active_project, owned_sessions)`, mutating the
    /// shared [`ProjectStore`] as needed. Runs before the `EggieApp` struct exists, so it takes the
    /// pieces it needs explicitly and talks to the daemon directly.
    fn seed_window(
        init: &NewWindowInit,
        window_id: WindowId,
        client: &DaemonClient,
        project_store: &Entity<ProjectStore>,
        config: &crate::settings::AppSettings,
        appearance: TerminalAppearance,
        cx: &mut Context<Self>,
    ) -> (ProjectId, Vec<SessionSummary>) {
        match init {
            NewWindowInit::ColdStart { project_root } => {
                Self::seed_cold_start(project_root, window_id, client, project_store, config, appearance, cx)
            }
            NewWindowInit::OpenDir { project_id } => {
                // The project already exists in the shared store (added by the waking path). Open one
                // terminal in it, since `eggie <dir>` is an explicit request for a terminal there.
                let root = project_store
                    .read(cx)
                    .projects()
                    .iter()
                    .find(|project| project.id == *project_id)
                    .map(|project| project.effective_root())
                    .unwrap_or_else(fallback_home_dir);
                let sessions = Self::create_session(client, *project_id, window_id, root, config, appearance)
                    .into_iter()
                    .collect();
                (*project_id, sessions)
            }
            NewWindowInit::Empty => {
                // New Window (Cmd+N): activate the first shared project, show it empty (D7).
                let active = project_store
                    .read(cx)
                    .projects()
                    .first()
                    .map(|project| project.id);
                match active {
                    Some(id) => (id, Vec::new()),
                    None => {
                        // No projects at all (should not happen once Home exists); create one.
                        let project = Project::new("Home".to_owned());
                        let id = project.id;
                        project_store.update(cx, |store, cx| store.add(project, cx));
                        (id, Vec::new())
                    }
                }
            }
        }
    }

    /// Cold-start seeding: reconcile the persisted project list with the daemon's surviving sessions.
    fn seed_cold_start(
        project_root: &Path,
        window_id: WindowId,
        client: &DaemonClient,
        project_store: &Entity<ProjectStore>,
        config: &crate::settings::AppSettings,
        appearance: TerminalAppearance,
        cx: &mut Context<Self>,
    ) -> (ProjectId, Vec<SessionSummary>) {
        let all_sessions = match client.request(ClientRequest::ListSessions) {
            Ok(DaemonResponse::Sessions { sessions }) => sessions,
            _ => Vec::new(),
        };
        // Adopt a project for the launch directory: reuse a persisted project with the same root,
        // otherwise create one. If there are no persisted projects at all, fall back to Home.
        let (active_project, project_root_buf) = project_store.update(cx, |store, cx| {
            let (id, _) = store.upsert_by_root(project_root.to_path_buf(), cx);
            (id, project_root.to_path_buf())
        });

        // Claim every surviving session whose project this window is adopting, and any that match the
        // launch directory, into this window. This preserves the old "restart keeps your terminals".
        let mut owned: Vec<SessionSummary> = Vec::new();
        for session in all_sessions {
            let belongs = session.project_id == active_project
                || session.initial_directory == project_root_buf;
            if !belongs {
                continue;
            }
            if let Ok(DaemonResponse::SessionClaimed { session }) =
                client.request(ClientRequest::ClaimSession {
                    session_id: session.id,
                    window_id,
                })
            {
                owned.push(session);
            }
        }

        // Nothing to adopt: open a fresh terminal so the window is not empty on first launch.
        if owned.is_empty() {
            let root = project_store
                .read(cx)
                .projects()
                .iter()
                .find(|project| project.id == active_project)
                .map(|project| project.effective_root())
                .unwrap_or(project_root_buf);
            owned.extend(Self::create_session(
                client,
                active_project,
                window_id,
                root,
                config,
                appearance,
            ));
        }
        (active_project, owned)
    }

    /// Spawn a single terminal session in the daemon, returning its summary (or `None` on error).
    fn create_session(
        client: &DaemonClient,
        project_id: ProjectId,
        window_id: WindowId,
        cwd: PathBuf,
        config: &crate::settings::AppSettings,
        appearance: TerminalAppearance,
    ) -> Option<SessionSummary> {
        match client.request(ClientRequest::CreateSession {
            project_id,
            window_id,
            cwd,
            size: TerminalSize::default(),
            appearance,
            scrollback_limit: config.scrollback_lines,
            shell_program: config.shell_program.clone(),
            shell_args: config.shell_args.clone(),
            shell_features: config.shell_features_string(),
        }) {
            Ok(DaemonResponse::SessionCreated { session }) => Some(session),
            other => {
                eprintln!("failed to create terminal session: {other:?}");
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        window_id: WindowId,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        project_store: Entity<ProjectStore>,
        notification_routes: NotificationRoutes,
        init: NewWindowInit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = settings.read(cx).config().clone();
        let terminal_theme = config.effective_theme(is_dark_appearance(window.appearance()));
        let terminal_appearance = terminal_theme.appearance();
        let progress_timeouts = TerminalProgressTimeouts {
            completed_ms: config.progress_complete_timeout_secs.saturating_mul(1_000),
            stale_ms: config.progress_stale_timeout_secs.saturating_mul(1_000),
        };
        let colors = UiColors::from_theme(terminal_theme);

        // Resolve the requested init into this window's active project and owned sessions, mutating
        // the shared project store as needed (seeding Home / adopting the launch directory).
        let (active_project, sessions) = Self::seed_window(
            &init,
            window_id,
            &client,
            &project_store,
            &config,
            terminal_appearance,
            cx,
        );

        // Build a per-window view for every shared project (empty by default), then place this
        // window's sessions into their project's layout so adopted terminals show up as tabs.
        let projects = project_store.read(cx).projects().to_vec();
        let mut project_views: HashMap<ProjectId, WindowProjectView> = projects
            .iter()
            .map(|project| (project.id, WindowProjectView::empty()))
            .collect();
        for session in &sessions {
            if let Some(view) = project_views.get_mut(&session.project_id) {
                let group_id = view.layout.first_group_id();
                if let Some(group) = view.layout.find_group_mut(group_id) {
                    group.add_item(TabItem::terminal(session.id, session.title.clone()));
                }
                view.active_group_id = group_id;
            }
        }

        for session in &sessions {
            if let Err(error) = client.request(ClientRequest::SetAppearance {
                session_id: session.id,
                appearance: terminal_appearance,
            }) {
                eprintln!("failed to set terminal appearance: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetProgressTimeouts {
                session_id: session.id,
                timeouts: progress_timeouts,
            }) {
                eprintln!("failed to set terminal progress timeouts: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetOscPolicy {
                session_id: session.id,
                allow_clipboard_read: config.allow_osc_clipboard_read,
            }) {
                eprintln!("failed to set terminal OSC policy: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetUrlDetection {
                session_id: session.id,
                detect_urls: config.detect_urls,
            }) {
                eprintln!("failed to set terminal URL detection: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetCursorStyle {
                session_id: session.id,
                shape: config.cursor_shape.to_protocol(),
            }) {
                eprintln!("failed to set terminal cursor style: {error:#}");
            }
            // Pre-existing sessions adopted from the daemon were spawned with the built-in default;
            // align them to the configured scrollback (new sessions bake it in at CreateSession).
            if let Err(error) = client.request(ClientRequest::SetScrollbackLimit {
                session_id: session.id,
                limit: config.scrollback_lines,
            }) {
                eprintln!("failed to set terminal scrollback limit: {error:#}");
            }
        }
        let snapshots = sessions
            .iter()
            .filter_map(|session| {
                match client.request(ClientRequest::Snapshot {
                    session_id: session.id,
                }) {
                    Ok(DaemonResponse::Snapshot { snapshot }) => Some((session.id, snapshot)),
                    _ => None,
                }
            })
            .collect::<HashMap<_, _>>();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        // Update state changes (found/downloading/ready) re-render the sidebar indicator.
        cx.observe(&updates, |_, _, cx| cx.notify()).detach();
        // Cross-window project sync: when any window mutates the shared store, reconcile this
        // window's per-project views (add/drop) and re-render.
        cx.observe(&project_store, |app, _, cx| {
            app.reconcile_project_views(cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        let terminal_font_families = config.resolved_font_families();
        let terminal_font_features: std::sync::Arc<[crate::settings::FontFeature]> =
            config.resolved_font_features().into();
        let terminal_font_variations: std::sync::Arc<[crate::settings::FontVariation]> =
            config.resolved_font_variations().into();
        let terminal_font_codepoint_map: std::sync::Arc<[crate::settings::CodepointMapEntry]> =
            config.resolved_codepoint_map().into();
        let mut app = Self {
            input_sender: client
                .input_sender()
                .expect("failed to start terminal input worker"),
            input_latency: InputLatencyTracker::from_environment(),
            client,
            settings,
            updates,
            colors,
            terminal_theme,
            terminal_appearance,
            terminal_font_family: config.font_family.into(),
            terminal_font_size: config.font_size,
            terminal_padding_x: config.terminal_padding_x,
            terminal_padding_y: config.terminal_padding_y,
            terminal_minimum_contrast: config.minimum_contrast,
            terminal_metric_adjustments: config.font_metrics.resolve(),
            terminal_font_families,
            terminal_font_features,
            terminal_ligatures: config.ligatures,
            terminal_shaping_break_cursor: config.font_shaping_break_cursor,
            terminal_font_variations,
            terminal_font_thicken: config.font_thicken.then_some(config.font_thicken_strength),
            terminal_font_codepoint_map,
            terminal_renderer: MetalTerminalRenderer::default(),
            terminal_images_in_flight: HashSet::new(),
            terminal_frame_scheduled: false,
            progress_frame_scheduled: false,
            progress_animation_scheduled: false,
            progress_animation_epoch: Instant::now(),
            bell_flash: HashMap::new(),
            bell_flash_scheduled: false,
            cursor_blink_epoch: Instant::now(),
            cursor_blink_scheduled: false,
            progress_timeouts,
            snapshot_watchers: HashSet::new(),
            progress_watchers: HashSet::new(),
            osc_watchers: HashSet::new(),
            progress_updates: HashMap::new(),
            pending_progress_updates: HashMap::new(),
            window_id,
            project_store,
            projects,
            project_views,
            active_project,
            collapsed_projects: HashSet::new(),
            sessions,
            detached_sessions: Vec::new(),
            detached_popover_open: false,
            detached_toggle_bounds: None,
            update_popover_open: false,
            update_toggle_bounds: None,
            open_with_dropdown: None,
            open_with_dropdown_bounds: HashMap::new(),
            snapshots,
            terminal_sizes: HashMap::new(),
            terminal_resize_in_flight: HashMap::new(),
            terminal_viewports: HashMap::new(),
            terminal_selection_drag: None,
            terminal_search: None,
            command_palette: None,
            hyperlink_mouse_down: false,
            terminal_hovered_link: None,
            terminal_has_focus: false,
            terminal_focused_session: None,
            terminal_ime_states: HashMap::new(),
            session_inspections: HashMap::new(),
            session_inspection_errors: HashMap::new(),
            process_metrics_system: System::new(),
            right_tab: RightTab::Info,
            left_sidebar_width: LEFT_SIDEBAR_DEFAULT_WIDTH,
            right_sidebar_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            resizing_sidebar: None,
            resizing_split: None,
            split_bounds: Rc::new(RefCell::new(HashMap::new())),
            dragging_scrollbar: None,
            scrollbar_bounds: Rc::new(RefCell::new(HashMap::new())),
            moving_window: false,
            tab_drop_target: None,
            right_sidebar_scroll_handle: ScrollHandle::new(),
            tab_bar_scroll_handles: RefCell::new(HashMap::new()),
            focus_handle: cx.focus_handle(),
            notification_routes,
            allow_osc_clipboard_read: config.allow_osc_clipboard_read,
            detect_urls: config.detect_urls,
            copy_on_select: config.copy_on_select,
            cursor_shape: config.cursor_shape,
            scrollback_limit: config.scrollback_lines,
            cursor_blink: config.cursor_blink,
            language: config.language,
            closing: false,
            poll_error: None,
        };
        let visible_snapshots = app
            .visible_active_session_ids()
            .into_iter()
            .filter_map(|session_id| app.snapshots.get(&session_id).cloned())
            .collect::<Vec<_>>();
        for snapshot in visible_snapshots {
            app.schedule_terminal_images(&snapshot, cx);
        }
        app.start_polling(cx);
        app.start_inspection_polling(cx);
        app.start_latency_reporting(cx);
        app.install_close_confirmation(window, cx);
        cx.on_focus_in(&app.focus_handle, window, |app, _, cx| {
            app.set_terminal_has_focus(true);
            cx.notify();
        })
        .detach();
        cx.on_focus_out(&app.focus_handle, window, |app, _, _, cx| {
            app.set_terminal_has_focus(false);
            cx.notify();
        })
        .detach();
        app.focus_handle.focus(window, cx);
        app.set_terminal_has_focus(true);
        app
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.ensure_snapshot_watchers(cx);
        self.start_session_polling(cx);
        self.ensure_progress_watchers(cx);
        self.ensure_osc_watchers(cx);
    }

    fn ensure_progress_watchers(&mut self, cx: &mut Context<Self>) {
        let live_sessions = self
            .window_session_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        self.progress_updates
            .retain(|session_id, _| live_sessions.contains(session_id));
        self.pending_progress_updates
            .retain(|session_id, _| live_sessions.contains(session_id));
        self.progress_watchers
            .retain(|session_id| live_sessions.contains(session_id));

        for session_id in live_sessions {
            if !self.progress_watchers.insert(session_id) {
                continue;
            }
            self.start_progress_watching(session_id, cx);
        }
    }

    fn ensure_osc_watchers(&mut self, cx: &mut Context<Self>) {
        let live_sessions = self
            .window_session_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        self.osc_watchers
            .retain(|session_id| live_sessions.contains(session_id));
        for session_id in live_sessions {
            if self.osc_watchers.insert(session_id) {
                self.start_osc_watching(session_id, cx);
            }
        }
    }

    fn start_osc_watching(&self, session_id: SessionId, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection = None;
            let mut after_revision = 0;
            loop {
                let session_exists = this
                    .update(cx, |app, _| {
                        app.sessions.iter().any(|session| session.id == session_id)
                    })
                    .unwrap_or(false);
                if !session_exists {
                    break;
                }

                let request_client = client.clone();
                let (next_connection, result) = executor
                    .spawn(async move {
                        let mut connected = match connection.take() {
                            Some(connected) => connected,
                            None => match request_client.connect() {
                                Ok(connected) => connected,
                                Err(error) => return (None, Err(error)),
                            },
                        };
                        let result = connected.request(ClientRequest::WaitForOscEvents {
                            session_id,
                            after_revision,
                            timeout_ms: OSC_EVENT_WAIT_TIMEOUT.as_millis() as u32,
                        });
                        if result.is_ok() {
                            (Some(connected), result)
                        } else {
                            (None, result)
                        }
                    })
                    .await;
                connection = next_connection;
                let failed = result.is_err();
                let update = this.update_in(cx, |app, window, cx| match result {
                    Ok(DaemonResponse::OscEvents { update }) if update.session_id == session_id => {
                        after_revision = update.revision;
                        for event in update.events {
                            app.handle_osc_event(session_id, event.payload, window, cx);
                        }
                    }
                    Ok(DaemonResponse::OscEventsUnchanged { revision, .. }) => {
                        after_revision = after_revision.max(revision);
                    }
                    Ok(response) => {
                        eprintln!("unexpected OSC event response for {session_id}: {response:?}");
                    }
                    Err(error) => {
                        eprintln!("OSC event polling failed for {session_id}: {error:#}");
                    }
                });
                if update.is_err() {
                    break;
                }
                if failed {
                    executor.timer(Duration::from_millis(25)).await;
                }
            }
        })
        .detach();
    }

    fn handle_osc_event(
        &mut self,
        session_id: SessionId,
        payload: TerminalOscEventPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match payload {
            TerminalOscEventPayload::Notification { notification } => {
                if window.is_window_active() {
                    let client = self.client.clone();
                    let notification_id = notification.id;
                    cx.background_executor()
                        .spawn(async move {
                            let _ = client.request(ClientRequest::NotificationResponse {
                                session_id,
                                notification_id,
                                activated: false,
                            });
                        })
                        .detach();
                    return;
                }
                let tag = notification_tag(session_id, &notification.id);
                self.notification_routes.borrow_mut().insert(
                    tag.clone(),
                    NotificationRoute {
                        app: cx.weak_entity(),
                        client: self.client.clone(),
                        session_id,
                        notification_id: notification.id.clone(),
                    },
                );
                let actions = notification
                    .close_response_requested
                    .then(|| SystemNotificationAction {
                        id: "close".into(),
                        label: self.language.dismiss().into(),
                    })
                    .into_iter()
                    .collect();
                cx.show_system_notification(SystemNotification {
                    tag: tag.into(),
                    title: notification.title.into(),
                    body: notification.body.into(),
                    actions,
                });
            }
            TerminalOscEventPayload::NotificationClose { id } => {
                let tag = notification_tag(session_id, &id);
                cx.dismiss_system_notification(&tag);
                self.notification_routes.borrow_mut().remove(&tag);
            }
            TerminalOscEventPayload::ClipboardWrite {
                selection,
                contents,
            } => {
                let item = clipboard_item_from_contents(contents);
                if let Some(item) = item {
                    match selection {
                        TerminalClipboardSelection::Clipboard => cx.write_to_clipboard(item),
                        TerminalClipboardSelection::Primary => {
                            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                            cx.write_to_primary(item);
                            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                            cx.write_to_clipboard(item);
                        }
                    }
                }
            }
            TerminalOscEventPayload::ClipboardRead {
                request_id,
                selection,
                mime_types,
            } => {
                let item = match selection {
                    TerminalClipboardSelection::Clipboard => cx.read_from_clipboard(),
                    TerminalClipboardSelection::Primary => {
                        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                        {
                            cx.read_from_primary()
                        }
                        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                        {
                            cx.read_from_clipboard()
                        }
                    }
                };
                let contents = item
                    .map(|item| clipboard_contents_from_item(item, &mime_types))
                    .unwrap_or_default();
                let client = self.client.clone();
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = client.request(ClientRequest::CompleteClipboardRead {
                            session_id,
                            request_id,
                            contents,
                        }) {
                            eprintln!("failed to complete OSC clipboard read: {error:#}");
                        }
                    })
                    .detach();
            }
            TerminalOscEventPayload::OpenUrl { url, .. } => {
                let language = self.language;
                let detail = format!("{}\n{url}", language.url_open_detail());
                let prompt = window.prompt(
                    PromptLevel::Warning,
                    language.allow_url_open_title(),
                    Some(&detail),
                    &[language.open(), language.cancel()],
                    cx,
                );
                window
                    .spawn(cx, async move |cx| {
                        if matches!(prompt.await, Ok(0)) {
                            cx.update(|_, cx| cx.open_url(&url)).ok();
                        }
                    })
                    .detach();
            }
            TerminalOscEventPayload::FocusRequest => {
                cx.activate(true);
                window.activate_window();
                self.activate_session_by_id(session_id, window, cx);
            }
            TerminalOscEventPayload::Bell => {
                let bell_mode = self.settings.read(cx).config().bell_mode;
                if bell_mode.plays_sound() {
                    window.play_system_bell();
                }
                if bell_mode.flashes() {
                    self.bell_flash.insert(session_id, Instant::now());
                    self.ensure_bell_flash(cx);
                    // Draw attention to a background window so a bell is not missed.
                    if !window.is_window_active() {
                        window.request_attention();
                    }
                    cx.notify();
                }
            }
            TerminalOscEventPayload::AttentionRequest { request } => {
                if request != TerminalAttentionRequest::Cancel && !window.is_window_active() {
                    window.request_attention();
                }
            }
            TerminalOscEventPayload::FileTransfer { offer } => {
                let directory = self
                    .sessions
                    .iter()
                    .find(|session| session.id == session_id)
                    .map(|session| {
                        if session
                            .reported_location
                            .as_ref()
                            .is_none_or(|location| location.local)
                        {
                            session.current_directory.clone()
                        } else {
                            session.initial_directory.clone()
                        }
                    })
                    .unwrap_or_else(|| PathBuf::from("."));
                let client = self.client.clone();
                if offer.directory {
                    let picker = cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some(self.language.choose_receive_files_prompt().to_owned().into()),
                    });
                    window
                        .spawn(cx, async move |_| {
                            let destination = picker
                                .await
                                .ok()
                                .and_then(Result::ok)
                                .flatten()
                                .and_then(|mut paths| paths.pop());
                            if let Err(error) =
                                client.request(ClientRequest::CompleteFileTransfer {
                                    session_id,
                                    request_id: offer.request_id,
                                    destination,
                                })
                            {
                                eprintln!("failed to complete terminal file transfer: {error:#}");
                            }
                        })
                        .detach();
                } else {
                    let picker = cx.prompt_for_new_path(&directory, Some(&offer.suggested_name));
                    window
                        .spawn(cx, async move |_| {
                            let destination = picker.await.ok().and_then(Result::ok).flatten();
                            if let Err(error) =
                                client.request(ClientRequest::CompleteFileTransfer {
                                    session_id,
                                    request_id: offer.request_id,
                                    destination,
                                })
                            {
                                eprintln!("failed to complete terminal file transfer: {error:#}");
                            }
                        })
                        .detach();
                }
            }
        }
    }

    fn start_progress_watching(&self, session_id: SessionId, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection = None;
            let mut after_revision = 0;
            loop {
                let session_exists = this
                    .update(cx, |app, _| {
                        app.sessions.iter().any(|session| session.id == session_id)
                    })
                    .unwrap_or(false);
                if !session_exists {
                    break;
                }

                let request_client = client.clone();
                let (next_connection, result) = executor
                    .spawn(async move {
                        let mut connected = match connection.take() {
                            Some(connected) => connected,
                            None => match request_client.connect() {
                                Ok(connected) => connected,
                                Err(error) => return (None, Err(error)),
                            },
                        };
                        let result = connected.request(ClientRequest::WaitForProgress {
                            session_id,
                            after_revision,
                            timeout_ms: PROGRESS_WAIT_TIMEOUT.as_millis() as u32,
                        });
                        if result.is_ok() {
                            (Some(connected), result)
                        } else {
                            (None, result)
                        }
                    })
                    .await;
                connection = next_connection;
                let request_failed = result.is_err();
                let update = this.update_in(cx, |app, window, cx| match result {
                    Ok(DaemonResponse::Progress { update }) if update.session_id == session_id => {
                        after_revision = update.revision;
                        let should_replace = app
                            .pending_progress_updates
                            .get(&session_id)
                            .or_else(|| app.progress_updates.get(&session_id))
                            .is_none_or(|current| current.revision < update.revision);
                        if should_replace {
                            app.pending_progress_updates.insert(session_id, update);
                            app.schedule_progress_frame(window, cx);
                        }
                    }
                    Ok(DaemonResponse::ProgressUnchanged { revision, .. }) => {
                        after_revision = after_revision.max(revision);
                    }
                    Ok(response) => {
                        eprintln!("unexpected progress response for {session_id}: {response:?}");
                    }
                    Err(error) => {
                        eprintln!("progress polling failed for {session_id}: {error:#}");
                    }
                });
                if update.is_err() {
                    break;
                }
                if request_failed {
                    executor.timer(Duration::from_millis(25)).await;
                }
            }
        })
        .detach();
    }

    fn schedule_progress_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.progress_frame_scheduled {
            return;
        }
        self.progress_frame_scheduled = true;
        cx.on_next_frame(window, |app, window, cx| {
            app.progress_frame_scheduled = false;
            for (session_id, update) in app.pending_progress_updates.drain() {
                app.progress_updates.insert(session_id, update);
            }
            app.ensure_progress_animation(window, cx);
            cx.notify();
        });
        window.refresh();
    }

    fn ensure_progress_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_indeterminate = self.progress_updates.values().any(|update| {
            update
                .progress
                .is_some_and(|progress| progress.state == TerminalProgressState::Indeterminate)
        });
        if !has_indeterminate || self.progress_animation_scheduled {
            return;
        }

        self.progress_animation_scheduled = true;
        cx.on_next_frame(window, |app, window, cx| {
            app.progress_animation_scheduled = false;
            let has_indeterminate = app.progress_updates.values().any(|update| {
                update
                    .progress
                    .is_some_and(|progress| progress.state == TerminalProgressState::Indeterminate)
            });
            if has_indeterminate {
                cx.notify();
                app.ensure_progress_animation(window, cx);
            }
        });
        // GPUI's macOS display link stops while the window is minimized or fully occluded, so this
        // animation naturally suspends without a background timer waking hidden windows.
        window.refresh();
    }

    /// Run the bell-flash animation: a background timer that repaints while any session is within
    /// its pulse window, expiring each entry after [`BELL_FLASH_DURATION`]. Uses a background timer
    /// (not the display link) so the pulse still completes when the window is not frontmost.
    fn ensure_bell_flash(&mut self, cx: &mut Context<Self>) {
        if self.bell_flash_scheduled || self.bell_flash.is_empty() {
            return;
        }
        self.bell_flash_scheduled = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(BELL_FLASH_STEP).await;
                let keep_going = this
                    .update(cx, |app, cx| {
                        app.bell_flash
                            .retain(|_, started| started.elapsed() < BELL_FLASH_DURATION);
                        if app.bell_flash.is_empty() {
                            app.bell_flash_scheduled = false;
                            false
                        } else {
                            cx.notify();
                            true
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// The accent-tint color for a session's pulsing tab this frame, or `None` if the pulse is over.
    /// A single pulse: alpha eases from 0 up to [`BELL_FLASH_PEAK_ALPHA`] at the midpoint, then back
    /// to 0, using a raised-cosine so the fill and restore are continuous and smooth.
    fn bell_flash_tint(&self, session_id: SessionId) -> Option<u32> {
        let started = self.bell_flash.get(&session_id)?;
        let elapsed = started.elapsed();
        if elapsed >= BELL_FLASH_DURATION {
            return None;
        }
        let progress = elapsed.as_secs_f32() / BELL_FLASH_DURATION.as_secs_f32();
        // Raised cosine: 0 -> 1 -> 0 across the pulse, with zero slope at both ends.
        let intensity = 0.5 - 0.5 * (progress * std::f32::consts::TAU).cos();
        let alpha = (intensity * BELL_FLASH_PEAK_ALPHA).round() as u32;
        Some((self.colors.accent << 8) | alpha.min(0xff))
    }

    /// Whether the given session's cursor should blink this session, per the user's `cursor_blink`
    /// setting resolved against the program-reported blinking bit. A hidden cursor never blinks.
    fn cursor_should_blink(&self, snapshot: &TerminalSnapshot) -> bool {
        snapshot.cursor_shape != TerminalCursorShape::Hidden
            && self.cursor_blink.resolve(snapshot.cursor_blinking)
    }

    /// Whether the cursor should be painted this frame: solid unless it is blinking and currently in
    /// the "off" half of the blink cycle.
    fn cursor_visible_this_frame(&self, snapshot: &TerminalSnapshot) -> bool {
        if !self.cursor_should_blink(snapshot) {
            return true;
        }
        let elapsed = self.cursor_blink_epoch.elapsed().as_secs_f32();
        elapsed.rem_euclid(CURSOR_BLINK_PERIOD.as_secs_f32()) < CURSOR_BLINK_PERIOD.as_secs_f32() / 2.
    }

    /// Whether the active session's cursor is blinking right now (drives the blink timer loop).
    fn active_cursor_blinks(&self) -> bool {
        self.active_session_id()
            .and_then(|session_id| self.snapshots.get(&session_id))
            .is_some_and(|snapshot| self.cursor_should_blink(snapshot))
    }

    /// Drive the cursor blink: a background timer that repaints every half-cycle while the active
    /// session's cursor is blinking, and exits as soon as it stops (so an idle, non-blinking cursor
    /// costs nothing). Uses a background timer (not the display link) so it ticks regardless of
    /// frontmost state, mirroring [`Self::ensure_bell_flash`].
    fn ensure_cursor_blink(&mut self, cx: &mut Context<Self>) {
        if self.cursor_blink_scheduled || !self.active_cursor_blinks() {
            return;
        }
        self.cursor_blink_scheduled = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(CURSOR_BLINK_PERIOD / 2).await;
                let keep_going = this
                    .update(cx, |app, cx| {
                        if app.active_cursor_blinks() {
                            cx.notify();
                            true
                        } else {
                            app.cursor_blink_scheduled = false;
                            false
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    fn render_item_icon(
        &self,
        kind: ItemKind,
        session_id: Option<SessionId>,
        element_id: String,
    ) -> AnyElement {
        let Some(progress) = session_id
            .and_then(|session_id| self.progress_updates.get(&session_id))
            .and_then(|update| update.progress)
        else {
            return div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(ITEM_ICON_SLOT_SIZE))
                .h(px(ITEM_ICON_SLOT_SIZE))
                .child(icon(item_kind_icon(kind)))
                .into_any_element();
        };

        let colors = self.colors;
        let color = match progress.state {
            TerminalProgressState::Error => self.terminal_theme.palette[1],
            TerminalProgressState::Paused => self.terminal_theme.palette[3],
            TerminalProgressState::Normal | TerminalProgressState::Indeterminate => colors.accent,
        };
        let phase = (self.progress_animation_epoch.elapsed().as_secs_f32() * 0.8).fract();
        let language = self.language;
        let accessibility_label: SharedString = progress_label(progress, language).into();
        let tooltip_progress = progress;
        div()
            .id(element_id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(ITEM_ICON_SLOT_SIZE))
            .h(px(ITEM_ICON_SLOT_SIZE))
            .role(Role::Image)
            .aria_label(accessibility_label)
            .tooltip(move |_, cx| {
                cx.new(|_| ProgressTooltip {
                    text: progress_label(tooltip_progress, language).into(),
                    colors,
                })
                .into()
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        paint_progress_ring(bounds, progress, phase, color, colors.border, window)
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }

    fn schedule_terminal_images(&mut self, snapshot: &TerminalSnapshot, cx: &mut Context<Self>) {
        self.terminal_renderer.retain_snapshot_images(snapshot);
        for descriptor in &snapshot.images {
            let key = TerminalTextureKey {
                session_id: snapshot.session_id,
                image: descriptor.key,
            };
            let stream_key = TerminalImageStreamKey {
                session_id: snapshot.session_id,
                image_id: descriptor.key.id,
            };
            if self
                .terminal_renderer
                .has_image(snapshot.session_id, descriptor.key)
                || !self.terminal_images_in_flight.insert(stream_key)
            {
                continue;
            }
            let client = self.client.clone();
            let descriptor = descriptor.clone();
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                let result = executor
                    .spawn(async move { fetch_terminal_image(client, key.session_id, descriptor) })
                    .await;
                let _ = this.update_in(cx, |app, window, cx| {
                    app.terminal_images_in_flight.remove(&stream_key);
                    match result {
                        Ok(image) => {
                            let still_live =
                                app.snapshots.get(&key.session_id).is_some_and(|snapshot| {
                                    snapshot
                                        .images
                                        .iter()
                                        .any(|descriptor| descriptor.key == key.image)
                                });
                            if still_live {
                                app.terminal_renderer.install_image(image);
                            }
                            // Start the newest generation immediately after this stream becomes
                            // available. Waiting for the next platform frame here adds a full vsync
                            // of idle time per transfer and caps image animations below the PTY's
                            // production rate even when the raw Unix-socket copy finished early.
                            if let Some(snapshot) = app.snapshots.get(&key.session_id).cloned() {
                                app.schedule_terminal_images(&snapshot, cx);
                            }
                            // Even when this transfer was superseded, present the newest complete
                            // snapshot so a fast animation cannot leave the renderer waiting on an
                            // image generation which will never be presented.
                            app.schedule_terminal_frame(window, cx);
                        }
                        Err(error) => {
                            eprintln!(
                                "failed to fetch terminal image {} generation {}: {error:#}",
                                key.image.id, key.image.generation
                            );
                        }
                    }
                });
            })
            .detach();
        }
    }

    /// Coalesce terminal revisions and image arrivals onto the platform frame boundary.
    ///
    /// The daemon can publish faster than the display. Invalidating immediately for every IPC
    /// response makes GPUI start redundant draws and can deliver another revision while the
    /// previous frame is still being encoded. Keep applying deltas so the newest state is ready,
    /// but invalidate the view at most once per animation frame.
    fn schedule_terminal_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_frame_scheduled {
            return;
        }
        self.terminal_frame_scheduled = true;
        cx.on_next_frame(window, |app, _window, cx| {
            app.terminal_frame_scheduled = false;
            let visible_snapshots = app
                .visible_active_session_ids()
                .into_iter()
                .filter_map(|session_id| app.snapshots.get(&session_id).cloned())
                .collect::<Vec<_>>();
            let mut images_ready = true;
            for snapshot in visible_snapshots {
                images_ready &= snapshot.images.iter().all(|image| {
                    app.terminal_renderer
                        .has_image(snapshot.session_id, image.key)
                });
                app.schedule_terminal_images(&snapshot, cx);
            }
            if !images_ready {
                // Keep the last complete frame visible. Painting any visible terminal before its
                // image transfer finishes creates a blank intermediate frame on Kitty animation
                // ticks, which presents as severe flashing in notcurses xray.
                return;
            }
            cx.notify();
        });
        window.refresh();
    }

    fn start_latency_reporting(&self, cx: &mut Context<Self>) {
        if !self.input_latency.is_enabled() {
            return;
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_secs(1)).await;
                if this
                    .update_in(cx, |app, window, _| {
                        app.input_latency
                            .report_if_due(&window.input_latency_snapshot());
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn ensure_snapshot_watchers(&mut self, cx: &mut Context<Self>) {
        for session_id in self.visible_active_session_ids() {
            if let Some(snapshot) = self.snapshots.get(&session_id).cloned() {
                self.schedule_terminal_images(&snapshot, cx);
            }
            if self.snapshot_watchers.insert(session_id) {
                self.start_snapshot_watching(session_id, cx);
            }
        }
    }

    fn start_snapshot_watching(&self, session_id: SessionId, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection = None;
            while let Some(after_revision) = this
                .update(cx, |app, _| {
                    app.is_visible_active_session(session_id).then(|| {
                        app.snapshots
                            .get(&session_id)
                            .map_or(0, |snapshot| snapshot.revision)
                    })
                })
                .ok()
                .flatten()
            {
                let request_client = client.clone();
                let (next_connection, result) = executor
                    .spawn(async move {
                        let mut connected = match connection.take() {
                            Some(connected) => connected,
                            None => match request_client.connect() {
                                Ok(connected) => connected,
                                Err(error) => return (None, Err(error)),
                            },
                        };
                        let result = connected.request(ClientRequest::WaitForSnapshot {
                            session_id,
                            after_revision,
                            timeout_ms: SNAPSHOT_WAIT_TIMEOUT.as_millis() as u32,
                        });
                        if result.is_ok() {
                            (Some(connected), result)
                        } else {
                            (None, result)
                        }
                    })
                    .await;
                connection = next_connection;
                let request_failed = result.is_err();
                let update = this.update_in(cx, |app, window, cx| {
                    let mut changed = false;
                    match result {
                        Ok(DaemonResponse::Snapshot { snapshot })
                            if snapshot.session_id == session_id =>
                        {
                            if app.poll_error.take().is_some() {
                                changed = true;
                            }
                            let snapshot_changed = app
                                .snapshots
                                .get(&snapshot.session_id)
                                .is_none_or(|previous| previous.revision != snapshot.revision);
                            if snapshot_changed {
                                app.synchronize_session_title(snapshot.session_id, &snapshot.title);
                                app.input_latency.record_snapshot(
                                    snapshot.session_id,
                                    snapshot.last_input_sequence,
                                    snapshot.revision,
                                );
                                app.snapshots
                                    .insert(snapshot.session_id, Arc::clone(&snapshot));
                                changed = true;
                            }
                        }
                        Ok(DaemonResponse::SnapshotDelta { delta })
                            if delta.session_id == session_id =>
                        {
                            app.poll_error.take();
                            let snapshot = app
                                .snapshots
                                .get(&delta.session_id)
                                .and_then(|snapshot| snapshot.apply_delta(&delta));
                            if let Some(snapshot) = snapshot {
                                app.synchronize_session_title(snapshot.session_id, &snapshot.title);
                                app.input_latency.record_snapshot(
                                    snapshot.session_id,
                                    snapshot.last_input_sequence,
                                    snapshot.revision,
                                );
                                let snapshot = Arc::new(snapshot);
                                app.snapshots
                                    .insert(snapshot.session_id, Arc::clone(&snapshot));
                                changed = true;
                            } else {
                                // Dropping the stale base makes the next request ask for a full
                                // frame instead of retrying an inapplicable delta indefinitely.
                                app.snapshots.remove(&delta.session_id);
                                app.poll_error = Some(format!(
                                    "snapshot delta base {} is unavailable",
                                    delta.base_revision
                                ));
                                changed = true;
                            }
                        }
                        Ok(DaemonResponse::SnapshotUnchanged { .. }) => {
                            if app.poll_error.take().is_some() {
                                changed = true;
                            }
                        }
                        Ok(response) => {
                            let message = format!("unexpected snapshot response: {response:?}");
                            eprintln!("{message}");
                            if app.poll_error.as_ref() != Some(&message) {
                                app.poll_error = Some(message);
                                changed = true;
                            }
                        }
                        Err(error) => {
                            let message = format!("snapshot polling failed: {error:#}");
                            eprintln!("{message}");
                            if app.poll_error.as_ref() != Some(&message) {
                                app.poll_error = Some(message);
                                changed = true;
                            }
                        }
                    }
                    if changed && app.is_visible_active_session(session_id) {
                        app.schedule_terminal_frame(window, cx);
                    }
                });
                if update.is_err() {
                    eprintln!("poll update lost its window");
                    break;
                }
                if request_failed {
                    executor.timer(Duration::from_millis(25)).await;
                }
            }
            let _ = this.update(cx, |app, cx| {
                app.snapshot_watchers.remove(&session_id);
                // The session can become visible again between the loop's visibility check and
                // cleanup. Reconcile here so that transition cannot leave it without a watcher.
                if app.is_visible_active_session(session_id) {
                    app.ensure_snapshot_watchers(cx);
                }
            });
        })
        .detach();
    }

    fn start_session_polling(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let request_client = client.clone();
                let result = executor
                    .spawn(async move { request_client.request(ClientRequest::ListSessions) })
                    .await;
                let update = this.update_in(cx, |app, _window, cx| match result {
                    Ok(DaemonResponse::Sessions { sessions }) => {
                        app.apply_session_list(sessions, cx);
                    }
                    Ok(response) => eprintln!("unexpected sessions response: {response:?}"),
                    Err(error) => eprintln!("session polling failed: {error:#}"),
                });
                if update.is_err() {
                    eprintln!("session polling lost its window");
                    break;
                }
                executor.timer(SESSION_LIST_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Classify the daemon's flat session list into this window's own sessions and the detached
    /// (unowned) ones, then reconcile local state. Sessions owned by other windows are ignored.
    ///
    /// A session this window previously owned can vanish from its own set here — either terminated
    /// elsewhere, or (R10) claimed by another window between polls. In that case its tabs are pruned
    /// from every project layout so the window never renders a terminal the daemon no longer assigns
    /// to it.
    fn apply_session_list(&mut self, sessions: Vec<SessionSummary>, cx: &mut Context<Self>) {
        let mut owned = Vec::new();
        let mut detached = Vec::new();
        for session in sessions {
            match session.window_id {
                Some(window_id) if window_id == self.window_id => owned.push(session),
                None => detached.push(session),
                Some(_) => {} // owned by another window — not visible here
            }
        }

        let mut changed = false;
        for session in &owned {
            changed |= self.synchronize_session_title(session.id, &session.title);
        }

        // Prune tabs for sessions this window no longer owns (terminated or claimed away).
        let owned_ids: HashSet<SessionId> = owned.iter().map(|session| session.id).collect();
        let vanished: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|session| !owned_ids.contains(&session.id))
            .map(|session| session.id)
            .collect();
        for session_id in vanished {
            changed |= self.prune_session_from_layouts(session_id);
        }

        if self.sessions != owned {
            self.sessions = owned;
            changed = true;
        }
        if self.detached_sessions != detached {
            self.detached_sessions = detached;
            changed = true;
        }

        if changed {
            self.ensure_snapshot_watchers(cx);
            self.ensure_progress_watchers(cx);
            self.ensure_osc_watchers(cx);
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    /// Remove every tab backed by `session_id` from all project layouts, dropping any per-session
    /// caches. Returns whether anything changed. Used when a session leaves this window's ownership.
    fn prune_session_from_layouts(&mut self, session_id: SessionId) -> bool {
        let mut changed = false;
        // Collect (project_id, group_id, item_id) for every tab of this session across all views.
        let targets: Vec<(ProjectId, GroupId, ItemId)> = self
            .project_views
            .iter()
            .filter_map(|(project_id, view)| {
                view.layout
                    .find_terminal_item(session_id)
                    .map(|(group_id, item_id)| (*project_id, group_id, item_id))
            })
            .collect();
        for (project_id, group_id, item_id) in targets {
            if let Some(view) = self.project_views.get_mut(&project_id) {
                view.layout.close_item(group_id, item_id);
                if view.layout.find_group(group_id).is_none() {
                    view.active_group_id = view.layout.first_group_id();
                }
                changed = true;
            }
        }
        if changed {
            if self.terminal_focused_session == Some(session_id) {
                self.terminal_focused_session = None;
            }
            self.snapshots.remove(&session_id);
            self.terminal_sizes.remove(&session_id);
            self.terminal_resize_in_flight.remove(&session_id);
            self.terminal_ime_states.remove(&session_id);
            self.session_inspections.remove(&session_id);
            self.session_inspection_errors.remove(&session_id);
        }
        changed
    }

    fn start_inspection_polling(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let session_id = this
                    .update(cx, |app, _| app.active_session_id())
                    .ok()
                    .flatten();
                if let Some(session_id) = session_id {
                    let request_client = client.clone();
                    let result = executor
                        .spawn(async move {
                            request_client.request(ClientRequest::InspectSession { session_id })
                        })
                        .await;
                    let update = this.update_in(cx, |app, _window, cx| match result {
                        Ok(DaemonResponse::SessionInspection { mut inspection }) => {
                            app.fill_missing_process_metrics(&mut inspection);
                            let error_cleared =
                                app.session_inspection_errors.remove(&session_id).is_some();
                            let changed = error_cleared
                                || app.session_inspections.get(&session_id) != Some(&inspection);
                            if changed {
                                app.session_inspections.insert(session_id, inspection);
                                cx.notify();
                            }
                        }
                        Ok(response) => {
                            let message =
                                format!("unexpected session-inspection response: {response:?}");
                            eprintln!("{message}");
                            app.session_inspection_errors.insert(session_id, message);
                            cx.notify();
                        }
                        Err(error) => {
                            let message = format!("session inspection failed: {error:#}");
                            eprintln!("{message}");
                            app.session_inspection_errors.insert(session_id, message);
                            cx.notify();
                        }
                    });
                    if update.is_err() {
                        eprintln!("session-inspection update lost its window");
                        break;
                    }
                }
                executor.timer(SESSION_INSPECTION_INTERVAL).await;
            }
        })
        .detach();
    }

    fn fill_missing_process_metrics(&mut self, inspection: &mut SessionInspection) {
        let pids = inspection
            .processes
            .iter()
            .filter(|process| {
                process.cpu_usage_tenths_percent.is_none() || process.memory_bytes.is_none()
            })
            .map(|process| Pid::from_u32(process.pid))
            .collect::<Vec<_>>();
        if pids.is_empty() {
            return;
        }

        self.process_metrics_system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );
        for process in &mut inspection.processes {
            let Some(sample) = self
                .process_metrics_system
                .process(Pid::from_u32(process.pid))
            else {
                continue;
            };
            process
                .cpu_usage_tenths_percent
                .get_or_insert_with(|| cpu_usage_tenths_percent(sample.cpu_usage()));
            process.memory_bytes.get_or_insert_with(|| sample.memory());
        }
    }

    fn install_close_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            app.update(cx, |app, cx| {
                if app.closing || app.sessions.is_empty() {
                    return true;
                }
                let language = app.language;
                let prompt = window.prompt(
                    PromptLevel::Warning,
                    language.close_window_title(),
                    Some(language.close_window_message()),
                    &[
                        language.terminate_all(),
                        language.detach(),
                        language.cancel(),
                    ],
                    cx,
                );
                let app = cx.entity().downgrade();
                cx.spawn_in(window, async move |_, cx| {
                    let Ok(choice) = prompt.await else {
                        return;
                    };
                    if choice == 2 {
                        return;
                    }
                    let _ = cx.update(|window, cx| {
                        let _ = app.update(cx, |app, _| {
                            if choice == 0 {
                                // Terminate all: kill this window's sessions.
                                for session_id in app.window_session_ids() {
                                    let _ = app
                                        .client
                                        .request(ClientRequest::Terminate { session_id });
                                }
                            } else {
                                // Detach (choice == 1): clear this window's ownership so the sessions
                                // survive as claimable, unowned terminals instead of pointing at a
                                // window that is about to be destroyed.
                                let _ = app.client.request(ClientRequest::DetachWindow {
                                    window_id: app.window_id,
                                });
                            }
                            app.closing = true;
                        });
                        window.remove_window();
                    });
                })
                .detach();
                false
            })
            .unwrap_or(true)
        });
    }

    /// The per-window view for the active project. Invariant: `active_project` always names a
    /// project that has a view (maintained by `reconcile_project_views` and the always-present Home
    /// project), so this never returns `None` in practice — matching the old direct-index behaviour.
    fn workspace(&self) -> &WindowProjectView {
        self.project_views
            .get(&self.active_project)
            .expect("active_project must always have a window view")
    }

    fn workspace_mut(&mut self) -> &mut WindowProjectView {
        self.project_views
            .get_mut(&self.active_project)
            .expect("active_project must always have a window view")
    }

    /// The shared `Project` (id/name/root) for the active project, from the local mirror.
    fn active_project_data(&self) -> &Project {
        self.projects
            .iter()
            .find(|project| project.id == self.active_project)
            .expect("active_project must exist in the project mirror")
    }

    /// Rebuild `project_views` and the local `projects` mirror to track the shared store: create an
    /// empty view for each newly-synced project, drop views for removed projects, and keep
    /// `active_project` pointing at a project that still exists.
    fn reconcile_project_views(&mut self, cx: &mut Context<Self>) {
        self.projects = self.project_store.read(cx).projects().to_vec();
        // Add views for projects that don't have one yet (synced in from another window).
        for project in &self.projects {
            self.project_views
                .entry(project.id)
                .or_insert_with(WindowProjectView::empty);
        }
        // Drop views for projects that no longer exist in the shared list.
        let live: HashSet<ProjectId> = self.projects.iter().map(|project| project.id).collect();
        self.project_views.retain(|id, _| live.contains(id));
        self.collapsed_projects.retain(|id| live.contains(id));
        // Keep the active project valid; fall back to the first project if it was removed.
        if !live.contains(&self.active_project)
            && let Some(first) = self.projects.first()
        {
            self.active_project = first.id;
        }
    }

    fn active_session_id(&self) -> Option<SessionId> {
        self.workspace()
            .layout
            .find_group(self.workspace().active_group_id)?
            .active_item()?
            .session_id
    }

    /// Returns the active terminal tab in every visible split group. This is intentionally
    /// independent from `active_group_id`: that field selects the keyboard input target, while
    /// every session returned here must keep receiving snapshots and rendering frames.
    fn visible_active_session_ids(&self) -> Vec<SessionId> {
        visible_active_terminal_sessions(&self.workspace().layout)
    }

    fn is_visible_active_session(&self, session_id: SessionId) -> bool {
        layout_has_visible_active_terminal(&self.workspace().layout, session_id)
    }

    fn set_terminal_has_focus(&mut self, focused: bool) {
        self.terminal_has_focus = focused;
        self.sync_terminal_focus();
    }

    fn sync_terminal_focus(&mut self) {
        let next_session = self
            .terminal_has_focus
            .then(|| self.active_session_id())
            .flatten();
        if self.terminal_focused_session == next_session {
            return;
        }
        if let Some(session_id) = self.terminal_focused_session.take()
            && let Err(error) = self.input_sender.send_focus(session_id, false)
        {
            eprintln!("failed to enqueue terminal focus-out: {error:#}");
        }
        if let Some(session_id) = next_session {
            if let Err(error) = self.input_sender.send_focus(session_id, true) {
                eprintln!("failed to enqueue terminal focus-in: {error:#}");
                return;
            }
            self.terminal_focused_session = Some(session_id);
        }
    }

    /// Ids of the sessions this window owns. `self.sessions` is already filtered to this window's
    /// sessions by the polling classifier, so this is just their ids.
    fn window_session_ids(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|session| session.id).collect()
    }

    /// Keeps the daemon session summary and every tab model in lockstep. The sidebar renders the
    /// former while group tab bars render the latter, so updating only one produces divergent OSC
    /// titles until the window is rebuilt.
    fn synchronize_session_title(&mut self, session_id: SessionId, title: &str) -> bool {
        let mut changed = false;
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            && session.title != title
        {
            session.title = title.to_owned();
            changed = true;
        }
        for view in self.project_views.values_mut() {
            changed |= view.layout.update_terminal_title(session_id, title);
        }
        changed
    }

    /// Claim a detached session into this window: set its owner to this window in the daemon, then
    /// place it as a tab in its project's layout. If the session's project isn't in the shared list
    /// (its project was deleted), fall back to the active project. Activates the claimed session.
    fn claim_detached_session(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(DaemonResponse::SessionClaimed { session }) =
            self.client.request(ClientRequest::ClaimSession {
                session_id,
                window_id: self.window_id,
            })
        else {
            return;
        };
        // Choose the project this session lands in: its own project if this window has a view for it,
        // otherwise the currently active project (orphaned project_id, R8).
        let target_project = if self.project_views.contains_key(&session.project_id) {
            session.project_id
        } else {
            self.active_project
        };
        if let Some(view) = self.project_views.get_mut(&target_project) {
            let group_id = view.active_group_id;
            if let Some(group) = view.layout.find_group_mut(group_id) {
                group.add_item(TabItem::terminal(session.id, session.title.clone()));
            }
        }
        self.configure_session_progress(session.id);
        self.configure_session_osc_policy(session.id);
        self.sessions.push(session);
        self.detached_sessions
            .retain(|candidate| candidate.id != session_id);
        if self.detached_sessions.is_empty() {
            self.detached_popover_open = false;
        }
        self.active_project = target_project;
        self.activate_session(target_project, session_id, window, cx);
        self.ensure_snapshot_watchers(cx);
        self.ensure_progress_watchers(cx);
        self.ensure_osc_watchers(cx);
        cx.notify();
    }

    /// Permanently terminate a detached session from the popover.
    fn destroy_detached_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let _ = self
            .client
            .request(ClientRequest::Terminate { session_id });
        self.detached_sessions
            .retain(|candidate| candidate.id != session_id);
        if self.detached_sessions.is_empty() {
            self.detached_popover_open = false;
        }
        cx.notify();
    }

    fn configure_session_progress(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetProgressTimeouts {
            session_id,
            timeouts: self.progress_timeouts,
        }) {
            eprintln!("failed to configure terminal progress for {session_id}: {error:#}");
        }
    }

    fn configure_session_osc_policy(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetOscPolicy {
            session_id,
            allow_clipboard_read: self.allow_osc_clipboard_read,
        }) {
            eprintln!("failed to configure terminal OSC policy for {session_id}: {error:#}");
        }
    }

    fn configure_session_url_detection(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetUrlDetection {
            session_id,
            detect_urls: self.detect_urls,
        }) {
            eprintln!("failed to configure terminal URL detection for {session_id}: {error:#}");
        }
    }

    fn configure_session_cursor_shape(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetCursorStyle {
            session_id,
            shape: self.cursor_shape.to_protocol(),
        }) {
            eprintln!("failed to configure terminal cursor style for {session_id}: {error:#}");
        }
    }

    fn configure_session_scrollback(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetScrollbackLimit {
            session_id,
            limit: self.scrollback_limit,
        }) {
            eprintln!("failed to configure terminal scrollback for {session_id}: {error:#}");
        }
    }

    fn new_terminal(
        &mut self,
        group_id: GroupId,
        split: Option<Direction>,
        cx: &mut Context<Self>,
    ) {
        let project = self.active_project_data().clone();
        let config = self.settings.read(cx).config();
        let scrollback_limit = config.scrollback_lines;
        let shell_program = config.shell_program.clone();
        let shell_args = config.shell_args.clone();
        let shell_features = config.shell_features_string();
        let Ok(DaemonResponse::SessionCreated { session }) =
            self.client.request(ClientRequest::CreateSession {
                project_id: project.id,
                window_id: self.window_id,
                cwd: project.effective_root(),
                size: TerminalSize::default(),
                appearance: self.terminal_appearance,
                scrollback_limit,
                shell_program,
                shell_args,
                shell_features,
            })
        else {
            return;
        };
        let item = TabItem::terminal(session.id, session.title.clone());
        let workspace = self.workspace_mut();
        if let Some(direction) = split {
            if let Some(new_group) = workspace
                .layout
                .split_group_direction(group_id, direction, item)
            {
                workspace.active_group_id = new_group;
            }
        } else if let Some(group) = workspace.layout.find_group_mut(group_id) {
            group.add_item(item);
            workspace.active_group_id = group_id;
        }
        self.configure_session_progress(session.id);
        self.configure_session_osc_policy(session.id);
        self.sessions.push(session);
        self.ensure_snapshot_watchers(cx);
        self.ensure_progress_watchers(cx);
        self.ensure_osc_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    fn prompt_add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.language;
        let receiver = prompt_for_text(
            window,
            language.new_project_title(),
            language.new_project_message(),
            Some(language.project_name_placeholder()),
            language.ok(),
            language.cancel(),
        );
        let app = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let name = receiver.await.ok()??;
                let name = name.trim();
                if name.is_empty() {
                    return None;
                }
                app.update(cx, |app, cx| app.add_project(name.to_owned(), cx))
                    .ok()?;
                Some(())
            })
            .detach();
    }

    /// Add a new project to the shared store. Per D2/D7 this does NOT spawn a terminal — the new
    /// project starts empty (an empty layout) and the first terminal is created when the user acts
    /// in it. `reconcile_project_views` (driven by the store observer) creates the empty view; here
    /// we just make the new project active in this window.
    fn add_project(&mut self, name: String, cx: &mut Context<Self>) {
        let project = Project::new(name);
        let id = project.id;
        self.project_store
            .update(cx, |store, cx| store.add(project, cx));
        // The observer reconciles views asynchronously; reconcile now so `active_project` is valid
        // immediately (the store update already notified, but our own view map lags until reconcile).
        self.reconcile_project_views(cx);
        self.active_project = id;
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    fn activate_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        if self.project_views.contains_key(&project_id) {
            self.active_project = project_id;
            self.ensure_snapshot_watchers(cx);
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    fn toggle_project_collapsed(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        if self.collapsed_projects.contains(&project_id) {
            self.collapsed_projects.remove(&project_id);
        } else {
            self.collapsed_projects.insert(project_id);
        }
        cx.notify();
    }

    fn prompt_set_project_root(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.language.set_working_directory_prompt().to_owned().into()),
        });
        let app = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let mut paths = paths.await.ok()?.ok()??;
            let root = paths.pop()?;
            app.update(cx, move |app, cx| {
                app.project_store
                    .update(cx, |store, cx| store.set_root(project_id, Some(root), cx));
            })
            .ok()?;
            Some(())
        })
        .detach();
    }

    fn show_project_context_menu(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(view) = self.project_views.get(&project_id) else {
            return;
        };
        let tab_count = view.layout.item_count();
        let Some(menu) = prepare_project_menu(window, tab_count, self.language) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            this.update(cx, move |app, cx| {
                if !app.project_views.contains_key(&project_id) {
                    return;
                }
                match command {
                    NativeProjectMenuCommand::EditName => {
                        app.prompt_rename_project(project_id, cx);
                    }
                    NativeProjectMenuCommand::SetRoot => {
                        app.prompt_set_project_root(project_id, cx);
                    }
                    NativeProjectMenuCommand::CloseTabs => {
                        app.close_project_tabs(project_id, cx);
                    }
                    NativeProjectMenuCommand::DeleteProject => {
                        app.delete_project(project_id, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn show_process_context_menu(
        &mut self,
        pid: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(menu) = prepare_process_menu(window, self.language) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            this.update(cx, move |app, cx| {
                app.handle_process_menu_command(command, pid, cx);
            })
            .ok();
        })
        .detach();
    }

    fn handle_process_menu_command(
        &mut self,
        command: NativeProcessMenuCommand,
        pid: u32,
        cx: &mut Context<Self>,
    ) {
        match command {
            NativeProcessMenuCommand::Terminate => {
                kill_process(pid, libc::SIGTERM);
            }
            NativeProcessMenuCommand::ForceKill => {
                kill_process(pid, libc::SIGKILL);
            }
            NativeProcessMenuCommand::CopyPid => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pid.to_string()));
            }
            NativeProcessMenuCommand::CopyExecutablePath => {
                if let Some(path) = executable_path_for_pid(pid) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(path));
                }
            }
        }
    }

    fn prompt_rename_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let Some(project) = self.projects.iter().find(|project| project.id == project_id) else {
            return;
        };
        let current_name = project.name.clone();
        let language = self.language;
        let Some(window) = cx.active_window() else {
            return;
        };
        let app = cx.entity().downgrade();
        window
            .update(cx, |_, window, cx| {
                let receiver = prompt_for_text(
                    window,
                    language.rename_project_title(),
                    language.rename_project_message(),
                    Some(&current_name),
                    language.ok(),
                    language.cancel(),
                );
                cx.spawn(async move |cx| {
                    let name = receiver.await.ok()??;
                    let name = name.trim();
                    if name.is_empty() {
                        return None;
                    }
                    app.update(cx, move |app, cx| {
                        app.project_store.update(cx, |store, cx| {
                            store.rename(project_id, name.to_owned(), cx)
                        });
                    })
                    .ok()?;
                    Some(())
                })
                .detach();
            })
            .ok();
    }

    fn close_project_tabs(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let Some(view) = self.project_views.get(&project_id) else {
            return;
        };
        let items_to_close: Vec<(GroupId, ItemId)> = view
            .layout
            .terminal_items()
            .filter_map(|item| {
                item.session_id
                    .and_then(|session_id| view.layout.find_terminal_item(session_id))
            })
            .collect();
        for (group_id, item_id) in items_to_close {
            self.close_item(group_id, item_id, cx);
        }
    }

    fn delete_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let Some(view) = self.project_views.get(&project_id) else {
            return;
        };
        // Terminate this window's terminals in the project before dropping it from the shared list.
        let items_to_close: Vec<(GroupId, ItemId)> = view
            .layout
            .terminal_items()
            .filter_map(|item| {
                item.session_id
                    .and_then(|session_id| view.layout.find_terminal_item(session_id))
            })
            .collect();
        for (group_id, item_id) in items_to_close {
            self.close_item(group_id, item_id, cx);
        }
        // Remove from the shared store (syncs to all windows); the observer reconciles views.
        self.project_store
            .update(cx, |store, cx| store.remove(project_id, cx));
        self.reconcile_project_views(cx);
        // D8: never leave the list empty — recreate a default Home project.
        if self.projects.is_empty() {
            self.add_project("Home".to_owned(), cx);
        }
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    fn activate_item(
        &mut self,
        group_id: GroupId,
        item_id: eggie_domain::ItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace_mut();
        if let Some(group) = workspace.layout.find_group_mut(group_id) {
            group.active_item_id = Some(item_id);
            workspace.active_group_id = group_id;
            self.focus_handle.focus(window, cx);
            self.terminal_has_focus = true;
            // The search bar is bound to a specific session; if the newly active session differs,
            // drop it so stale state and highlights don't linger (its doc contract: switching
            // sessions resets it).
            let active_session_id = self.active_session_id();
            if self
                .terminal_search
                .as_ref()
                .is_some_and(|search| Some(search.session_id) != active_session_id)
            {
                self.terminal_search = None;
            }
            self.ensure_snapshot_watchers(cx);
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    fn focus_group(&mut self, group_id: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace().layout.find_group(group_id).is_some() {
            self.workspace_mut().active_group_id = group_id;
            self.focus_handle.focus(window, cx);
            self.terminal_has_focus = true;
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    fn activate_session(
        &mut self,
        project_id: ProjectId,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((group_id, item_id)) = self
            .project_views
            .get(&project_id)
            .and_then(|view| view.layout.find_terminal_item(session_id))
        else {
            return;
        };
        self.active_project = project_id;
        self.activate_item(group_id, item_id, window, cx);
    }

    fn activate_session_by_id(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project_id) = self
            .project_views
            .iter()
            .find(|(_, view)| view.layout.find_terminal_item(session_id).is_some())
            .map(|(id, _)| *id)
        {
            self.activate_session(project_id, session_id, window, cx);
        }
    }

    fn close_item(
        &mut self,
        group_id: GroupId,
        item_id: eggie_domain::ItemId,
        cx: &mut Context<Self>,
    ) {
        let removed = self.workspace_mut().layout.close_item(group_id, item_id);
        let workspace = self.workspace_mut();
        if workspace.layout.find_group(group_id).is_none() {
            workspace.active_group_id = workspace.layout.first_group_id();
        }
        if let Some(session_id) = removed.and_then(|item| item.session_id) {
            if self.terminal_focused_session == Some(session_id) {
                self.terminal_focused_session = None;
            }
            let _ = self.client.request(ClientRequest::Terminate { session_id });
            self.sessions.retain(|session| session.id != session_id);
            self.snapshots.remove(&session_id);
            self.terminal_sizes.remove(&session_id);
            self.terminal_resize_in_flight.remove(&session_id);
            self.terminal_ime_states.remove(&session_id);
            if self
                .terminal_hovered_link
                .as_ref()
                .is_some_and(|(hovered_session, _)| *hovered_session == session_id)
            {
                self.terminal_hovered_link = None;
            }
            self.session_inspections.remove(&session_id);
            self.session_inspection_errors.remove(&session_id);
        }
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    fn resize_terminal(
        &mut self,
        session_id: SessionId,
        size: TerminalSize,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_sizes.get(&session_id) == Some(&size) {
            return;
        }
        self.terminal_sizes.insert(session_id, size);
        self.send_pending_terminal_resize(session_id, cx);
    }

    fn send_pending_terminal_resize(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if self.terminal_resize_in_flight.contains_key(&session_id) {
            return;
        }
        let Some(size) = self.terminal_sizes.get(&session_id).copied() else {
            return;
        };
        self.terminal_resize_in_flight.insert(session_id, size);
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = executor
                .spawn(async move {
                    client
                        .request(ClientRequest::Resize { session_id, size })
                        .map(|_| ())
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |app, cx| {
                app.finish_terminal_resize(session_id, size, result, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_terminal_resize(
        &mut self,
        session_id: SessionId,
        sent_size: TerminalSize,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_resize_in_flight.get(&session_id) != Some(&sent_size) {
            return;
        }
        self.terminal_resize_in_flight.remove(&session_id);
        if let Err(error) = result {
            eprintln!("failed to resize terminal {session_id}: {error}");
            if self.terminal_sizes.get(&session_id) == Some(&sent_size) {
                self.terminal_sizes.remove(&session_id);
            }
            return;
        }
        if self.terminal_sizes.get(&session_id) != Some(&sent_size) {
            self.send_pending_terminal_resize(session_id, cx);
        }
    }

    fn record_terminal_viewport(
        &mut self,
        group_id: GroupId,
        session_id: SessionId,
        bounds: Bounds<Pixels>,
        cell_width: f32,
        line_height: f32,
        size: TerminalSize,
    ) {
        self.terminal_viewports.insert(
            group_id,
            TerminalViewport {
                session_id,
                bounds,
                cell_width,
                line_height,
                rows: size.rows,
                columns: size.columns,
            },
        );
    }

    fn terminal_mouse_down(
        &mut self,
        group_id: GroupId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_group(group_id, window, cx);
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        // Cmd/Ctrl+click opens a link. Explicit OSC 8 hyperlinks take priority; a bare
        // auto-detected URL under the cursor is the fallback.
        let hyperlink = self.snapshots.get(&viewport.session_id).and_then(|snapshot| {
            terminal_hyperlink_at(snapshot, point)
                .or_else(|| detected_link_at(snapshot, point).map(|link| link.url.clone()))
        });
        if event.button == MouseButton::Left
            && event.modifiers.platform
            && let Some(url) = hyperlink
        {
            self.hyperlink_mouse_down = true;
            self.terminal_selection_drag = None;
            if url_has_safe_external_scheme(&url) {
                cx.open_url(&url);
            } else {
                let language = self.language;
                let detail = format!("{}\n{url}", language.hyperlink_open_detail());
                let prompt = window.prompt(
                    PromptLevel::Warning,
                    language.open_hyperlink_title(),
                    Some(&detail),
                    &[language.open(), language.cancel()],
                    cx,
                );
                window
                    .spawn(cx, async move |cx| {
                        if matches!(prompt.await, Ok(0)) {
                            cx.update(|_, cx| cx.open_url(&url)).ok();
                        }
                    })
                    .detach();
            }
            cx.stop_propagation();
            return;
        }
        let Some(button) = protocol_mouse_button(event.button) else {
            return;
        };
        let modifiers = protocol_terminal_modifiers(event.modifiers);
        if let Err(error) = self.input_sender.send_mouse(
            viewport.session_id,
            TerminalMouseEvent {
                action: TerminalMouseAction::Press,
                button: Some(button),
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers,
            },
        ) {
            eprintln!("failed to enqueue terminal mouse press: {error:#}");
        }

        let Some(snapshot) = self.snapshots.get(&viewport.session_id) else {
            return;
        };
        let captured = snapshot.input_modes.captures_mouse() && !event.modifiers.shift;
        if captured {
            self.terminal_selection_drag = None;
            let _ = self.input_sender.send_selection_clear(viewport.session_id);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            cx.stop_propagation();
            return;
        }

        let side = terminal_selection_side(viewport, event.position);
        // The daemon owns the authoritative selection (a terminal-core `Selection` spanning
        // scrollback). We only forward the viewport point + intent; the daemon expands word/line
        // selections and keeps the anchor valid across scroll. `preserve_without_drag` records
        // whether a selection should survive a click that never becomes a drag.
        let preserve_without_drag = match event.click_count {
            0 | 1 if event.modifiers.shift => {
                // Extend the existing selection's head to the clicked cell.
                let _ = self
                    .input_sender
                    .send_selection_update(viewport.session_id, cell_position(point), side);
                true
            }
            0 | 1 => {
                let _ = self.input_sender.send_selection_start(
                    viewport.session_id,
                    cell_position(point),
                    side,
                    TerminalSelectionKind::Simple,
                );
                false
            }
            2 => {
                let _ = self.input_sender.send_selection_start(
                    viewport.session_id,
                    cell_position(point),
                    side,
                    TerminalSelectionKind::Semantic,
                );
                true
            }
            _ => {
                let _ = self.input_sender.send_selection_start(
                    viewport.session_id,
                    cell_position(point),
                    side,
                    TerminalSelectionKind::Lines,
                );
                true
            }
        };
        self.terminal_selection_drag = Some(TerminalSelectionDrag {
            group_id,
            session_id: viewport.session_id,
            dragged: preserve_without_drag,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn terminal_mouse_up(
        &mut self,
        group_id: GroupId,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        if self.hyperlink_mouse_down {
            self.hyperlink_mouse_down = false;
            cx.stop_propagation();
            return;
        }
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        let Some(button) = protocol_mouse_button(event.button) else {
            return;
        };
        if let Err(error) = self.input_sender.send_mouse(
            viewport.session_id,
            TerminalMouseEvent {
                action: TerminalMouseAction::Release,
                button: Some(button),
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers: protocol_terminal_modifiers(event.modifiers),
            },
        ) {
            eprintln!("failed to enqueue terminal mouse release: {error:#}");
        }
        if self
            .snapshots
            .get(&viewport.session_id)
            .is_some_and(|snapshot| snapshot.input_modes.captures_mouse())
            && !event.modifiers.shift
        {
            cx.stop_propagation();
        }
    }

    fn terminal_mouse_move(
        &mut self,
        group_id: GroupId,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        // Update the hovered-URL state first, before the mouse-tracking early-returns, so the hover
        // underline and pointer cursor work even for apps that consume mouse motion.
        self.update_terminal_hovered_link(viewport.session_id, point, cx);
        let Some(tracking) = self
            .snapshots
            .get(&viewport.session_id)
            .map(|snapshot| snapshot.input_modes.mouse_tracking)
        else {
            return;
        };
        let button = event.pressed_button.and_then(protocol_mouse_button);
        let should_report = match tracking {
            TerminalMouseTracking::Disabled | TerminalMouseTracking::Click => false,
            TerminalMouseTracking::Drag => button.is_some(),
            TerminalMouseTracking::Motion => true,
        };
        if !should_report
            || (event.modifiers.shift
                && matches!(
                    button,
                    Some(TerminalMouseButton::Left | TerminalMouseButton::Right)
                ))
        {
            return;
        }
        if let Err(error) = self.input_sender.send_mouse(
            viewport.session_id,
            TerminalMouseEvent {
                action: TerminalMouseAction::Move,
                button,
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers: protocol_terminal_modifiers(event.modifiers),
            },
        ) {
            eprintln!("failed to enqueue terminal mouse motion: {error:#}");
        }
    }

    /// Recompute which auto-detected URL (if any) is under `point` and update hover state. Only
    /// notifies when the hovered link actually changes, to avoid a rerender on every mouse move.
    fn update_terminal_hovered_link(
        &mut self,
        session_id: SessionId,
        point: TerminalPoint,
        cx: &mut Context<Self>,
    ) {
        let hovered = self
            .snapshots
            .get(&session_id)
            .and_then(|snapshot| detected_link_at(snapshot, point))
            .map(|link| (session_id, link.clone()));
        if self.terminal_hovered_link != hovered {
            self.terminal_hovered_link = hovered;
            cx.notify();
        }
    }

    fn terminal_scroll(
        &mut self,
        group_id: GroupId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        let (x, y, unit) = match event.delta {
            ScrollDelta::Pixels(delta) => (
                f32::from(delta.x),
                f32::from(delta.y),
                TerminalScrollUnit::Pixels,
            ),
            ScrollDelta::Lines(delta) => (delta.x, delta.y, TerminalScrollUnit::Lines),
        };
        let phase = match event.touch_phase {
            TouchPhase::Started => TerminalScrollPhase::Started,
            TouchPhase::Moved => TerminalScrollPhase::Moved,
            TouchPhase::Ended | TouchPhase::Cancelled => TerminalScrollPhase::Ended,
        };
        if let Err(error) = self.input_sender.send_scroll(
            viewport.session_id,
            TerminalScrollEvent {
                delta: TerminalScrollDelta {
                    x: fixed_terminal_scroll_delta(x),
                    y: fixed_terminal_scroll_delta(y),
                    unit,
                },
                phase,
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers: protocol_terminal_modifiers(event.modifiers),
            },
        ) {
            eprintln!("failed to enqueue terminal scroll: {error:#}");
        }
        cx.stop_propagation();
    }

    fn update_terminal_selection_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut drag) = self.terminal_selection_drag else {
            return;
        };
        if !event.dragging() {
            return;
        }
        let Some(viewport) = self.terminal_viewports.get(&drag.group_id).copied() else {
            return;
        };
        // Auto-scroll when the pointer is dragged above or below the viewport, so a selection can be
        // extended into scrollback. The daemon scrolls the grid and the terminal core keeps the
        // anchor valid; the next snapshot reprojects the selection over the new viewport.
        let top = f32::from(viewport.bounds.origin.y);
        let bottom = top + f32::from(viewport.bounds.size.height);
        let pointer_y = f32::from(event.position.y);
        if viewport.line_height > 0. {
            let lines = if pointer_y < top {
                ((top - pointer_y) / viewport.line_height).ceil() as i32
            } else if pointer_y > bottom {
                -(((pointer_y - bottom) / viewport.line_height).ceil() as i32)
            } else {
                0
            };
            if lines != 0 {
                let clamped_point =
                    terminal_point_from_position(viewport, event.position).unwrap_or(TerminalPoint {
                        line: 0,
                        column: 0,
                    });
                let _ = self.input_sender.send_scroll(
                    drag.session_id,
                    TerminalScrollEvent {
                        delta: TerminalScrollDelta {
                            x: 0,
                            y: lines.saturating_mul(TERMINAL_SCROLL_DELTA_SCALE),
                            unit: TerminalScrollUnit::Lines,
                        },
                        phase: TerminalScrollPhase::Moved,
                        position: protocol_mouse_position(clamped_point, viewport, event.position),
                        modifiers: protocol_terminal_modifiers(event.modifiers),
                    },
                );
            }
        }
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        let side = terminal_selection_side(viewport, event.position);
        let _ = self
            .input_sender
            .send_selection_update(drag.session_id, cell_position(point), side);
        drag.dragged = true;
        self.terminal_selection_drag = Some(drag);
        cx.notify();
    }

    fn finish_terminal_selection_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.terminal_selection_drag.take() else {
            return;
        };
        if !drag.dragged {
            let _ = self.input_sender.send_selection_clear(drag.session_id);
        } else if self.copy_on_select {
            // The user dragged out a real selection; mirror it to the system clipboard immediately.
            // Uses the drag's own session (not the active one) so a selection in a background split
            // still copies correctly.
            self.copy_terminal_selection(drag.session_id, cx);
        }
        cx.notify();
    }

    fn copy_active_terminal_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        self.copy_terminal_selection(session_id, cx)
    }

    fn copy_terminal_selection(&self, session_id: SessionId, cx: &mut Context<Self>) -> bool {
        // The daemon owns the selection and extracts its text from the terminal core (whole
        // scrollback span, soft-wrap unwrapped, trailing whitespace trimmed).
        let text = match self
            .client
            .request(ClientRequest::TerminalCopySelection { session_id })
        {
            Ok(DaemonResponse::SelectionText { text, .. }) => text,
            _ => return false,
        };
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn paste_into_active_terminal(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let paste_events = self
            .snapshots
            .get(&session_id)
            .is_some_and(|snapshot| snapshot.input_modes.paste_events);
        let enqueued = if paste_events {
            let contents = clipboard_contents_from_item(item, &["*/*".to_owned()]);
            self.enqueue_terminal_clipboard_paste(
                session_id,
                TerminalClipboardSelection::Clipboard,
                contents,
            )
        } else {
            let Some(text) = item.text() else {
                return false;
            };
            self.enqueue_terminal_paste(session_id, text)
        };
        if !enqueued {
            return false;
        }
        let _ = self.input_sender.send_selection_clear(session_id);
        let ime_changed = self.terminal_ime_states.remove(&session_id).is_some();
        if ime_changed {
            cx.notify();
        }
        true
    }

    fn next_input_sequence() -> u64 {
        NEXT_INPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    }

    fn enqueue_terminal_input(&self, session_id: SessionId, bytes: Vec<u8>) -> bool {
        let sequence = Self::next_input_sequence();
        self.input_latency.record_input(session_id, sequence);
        if let Err(error) = self.input_sender.send_input(session_id, bytes, sequence) {
            self.input_latency.discard_input(session_id, sequence);
            eprintln!("failed to enqueue terminal input: {error:#}");
            return false;
        }
        true
    }

    fn enqueue_terminal_paste(&self, session_id: SessionId, text: String) -> bool {
        let sequence = Self::next_input_sequence();
        self.input_latency.record_input(session_id, sequence);
        if let Err(error) = self.input_sender.send_paste(session_id, text, sequence) {
            self.input_latency.discard_input(session_id, sequence);
            eprintln!("failed to enqueue terminal paste: {error:#}");
            return false;
        }
        true
    }

    fn enqueue_terminal_clipboard_paste(
        &self,
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
    ) -> bool {
        let sequence = Self::next_input_sequence();
        self.input_latency.record_input(session_id, sequence);
        if let Err(error) = self
            .input_sender
            .send_paste_clipboard(session_id, selection, contents, sequence)
        {
            self.input_latency.discard_input(session_id, sequence);
            eprintln!("failed to enqueue terminal rich paste: {error:#}");
            return false;
        }
        true
    }

    fn select_all_active_terminal(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        // The daemon selects the whole buffer (including scrollback) in the terminal core and
        // republishes; the projected selection arrives with the next snapshot.
        self.client
            .request(ClientRequest::TerminalSelectAll { session_id })
            .is_ok()
    }

    // --- Keybinding action handlers ----------------------------------------------------------
    //
    // These are thin wrappers over existing capabilities, invoked by the configurable keymap.
    // Each returns whether it acted so the caller can stop action propagation.

    fn new_tab_in_active_group(&mut self, cx: &mut Context<Self>) -> bool {
        let group_id = self.workspace().active_group_id;
        self.new_terminal(group_id, None, cx);
        true
    }

    fn split_active_group(&mut self, direction: Direction, cx: &mut Context<Self>) -> bool {
        let group_id = self.workspace().active_group_id;
        self.new_terminal(group_id, Some(direction), cx);
        true
    }

    fn close_active_tab(&mut self, cx: &mut Context<Self>) -> bool {
        let group_id = self.workspace().active_group_id;
        let Some(item_id) = self
            .workspace()
            .layout
            .find_group(group_id)
            .and_then(|group| group.active_item_id)
        else {
            return false;
        };
        self.close_item(group_id, item_id, cx);
        true
    }

    fn activate_relative_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let group_id = self.workspace().active_group_id;
        let Some(group) = self.workspace().layout.find_group(group_id) else {
            return false;
        };
        let count = group.items.len();
        if count < 2 {
            return false;
        }
        let Some(current) = group
            .active_item_id
            .and_then(|active| group.items.iter().position(|item| item.id == active))
        else {
            return false;
        };
        let next_index = (current as isize + delta).rem_euclid(count as isize) as usize;
        let item_id = group.items[next_index].id;
        self.activate_item(group_id, item_id, window, cx);
        true
    }

    fn clear_active_terminal(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        // Send Ctrl-L (form feed); the shell redraws its prompt on a cleared screen.
        self.enqueue_terminal_input(session_id, vec![0x0c])
    }

    fn scroll_active_terminal(&mut self, command: TerminalScrollCommand, _cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        self.client
            .request(ClientRequest::TerminalScrollTo {
                session_id,
                command,
            })
            .is_ok()
    }

    fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| settings.font_size = (settings.font_size + delta).round(),
                cx,
            )
        });
        true
    }

    fn reset_font_size(&mut self, cx: &mut Context<Self>) -> bool {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.font_size = DEFAULT_FONT_SIZE, cx)
        });
        true
    }

    /// Open the in-terminal search bar for the active session (⌘F), or refocus it if already open.
    fn open_terminal_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        match &self.terminal_search {
            Some(search) if search.session_id == session_id => {}
            _ => {
                let colors = self.colors;
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(
                        window,
                        cx,
                        TextInputStyle {
                            text_color: (colors.text << 8) | 0xff,
                            placeholder_color: (colors.muted << 8) | 0xff,
                            cursor_color: (colors.accent << 8) | 0xff,
                            selection_color: (colors.accent << 8) | 0x55,
                        },
                    );
                    input.set_placeholder(self.language.search_placeholder());
                    input
                });
                let subscription =
                    cx.subscribe_in(&input, window, Self::on_search_input_event);
                self.terminal_search = Some(TerminalSearchUi {
                    session_id,
                    input,
                    regex: false,
                    result: None,
                    _subscription: subscription,
                });
            }
        }
        if let Some(search) = &self.terminal_search {
            let handle = search.input.read(cx).focus_handle();
            handle.focus(window, cx);
        }
        cx.notify();
        true
    }

    /// Route events emitted by the search input to search behavior.
    fn on_search_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                self.run_terminal_search(TerminalSearchDirection::Forward, true, cx)
            }
            TextInputEvent::Confirm => {
                self.navigate_terminal_search(TerminalSearchDirection::Forward, cx)
            }
            TextInputEvent::ConfirmReverse => {
                self.navigate_terminal_search(TerminalSearchDirection::Backward, cx)
            }
            TextInputEvent::Cancel => {
                // Close and return focus to the terminal (same path as the X button), so keyboard
                // input keeps working instead of being dropped when the input entity stops rendering.
                self.close_terminal_search(window, cx);
            }
        }
    }

    /// Close the search bar, drop its highlights, and return focus to the terminal.
    fn close_terminal_search(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.terminal_search.take().is_none() {
            return false;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
        true
    }

    /// The current query text, or empty if the search bar is closed.
    fn terminal_search_query(&self, cx: &App) -> String {
        self.terminal_search
            .as_ref()
            .map(|search| search.input.read(cx).content().to_owned())
            .unwrap_or_default()
    }

    /// Send the current query to the daemon and apply the result (highlights + counter). `fresh`
    /// restarts the search from the viewport (used when the query text changes); otherwise the
    /// daemon advances from the previous active match in `direction`.
    fn run_terminal_search(
        &mut self,
        direction: TerminalSearchDirection,
        fresh: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = &self.terminal_search else {
            return;
        };
        let session_id = search.session_id;
        let request = TerminalSearchRequest {
            query: search.input.read(cx).content().to_owned(),
            regex: search.regex,
            direction,
            fresh,
        };
        let result = match self.client.request(ClientRequest::TerminalSearch {
            session_id,
            request,
        }) {
            Ok(DaemonResponse::SearchResult { result, .. }) => result,
            Ok(_) => return,
            Err(error) => {
                self.poll_error = Some(format!("terminal search failed: {error}"));
                return;
            }
        };
        // The daemon may have scrolled its viewport to reveal the active match and published a new
        // snapshot; the match coordinates are relative to THAT snapshot. Pull it synchronously when
        // it is newer than what we hold, so highlights align with the text instead of being painted
        // over the pre-scroll frame (the async watcher would otherwise land a frame or more later).
        let stale = self
            .snapshots
            .get(&session_id)
            .is_none_or(|snapshot| snapshot.revision < result.revision);
        if stale
            && let Ok(DaemonResponse::Snapshot { snapshot }) =
                self.client.request(ClientRequest::Snapshot { session_id })
        {
            self.snapshots.insert(session_id, snapshot);
        }
        if let Some(search) = &mut self.terminal_search {
            search.result = Some(result);
        }
        cx.notify();
    }

    /// Toggle regex interpretation of the query and re-run the search.
    fn toggle_terminal_search_regex(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.terminal_search else {
            return;
        };
        search.regex = !search.regex;
        self.run_terminal_search(TerminalSearchDirection::Forward, true, cx);
    }

    /// Advance to the next (Enter) or previous (Shift+Enter) match.
    fn navigate_terminal_search(
        &mut self,
        direction: TerminalSearchDirection,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_search_query(cx).is_empty() {
            return;
        }
        self.run_terminal_search(direction, false, cx);
    }

    /// Viewport-relative search highlights for `session_id`, if the search bar targets it.
    fn terminal_search_matches(&self, session_id: SessionId) -> Option<&TerminalSearchResult> {
        let search = self.terminal_search.as_ref()?;
        if search.session_id != session_id {
            return None;
        }
        search.result.as_ref()
    }

    /// Render the floating search bar overlaid on the top-right of the terminal content region.
    fn render_terminal_search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.terminal_search.as_ref()?;
        let colors = self.colors;
        let query_is_empty = search.input.read(cx).content().is_empty();
        let regex = search.regex;
        let (index, total) = search
            .result
            .as_ref()
            .map(|result| (result.index, result.total))
            .unwrap_or((0, 0));

        // "3/12" when there are matches, "0/0" for a query with no hits, blank for an empty query.
        let counter_text = if query_is_empty {
            String::new()
        } else if total == 0 {
            "0/0".to_owned()
        } else {
            format!("{}/{}", index + 1, total)
        };
        let no_matches = !query_is_empty && total == 0;

        let bar = div()
            .absolute()
            .top(px(8.))
            .right(px(12.))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(34.))
            // Ideal width, but never wider than the container so a narrow split can't overflow it.
            .w(px(340.))
            .max_w(relative(0.9))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_md()
            // Floating overlay over the terminal: occlude blocks every mouse event type (down/up/move/
            // scroll) plus hover/tooltip for the content region behind, so presses on the bar's chrome
            // (icon, counter, padding) can't bubble down to terminal_mouse_down and steal focus. One
            // call, all event types — replaces the earlier left-mouse-down-only stop_propagation patch.
            .occlude()
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(colors.muted))
                    .child(icon(IconName::Search)),
            )
            // The reusable text-input component owns focus, selection, caret, and editing.
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(48.))
                    .h_full()
                    .text_size(px(13.))
                    .text_color(rgb(colors.text))
                    .child(search.input.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .min_w(px(38.))
                    .text_size(px(12.))
                    .text_color(rgb(if no_matches { 0xE06C75 } else { colors.muted }))
                    .child(counter_text),
            )
            // Regex toggle: `.*` highlighted when active.
            .child(
                div()
                    .id("terminal-search-regex")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded_lg()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(rgb(if regex { colors.accent } else { colors.muted }))
                    .when(regex, |element| element.bg(rgb(colors.panel_alt)))
                    .hover(move |element| element.bg(rgb(colors.hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.toggle_terminal_search_regex(cx);
                    }))
                    .child(".*"),
            )
            .child(icon_button(
                IconName::ArrowUp,
                "terminal-search-prev",
                colors,
                None,
                cx.listener(|app, _, _, cx| {
                    cx.stop_propagation();
                    app.navigate_terminal_search(TerminalSearchDirection::Backward, cx);
                }),
            ))
            .child(icon_button(
                IconName::ArrowDown,
                "terminal-search-next",
                colors,
                None,
                cx.listener(|app, _, _, cx| {
                    cx.stop_propagation();
                    app.navigate_terminal_search(TerminalSearchDirection::Forward, cx);
                }),
            ))
            .child(icon_button(
                IconName::Close,
                "terminal-search-close",
                colors,
                None,
                cx.listener(|app, _, window, cx| {
                    cx.stop_propagation();
                    app.close_terminal_search(window, cx);
                }),
            ));
        Some(bar.into_any_element())
    }

    /// Toggle the command palette: open it (building a fresh input) if closed, close it if open.
    fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.is_some() {
            self.close_command_palette(window, cx);
            return;
        }
        let colors = self.colors;
        let input = cx.new(|cx| {
            let mut input = TextInput::new(
                window,
                cx,
                TextInputStyle {
                    text_color: (colors.text << 8) | 0xff,
                    placeholder_color: (colors.muted << 8) | 0xff,
                    cursor_color: (colors.accent << 8) | 0xff,
                    selection_color: (colors.accent << 8) | 0x55,
                },
            );
            input.set_placeholder(self.language.command_palette_placeholder());
            input
        });
        let subscription = cx.subscribe_in(&input, window, Self::on_palette_input_event);
        let handle = input.read(cx).focus_handle();
        self.command_palette = Some(CommandPaletteUi {
            input,
            selected_index: 0,
            scroll_handle: ScrollHandle::new(),
            _subscription: subscription,
        });
        handle.focus(window, cx);
        cx.notify();
    }

    /// Close the palette and return focus to the terminal so keyboard input keeps working.
    fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.command_palette.take().is_none() {
            return false;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
        true
    }

    /// Route events emitted by the palette's filter input.
    fn on_palette_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                // A new query reshuffles the filtered list; reset the selection to the top.
                if let Some(palette) = &mut self.command_palette {
                    palette.selected_index = 0;
                    palette.scroll_handle.set_offset(point(px(0.), px(0.)));
                }
                cx.notify();
            }
            TextInputEvent::Confirm | TextInputEvent::ConfirmReverse => {
                self.run_selected_palette_command(window, cx);
            }
            TextInputEvent::Cancel => {
                self.close_command_palette(window, cx);
            }
        }
    }

    /// The palette's current filter query (empty if the palette is closed).
    fn command_palette_query(&self, cx: &App) -> String {
        self.command_palette
            .as_ref()
            .map(|palette| palette.input.read(cx).content().trim().to_owned())
            .unwrap_or_default()
    }

    /// The palette entries matching the current query, in display order.
    fn filtered_palette_entries(&self, cx: &App) -> Vec<PaletteEntry> {
        let config = self.settings.read(cx).config();
        let query = self.command_palette_query(cx);
        palette_entries(config)
            .into_iter()
            .filter(|entry| command_matches(entry.label, &query))
            .collect()
    }

    /// Move the palette selection by `delta`, clamped to the filtered list, and keep it in view.
    fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.filtered_palette_entries(cx).len();
        let Some(palette) = &mut self.command_palette else {
            return;
        };
        if count == 0 {
            palette.selected_index = 0;
            return;
        }
        let last = count - 1;
        let current = palette.selected_index.min(last) as isize;
        let next = (current + delta).clamp(0, last as isize) as usize;
        palette.selected_index = next;
        palette.scroll_handle.scroll_to_item(next);
        cx.notify();
    }

    /// Run the currently selected command: pull its action out of the filtered list, close the
    /// palette (which returns focus to the terminal), then dispatch. Order matters — the dispatch
    /// path is captured against the focused node, so focus must be back on the workspace root first
    /// for window-level command handlers to receive it.
    fn run_selected_palette_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = &self.command_palette else {
            return;
        };
        let index = palette.selected_index;
        let mut entries = self.filtered_palette_entries(cx);
        if index >= entries.len() {
            return;
        }
        let action = entries.swap_remove(index).action;
        self.close_command_palette(window, cx);
        window.dispatch_action(action, cx);
    }

    /// Render the command palette as a centered modal over the whole workspace, or `None` when it is
    /// closed. A dimmed backdrop catches clicks (and, via `.occlude()`, all other pointer events) to
    /// close it; the panel holds the filter input above a scrollable, filterable command list.
    fn render_command_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = self.command_palette.as_ref()?;
        let colors = self.colors;
        let entries = self.filtered_palette_entries(cx);
        let selected = palette.selected_index.min(entries.len().saturating_sub(1));

        let mut list = div()
            .id("command-palette-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&palette.scroll_handle)
            .py_1();
        if entries.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .px_3()
                    .text_color(rgb(colors.muted))
                    .child(self.language.command_palette_empty()),
            );
        } else {
            for (index, entry) in entries.into_iter().enumerate() {
                let is_selected = index == selected;
                let mut row = div()
                    .id(("command-palette-row", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .flex_none()
                    .h(px(34.))
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .when(is_selected, |element| {
                        element
                            .bg(rgb(colors.panel_alt))
                            .text_color(rgb(colors.accent))
                    })
                    .hover(|element| element.bg(rgb(colors.panel_alt)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if let Some(palette) = &mut app.command_palette {
                            palette.selected_index = index;
                        }
                        app.run_selected_palette_command(window, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(entry.label.to_string()),
                    );
                if let Some(keystroke) = entry.keystroke {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(keystroke),
                    );
                }
                list = list.child(row);
            }
        }

        let panel = div()
            .id("command-palette-panel")
            .flex()
            .flex_col()
            .w(px(560.))
            .max_w(relative(0.9))
            .max_h(relative(0.5))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            // Swallow presses inside the panel so they don't bubble to the backdrop's close handler.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // ↑/↓ move the selection. The text input never binds these keys, so they bubble here
            // uncontested; every other key falls through to the input.
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "up" => {
                        app.move_palette_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        app.move_palette_selection(1, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .h(px(40.))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(colors.muted))
                            .child(icon(IconName::Search)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .text_size(px(14.))
                            .text_color(rgb(colors.text))
                            .child(palette.input.clone()),
                    ),
            )
            .child(list);

        let backdrop = div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            // Push the panel down from the top edge so it reads as a palette, not a dialog.
            .pt(px(96.))
            // Block every pointer event for the terminal content behind the modal.
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, cx| {
                    app.close_command_palette(window, cx);
                }),
            )
            .child(panel);
        Some(backdrop.into_any_element())
    }

    pub(crate) fn terminal_ime_state(&self, session_id: SessionId) -> Option<&TerminalImeState> {
        self.terminal_ime_states.get(&session_id)
    }

    pub(crate) fn set_terminal_marked_text(
        &mut self,
        session_id: SessionId,
        text: &str,
        selected_range: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let length = text.encode_utf16().count();
        if text.is_empty() {
            self.terminal_ime_states.remove(&session_id);
        } else {
            let selected_range = selected_range.unwrap_or(length..length);
            self.terminal_ime_states.insert(
                session_id,
                TerminalImeState {
                    text: text.to_owned(),
                    selected_range: selected_range.start.min(length)
                        ..selected_range.end.min(length),
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn clear_terminal_marked_text(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_ime_states.remove(&session_id).is_some() {
            cx.notify();
        }
    }

    pub(crate) fn commit_terminal_text(
        &mut self,
        session_id: SessionId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.terminal_ime_states.remove(&session_id);
        let _ = self.input_sender.send_selection_clear(session_id);
        if !text.is_empty() {
            self.enqueue_terminal_input(session_id, terminal_text_bytes(text));
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // When the search input or command palette is focused it owns keyboard input; never leak
        // keys into the terminal. The input's own handlers consume the keys; this guard covers the
        // case where the event bubbles up to the workspace root before the input consumes it.
        if self
            .terminal_search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).is_focused(window))
            || self
                .command_palette
                .as_ref()
                .is_some_and(|palette| palette.input.read(cx).is_focused(window))
        {
            return;
        }
        if is_character_palette_shortcut(event) && self.active_session_id().is_some() {
            window.show_character_palette();
            cx.stop_propagation();
            return;
        }
        let Some(session_id) = self.active_session_id() else {
            return;
        };
        let keyboard_flags = self
            .snapshots
            .get(&session_id)
            .map_or(0, |snapshot| snapshot.input_modes.kitty_keyboard_flags);
        let Some(bytes) = terminal_bytes_with_mode(event, keyboard_flags) else {
            return;
        };
        self.enqueue_terminal_input(session_id, bytes);
        let _ = self.input_sender.send_selection_clear(session_id);
        cx.stop_propagation();
    }

    fn key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Mirror the key_down guard: while the search input or command palette is focused it owns
        // keyboard input, so key releases must not leak into the terminal (e.g. as kitty key-release
        // escape sequences).
        if self
            .terminal_search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).is_focused(window))
            || self
                .command_palette
                .as_ref()
                .is_some_and(|palette| palette.input.read(cx).is_focused(window))
        {
            return;
        }
        let Some(session_id) = self.active_session_id() else {
            return;
        };
        let keyboard_flags = self
            .snapshots
            .get(&session_id)
            .map_or(0, |snapshot| snapshot.input_modes.kitty_keyboard_flags);
        if keyboard_flags & KITTY_REPORT_EVENT_TYPES == 0 {
            return;
        }
        let Some(bytes) = kitty_key_bytes(&event.keystroke, keyboard_flags, 3) else {
            return;
        };
        self.enqueue_terminal_input(session_id, bytes);
        cx.stop_propagation();
    }

    fn toggle_left_sidebar(&mut self, cx: &mut Context<Self>) {
        self.left_sidebar_collapsed = !self.left_sidebar_collapsed;
        self.resizing_sidebar = None;
        cx.notify();
    }

    fn toggle_right_sidebar(&mut self, cx: &mut Context<Self>) {
        self.right_sidebar_collapsed = !self.right_sidebar_collapsed;
        self.resizing_sidebar = None;
        cx.notify();
    }

    fn start_sidebar_resize(
        &mut self,
        edge: SidebarEdge,
        _: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.resizing_sidebar = Some(edge);
        cx.stop_propagation();
    }

    fn resize_sidebar(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edge) = self.resizing_sidebar else {
            return;
        };
        if !event.dragging() {
            self.resizing_sidebar = None;
            return;
        }

        let window_width = f32::from(window.bounds().size.width);
        let center_min_width = 320.;
        match edge {
            SidebarEdge::Left => {
                let right_width = if self.right_sidebar_collapsed {
                    0.
                } else {
                    self.right_sidebar_width
                };
                let maximum = LEFT_SIDEBAR_MAX_WIDTH
                    .min(window_width - right_width - center_min_width)
                    .max(LEFT_SIDEBAR_MIN_WIDTH);
                self.left_sidebar_width =
                    f32::from(event.position.x).clamp(LEFT_SIDEBAR_MIN_WIDTH, maximum);
            }
            SidebarEdge::Right => {
                let left_width = if self.left_sidebar_collapsed {
                    0.
                } else {
                    self.left_sidebar_width
                };
                let maximum = RIGHT_SIDEBAR_MAX_WIDTH
                    .min(window_width - left_width - center_min_width)
                    .max(RIGHT_SIDEBAR_MIN_WIDTH);
                self.right_sidebar_width = (window_width - f32::from(event.position.x))
                    .clamp(RIGHT_SIDEBAR_MIN_WIDTH, maximum);
            }
        }
        cx.notify();
    }

    fn start_split_resize(&mut self, split_id: SplitId, cx: &mut Context<Self>) {
        self.resizing_split = Some(split_id);
        cx.stop_propagation();
    }

    fn resize_split(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(split_id) = self.resizing_split else {
            return;
        };
        if !event.dragging() {
            self.resizing_split = None;
            return;
        }

        let Some(bounds) = self.split_bounds.borrow().get(&split_id).copied() else {
            return;
        };

        let layout = self.workspace().layout.clone();
        let Some(axis) = layout.split_axis(split_id) else {
            return;
        };

        let ratio = match axis {
            SplitAxis::Horizontal => {
                let width = f32::from(bounds.size.width);
                if width <= 0. {
                    return;
                }
                (f32::from(event.position.x - bounds.left()) / width).clamp(0.05, 0.95)
            }
            SplitAxis::Vertical => {
                let height = f32::from(bounds.size.height);
                if height <= 0. {
                    return;
                }
                (f32::from(event.position.y - bounds.top()) / height).clamp(0.05, 0.95)
            }
        };

        self.workspace_mut().layout.set_split_ratio(split_id, ratio);
        cx.notify();
    }

    /// Send an absolute scroll request for the scrollbar thumb drag / track click.
    fn scroll_terminal_to_offset(&self, session_id: SessionId, offset: u32) {
        if let Err(error) = self.client.request(ClientRequest::TerminalScrollToOffset {
            session_id,
            offset,
        }) {
            eprintln!("failed to scroll terminal to offset: {error:#}");
        }
    }

    /// Drive an in-progress scrollbar thumb drag. Mirrors `resize_split`: it reads the cached track
    /// bounds, maps the pointer Y (minus the grab offset) to an absolute scroll offset, renders the
    /// thumb optimistically at that offset, and asks the daemon to scroll there.
    fn drag_scrollbar(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut drag) = self.dragging_scrollbar else {
            return;
        };
        if !event.dragging() {
            self.dragging_scrollbar = None;
            return;
        }
        let Some(bounds) = self.scrollbar_bounds.borrow().get(&drag.group_id).copied() else {
            return;
        };
        let Some(snapshot) = self.snapshots.get(&drag.session_id) else {
            return;
        };
        let history = snapshot.history_size;
        let rows = snapshot.size.rows;
        let track_height = f32::from(bounds.size.height);
        let thumb_top = f32::from(event.position.y) - f32::from(bounds.top()) - drag.grab_offset;
        let offset = scrollbar_offset_from_thumb_top(track_height, history, rows, thumb_top);
        drag.pending_offset = offset;
        self.dragging_scrollbar = Some(drag);
        self.scroll_terminal_to_offset(drag.session_id, offset);
        cx.notify();
    }

    /// Handle a click on the scrollbar track (outside the thumb): page up if the click is above the
    /// thumb, page down if below. Reuses the discrete PageUp/PageDown scroll commands.
    fn scrollbar_track_click(
        &mut self,
        group_id: GroupId,
        session_id: SessionId,
        position: Point<Pixels>,
    ) {
        let Some(bounds) = self.scrollbar_bounds.borrow().get(&group_id).copied() else {
            return;
        };
        let Some(snapshot) = self.snapshots.get(&session_id) else {
            return;
        };
        let track_height = f32::from(bounds.size.height);
        let Some(thumb) =
            scrollbar_thumb_geometry(track_height, snapshot.history_size, snapshot.size.rows, snapshot.display_offset)
        else {
            return;
        };
        let click_y = f32::from(position.y) - f32::from(bounds.top());
        let command = if click_y < thumb.top {
            TerminalScrollCommand::PageUp
        } else if click_y > thumb.top + thumb.height {
            TerminalScrollCommand::PageDown
        } else {
            return;
        };
        if let Err(error) = self.client.request(ClientRequest::TerminalScrollTo {
            session_id,
            command,
        }) {
            eprintln!("failed to page terminal scrollbar: {error:#}");
        }
    }

    /// Render the terminal scrollbar overlay for a pane. Returns `None` when there is no scrollback
    /// (the scrollbar is hidden) or on the first frame before the track's bounds have been captured.
    ///
    /// The whole strip is `.occlude()`d so pointer events over it never fall through to the terminal
    /// (selection / link-hover) beneath — matching the search-bar overlay precedent. The thumb top
    /// is rendered from the previous frame's cached track height; while this group's thumb is being
    /// dragged we substitute the optimistic `pending_offset` so it tracks the pointer immediately.
    fn render_scrollbar(
        &self,
        group_id: GroupId,
        session_id: SessionId,
        history: u32,
        offset: u32,
        rows: u16,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if history == 0 {
            return None;
        }
        // Optimistic offset during an active drag on this group.
        let render_offset = match self.dragging_scrollbar {
            Some(drag) if drag.group_id == group_id => drag.pending_offset,
            _ => offset,
        };
        let track_height = self
            .scrollbar_bounds
            .borrow()
            .get(&group_id)
            .map(|bounds| f32::from(bounds.size.height));
        let thumb = track_height
            .and_then(|height| scrollbar_thumb_geometry(height, history, rows, render_offset));

        let scrollbar_bounds = self.scrollbar_bounds.clone();
        let dragging_this = self
            .dragging_scrollbar
            .is_some_and(|drag| drag.group_id == group_id);
        let muted = self.colors.muted;
        // Same hue at every state, only the opacity changes: light at rest, a touch darker on hover,
        // darkest while dragging. Never the accent color.
        const THUMB_ALPHA_IDLE: u32 = 0x59;
        const THUMB_ALPHA_HOVER: u32 = 0x99;
        const THUMB_ALPHA_DRAG: u32 = 0xcc;

        let mut track = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(TERMINAL_SCROLLBAR_WIDTH))
            .occlude()
            // Capture the track's window-space bounds for thumb geometry and drag hit-mapping.
            .child(
                canvas(
                    move |bounds, _, _| {
                        scrollbar_bounds.borrow_mut().insert(group_id, bounds);
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full(),
            )
            // A click on the track (outside the thumb) pages toward the click.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event: &MouseDownEvent, _, cx| {
                    app.scrollbar_track_click(group_id, session_id, event.position);
                    cx.stop_propagation();
                }),
            );

        if let Some(thumb) = thumb {
            let idle_alpha = if dragging_this {
                THUMB_ALPHA_DRAG
            } else {
                THUMB_ALPHA_IDLE
            };
            // A stable id lets GPUI track the hover state so the darker hover fill repaints promptly.
            let thumb_margin = (TERMINAL_SCROLLBAR_WIDTH - TERMINAL_SCROLLBAR_THUMB_WIDTH) / 2.;
            track = track.child(
                div()
                    .id(SharedString::from(format!("scrollbar-thumb-{group_id}")))
                    .absolute()
                    .top(px(thumb.top))
                    .right(px(thumb_margin))
                    .w(px(TERMINAL_SCROLLBAR_THUMB_WIDTH))
                    .h(px(thumb.height))
                    .rounded_full()
                    .bg(rgba((muted << 8) | idle_alpha))
                    .when(!dragging_this, |element| {
                        element.hover(|style| style.bg(rgba((muted << 8) | THUMB_ALPHA_HOVER)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app, event: &MouseDownEvent, _, cx| {
                            let thumb_top_window = app
                                .scrollbar_bounds
                                .borrow()
                                .get(&group_id)
                                .map(|bounds| f32::from(bounds.top()) + thumb.top);
                            if let Some(thumb_top_window) = thumb_top_window {
                                app.dragging_scrollbar = Some(ScrollbarDrag {
                                    group_id,
                                    session_id,
                                    grab_offset: f32::from(event.position.y) - thumb_top_window,
                                    pending_offset: render_offset,
                                });
                            }
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        Some(track.into_any_element())
    }

    /// Attach the manual window-drag handlers Eggie uses under `app_owns_titlebar_drag`.
    ///
    /// On macOS the `WindowControlArea::Drag` marker is a no-op; dragging is driven entirely by
    /// these handlers (press arms `moving_window`, the first move starts the OS drag, and a
    /// double click zooms). The marker is kept for Windows portability. Apply this only to blank
    /// sibling elements without interactive children, so tab clicks and drag-reordering stay
    /// unaffected by the drag hitbox.
    fn with_window_drag_handlers(element: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        element
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.moving_window = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.moving_window = false),
            )
            .on_mouse_move(cx.listener(|app, _, window, _| {
                if app.moving_window {
                    app.moving_window = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
    }

    fn render_window_drag_region(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut titlebar = Self::with_window_drag_handlers(
            div()
                .id("window-drag-region")
                .flex()
                .flex_none()
                .items_center()
                .justify_end()
                .w_full()
                .h(px(TITLEBAR_HEIGHT))
                .pl(px(72.))
                .pr_2(),
            cx,
        );
        if !self.left_sidebar_collapsed {
            if let Some(indicator) = self.render_update_indicator(cx) {
                titlebar = titlebar.child(indicator);
            }
            titlebar = titlebar.child(icon_button(
                IconName::PanelLeftOpen,
                "collapse-left-sidebar",
                self.colors,
                Some(self.language.collapse_sidebar_tooltip().into()),
                cx.listener(|app, _, _, cx| {
                    cx.stop_propagation();
                    app.toggle_left_sidebar(cx);
                }),
            ));
        }
        titlebar.into_any_element()
    }

    /// The update indicator in the title bar (left of the collapse-sidebar button): an icon for
    /// the current update state, plus a mini progress bar while downloading. Hidden when idle.
    fn render_update_indicator(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use eggie_update::UpdateState;

        let state = self.updates.read(cx).state().clone();
        let (icon_name, tooltip, tinted): (IconName, SharedString, bool) = match &state {
            UpdateState::Available(_) => (
                IconName::Download,
                self.language.update_available_tooltip().into(),
                true,
            ),
            UpdateState::Downloading { .. } => (
                IconName::Download,
                self.language.update_downloading().into(),
                false,
            ),
            UpdateState::Ready { .. } => (
                IconName::CheckmarkCircle,
                self.language.update_ready_tooltip().into(),
                true,
            ),
            UpdateState::Error(_) | UpdateState::UpToDate => (
                IconName::AlertCircle,
                self.language.update_error_title().into(),
                false,
            ),
            _ => return None,
        };
        let colors = self.colors;
        let opens_window = matches!(state, UpdateState::Available(_));
        let control = cx.entity().downgrade();

        let mut indicator = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_0p5()
            .px_1()
            .h_full()
            // Capture on-screen bounds so the popover can anchor just below the icon.
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let control = control.clone();
                cx.defer(move |cx| {
                    control
                        .update(cx, |app, cx| {
                            if app.update_toggle_bounds != Some(bounds) {
                                app.update_toggle_bounds = Some(bounds);
                                if app.update_popover_open {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .child(
                div()
                    .id("update-indicator-toggle")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded_lg()
                    .when(tinted, |this| this.text_color(rgb(colors.accent)))
                    .when(!tinted, |this| this.text_color(rgb(colors.muted)))
                    .hover(move |element| {
                        element
                            .bg(rgb(colors.panel_alt))
                            .text_color(rgb(colors.text))
                    })
                    .tooltip(tooltip_text(tooltip, colors))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if opens_window {
                            crate::update_window::open_update_window(
                                app.updates.clone(),
                                app.settings.clone(),
                                cx,
                            );
                        } else {
                            app.update_popover_open = !app.update_popover_open;
                            cx.notify();
                        }
                        let _ = window;
                    }))
                    .child(icon(icon_name)),
            );

        // Mini progress bar under the icon while downloading.
        if let UpdateState::Downloading { progress, .. } = &state {
            let total = progress.total.unwrap_or(0).max(1);
            let fraction = (progress.downloaded as f32 / total as f32).clamp(0., 1.);
            indicator = indicator.child(
                div()
                    .w(px(20.))
                    .h(px(2.))
                    .rounded_full()
                    .bg(rgb(colors.panel_alt))
                    .child(
                        div()
                            .h(px(2.))
                            .rounded_full()
                            .bg(rgb(colors.accent))
                            .w(px(20. * fraction)),
                    ),
            );
        }

        Some(indicator.into_any_element())
    }

    /// Popover for the download / ready / error states, anchored under the update indicator.
    fn render_update_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use eggie_update::UpdateState;

        if !self.update_popover_open {
            return None;
        }
        let state = self.updates.read(cx).state().clone();
        if matches!(state, UpdateState::Idle | UpdateState::Checking | UpdateState::Available(_)) {
            return None;
        }
        let colors = self.colors;
        let language = self.language;

        let body: AnyElement = match &state {
            UpdateState::Downloading { progress, .. } => {
                let total = progress.total.unwrap_or(0).max(1);
                let fraction = (progress.downloaded as f32 / total as f32).clamp(0., 1.);
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors.text))
                            .child(language.update_downloading()),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(5.))
                            .rounded_full()
                            .bg(rgb(colors.panel_alt))
                            .child(
                                div()
                                    .h(px(5.))
                                    .rounded_full()
                                    .bg(rgb(colors.accent))
                                    .w(relative(fraction)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(div().child(format!(
                                "{}/s",
                                format_update_speed(progress.bytes_per_sec)
                            )))
                            .child(div().child(format!("{:.0}%", fraction * 100.))),
                    )
                    .into_any_element()
            }
            UpdateState::Ready { release, .. } => {
                let compatible = release.daemon_compatible();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors.text))
                            .child(if compatible {
                                language.update_ready_restart()
                            } else {
                                language.update_ready_restart_with_daemon()
                            }),
                    )
                    .when(!compatible, |this| {
                        this.child(
                            div()
                                .p_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(0xd08700))
                                .bg(rgba(0xd0870022))
                                .text_size(px(11.))
                                .text_color(rgb(0xb87400))
                                .child(language.update_daemon_warning()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                popover_button(
                                    language.update_later(),
                                    false,
                                    colors,
                                    cx.listener(|app, _, _, cx| {
                                        app.update_popover_open = false;
                                        app.updates
                                            .update(cx, |controller, cx| controller.dismiss(cx));
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(popover_button(
                                if compatible {
                                    language.update_restart()
                                } else {
                                    language.update_restart_with_daemon()
                                },
                                true,
                                colors,
                                cx.listener(|app, _, _, cx| {
                                    app.updates.update(cx, |controller, cx| {
                                        if let Err(error) =
                                            controller.install_and_restart(cx)
                                        {
                                            controller.report_error(format!("{error:#}"), cx);
                                        }
                                    });
                                }),
                            )),
                    )
                    .into_any_element()
            }
            UpdateState::Error(message) => {
                let message = message.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0xff6b6b))
                            .child(format!("{}: {message}", language.update_error_title())),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .child(popover_button(
                                language.update_retry(),
                                true,
                                colors,
                                cx.listener(|app, _, _, cx| {
                                    app.update_popover_open = false;
                                    app.updates
                                        .update(cx, |controller, cx| controller.check(false, cx));
                                    cx.notify();
                                }),
                            )),
                    )
                    .into_any_element()
            }
            UpdateState::UpToDate => div()
                .text_size(px(13.))
                .text_color(rgb(colors.text))
                .child(language.update_up_to_date())
                .into_any_element(),
            _ => div().into_any_element(),
        };

        let popover = div()
            .flex()
            .flex_col()
            .w(px(260.))
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |app, _, _, cx| {
                if app.update_popover_open {
                    app.update_popover_open = false;
                    cx.notify();
                }
            }))
            .child(body);

        let anchored = match self.update_toggle_bounds {
            Some(bounds) => anchored()
                .anchor(Anchor::TopRight)
                .position(point(bounds.right(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(popover),
            None => anchored()
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(px(8.))
                .child(popover),
        };
        Some(deferred(anchored).priority(2).into_any_element())
    }

    fn render_sidebar_resize_handle(
        &self,
        edge: SidebarEdge,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match edge {
            SidebarEdge::Left => "resize-left-sidebar",
            SidebarEdge::Right => "resize-right-sidebar",
        };
        div()
            .id(id)
            .flex_none()
            .w(px(SIDEBAR_RESIZE_HANDLE_WIDTH))
            .h_full()
            .ml(px(-SIDEBAR_RESIZE_HANDLE_WIDTH / 2.))
            .mr(px(-SIDEBAR_RESIZE_HANDLE_WIDTH / 2.))
            .cursor_col_resize()
            .hover(|element| element.bg(rgb(self.colors.accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event, _, cx| app.start_sidebar_resize(edge, event, cx)),
            )
            .into_any_element()
    }

    fn render_split_divider(
        &self,
        axis: SplitAxis,
        split_id: SplitId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let horizontal = axis == SplitAxis::Horizontal;
        // The handle is a wide, transparent grab area so it is easy to click, while the visible
        // line inside it stays thin. Filling the whole grab area with the accent color would make
        // the divider look far too thick.
        let handle = SPLIT_DIVIDER_HANDLE_WIDTH;
        let line = SPLIT_DIVIDER_LINE_WIDTH;
        let group_name: SharedString = format!("split-divider-{split_id}").into();
        let mut divider = div()
            .id(format!("split-divider-{split_id}"))
            .group(group_name.clone())
            .flex_none()
            .flex()
            .items_center()
            .justify_center();
        let mut visible = div();
        if horizontal {
            // Symmetric negative margins straddle the handle across the seam so it is centered on
            // it and consumes zero net layout width (same trick as the sidebar resize handle).
            // Rendered after the first pane, it stacks above both panes and receives mouse events.
            divider = divider
                .w(px(handle))
                .h_full()
                .ml(px(-handle / 2.))
                .mr(px(-handle / 2.))
                .cursor_col_resize();
            visible = visible.w(px(line)).h_full();
        } else {
            divider = divider
                .h(px(handle))
                .w_full()
                .mt(px(-handle / 2.))
                .mb(px(-handle / 2.))
                .cursor_row_resize();
            visible = visible.h(px(line)).w_full();
        }
        divider
            .child(
                // A thin line, centered on the seam, that lights up in the accent color while the
                // grab area is hovered. `group_hover` reacts to hovering the surrounding handle,
                // not just the line itself.
                visible.group_hover(group_name, |style| style.bg(rgb(self.colors.accent))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, _, _, cx| app.start_split_resize(split_id, cx)),
            )
            .into_any_element()
    }

    fn render_traffic_light_tab_bar_region(
        &self,
        group_id: GroupId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Self::with_window_drag_handlers(
            div()
                .id(format!("traffic-light-tab-bar-{group_id}"))
                .flex_none()
                .w(px(TRAFFIC_LIGHT_INSET_WIDTH))
                .h_full(),
            cx,
        )
        .into_any_element()
    }

    fn show_native_tab_context_menu(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_item(group_id, item_id, window, cx);
        cx.stop_propagation();
        let directions = [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ];
        let move_enabled = directions.map(|direction| {
            self.workspace()
                .layout
                .neighbor_group_id(group_id, direction)
                .is_some()
        });
        let Some(menu) = prepare_tab_menu(window, move_enabled, self.language) else {
            return;
        };
        // AppKit menus are modal and run a nested event loop. Start the menu from a foreground
        // future after this GPUI listener has returned so polling tasks cannot re-enter the App
        // while its entity RefCell is still mutably borrowed.
        cx.spawn(async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            this.update(cx, |app, cx| {
                let item_still_exists = app
                    .workspace()
                    .layout
                    .find_group(group_id)
                    .is_some_and(|group| group.items.iter().any(|item| item.id == item_id));
                if !item_still_exists {
                    return;
                }
                match command {
                    NativeTabMenuCommand::Split(direction) => {
                        app.new_terminal(group_id, Some(direction), cx);
                    }
                    NativeTabMenuCommand::Move(direction) => {
                        app.move_tab_in_direction(group_id, item_id, direction, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn move_tab_in_direction(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        direction: Direction,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace_mut();
        let Some(target_group_id) = workspace.layout.neighbor_group_id(group_id, direction) else {
            cx.notify();
            return;
        };
        if workspace
            .layout
            .move_item(group_id, item_id, target_group_id)
            .is_some()
        {
            workspace.active_group_id = target_group_id;
        }
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    fn clear_tab_drop_target(&mut self, cx: &mut Context<Self>) {
        if self.tab_drop_target.take().is_some() {
            cx.notify();
        }
    }

    fn update_content_drop_target(
        &mut self,
        group_id: GroupId,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }

        let x = f32::from(event.event.position.x - event.bounds.left());
        let group_y = f32::from(event.event.position.y - event.bounds.top());
        if group_y < TAB_BAR_HEIGHT {
            return;
        }

        let dragged_tab = event
            .dragged_item()
            .downcast_ref::<DraggedTab>()
            .expect("drag move type must match DraggedTab");
        let source_item_count = self
            .workspace()
            .layout
            .find_group(dragged_tab.source_group_id)
            .map_or(0, |group| group.items.len());
        if !should_show_content_drop_target(
            dragged_tab.source_group_id,
            group_id,
            source_item_count,
        ) {
            self.clear_tab_drop_target(cx);
            return;
        }

        let width = f32::from(event.bounds.size.width);
        let height = (f32::from(event.bounds.size.height) - TAB_BAR_HEIGHT).max(0.);
        let y = group_y - TAB_BAR_HEIGHT;
        let zone = content_drop_zone(width, height, x, y);
        let target = Some(TabDropTarget { group_id, zone });
        if self.tab_drop_target != target {
            self.tab_drop_target = target;
            cx.notify();
        }
    }

    fn update_tab_bar_drop_target(
        &mut self,
        group_id: GroupId,
        tab_index: usize,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let relative_x = f32::from(event.event.position.x - event.bounds.left());
        let insertion_index =
            tab_bar_insertion_index(tab_index, f32::from(event.bounds.size.width), relative_x);
        let target = Some(TabDropTarget {
            group_id,
            zone: TabDropZone::TabBar { insertion_index },
        });
        if self.tab_drop_target != target {
            self.tab_drop_target = target;
            cx.notify();
        }
    }

    fn update_tab_bar_trailing_drop_target(
        &mut self,
        group_id: GroupId,
        insertion_index: usize,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let target = Some(TabDropTarget {
            group_id,
            zone: TabDropZone::TabBar { insertion_index },
        });
        if self.tab_drop_target != target {
            self.tab_drop_target = target;
            cx.notify();
        }
    }

    fn handle_tab_drop(&mut self, dragged_tab: &DraggedTab, cx: &mut Context<Self>) {
        let Some(target) = self.tab_drop_target.take() else {
            return;
        };
        let workspace = self.workspace_mut();
        let destination_group = match target.zone {
            TabDropZone::Center => workspace.layout.move_item(
                dragged_tab.source_group_id,
                dragged_tab.item_id,
                target.group_id,
            ),
            TabDropZone::Edge(direction) => workspace.layout.move_item_to_new_group(
                dragged_tab.source_group_id,
                dragged_tab.item_id,
                target.group_id,
                direction,
            ),
            TabDropZone::TabBar { insertion_index } => workspace.layout.move_item_at(
                dragged_tab.source_group_id,
                dragged_tab.item_id,
                target.group_id,
                insertion_index,
            ),
        };
        if let Some(destination_group) = destination_group {
            workspace.active_group_id = destination_group;
        }
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    /// Order a project's sessions to match the reading order of its layout tree — left-to-right,
    /// top-to-bottom, and within a tab group by tab order. This keeps the sidebar list in sync with
    /// how the terminals are actually arranged in the main area (splits and tab reordering), rather
    /// than showing them in creation order.
    ///
    /// Sessions that belong to the project but are absent from the layout tree (which should not
    /// normally happen) are appended afterwards in their existing order so a terminal can never
    /// silently disappear from the sidebar.
    fn sidebar_sessions_in_layout_order<'a>(
        &'a self,
        project_id: ProjectId,
        view: &'a WindowProjectView,
    ) -> Vec<&'a SessionSummary> {
        // Reading-order position of each session within the layout tree.
        let mut layout_rank: HashMap<SessionId, usize> = HashMap::new();
        for item in view.layout.terminal_items() {
            if let Some(session_id) = item.session_id {
                let next_rank = layout_rank.len();
                layout_rank.entry(session_id).or_insert(next_rank);
            }
        }
        let orphan_rank = layout_rank.len();

        let mut sessions: Vec<&SessionSummary> = self
            .sessions
            .iter()
            .filter(|session| session.project_id == project_id)
            .collect();

        // Stable sort keeps orphan sessions (rank = len) in their original relative order, appended
        // after everything the layout knows about.
        sessions.sort_by_key(|session| layout_rank.get(&session.id).copied().unwrap_or(orphan_rank));
        sessions
    }

    /// The button next to "add project" that opens the detached-session popover. Rendered only when
    /// there is at least one detached session (D6), with a count badge.
    fn render_detached_sessions_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.detached_sessions.is_empty() {
            return None;
        }
        let count = self.detached_sessions.len();
        let colors = self.colors;
        let tooltip: SharedString = self.language.detached_sessions_tooltip().into();
        let control = cx.entity().downgrade();
        Some(
            div()
                // Capture the toggle's on-screen bounds so the popover can anchor just below it.
                .on_children_prepainted(move |children, _, cx| {
                    let Some(bounds) = children.first().copied() else {
                        return;
                    };
                    let control = control.clone();
                    cx.defer(move |cx| {
                        control
                            .update(cx, |app, cx| {
                                if app.detached_toggle_bounds != Some(bounds) {
                                    app.detached_toggle_bounds = Some(bounds);
                                    if app.detached_popover_open {
                                        cx.notify();
                                    }
                                }
                            })
                            .ok();
                    });
                })
                .child(
                    div()
                        .id("detached-sessions-toggle")
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .h(px(24.))
                        .px_2()
                        .rounded_lg()
                        .text_color(rgb(colors.muted))
                        .cursor_pointer()
                        .hover(move |element| {
                            element
                                .bg(rgb(colors.panel_alt))
                                .text_color(rgb(colors.text))
                        })
                        .tooltip(tooltip_text(tooltip, colors))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.detached_popover_open = !app.detached_popover_open;
                            cx.notify();
                        }))
                        .child(icon_sized(IconName::Terminal, 14.))
                        .child(div().text_size(px(11.)).child(count.to_string())),
                )
                .into_any_element(),
        )
    }

    /// The popover listing detached (unowned) sessions grouped by project, each with claim/destroy
    /// actions (D6). Uses the same anchored + deferred + `.occlude()` overlay pattern as the settings
    /// dropdowns; `.occlude()` stops clicks on blank areas from falling through to the sidebar.
    fn render_detached_sessions_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.detached_popover_open || self.detached_sessions.is_empty() {
            return None;
        }
        let colors = self.colors;
        let claim_tooltip: SharedString = self.language.claim_session_tooltip().into();
        let destroy_tooltip: SharedString = self.language.destroy_session_tooltip().into();

        // Group detached sessions by project, preserving the shared project order; sessions whose
        // project no longer exists fall into a trailing "other" group.
        let mut groups: Vec<(String, Vec<&SessionSummary>)> = Vec::new();
        let mut grouped_projects: HashSet<ProjectId> = HashSet::new();
        for project in &self.projects {
            let members: Vec<&SessionSummary> = self
                .detached_sessions
                .iter()
                .filter(|session| session.project_id == project.id)
                .collect();
            if !members.is_empty() {
                grouped_projects.insert(project.id);
                groups.push((project.name.clone(), members));
            }
        }
        let mut orphans: Vec<&SessionSummary> = self
            .detached_sessions
            .iter()
            .filter(|session| !grouped_projects.contains(&session.project_id))
            .collect();
        if !orphans.is_empty() {
            orphans.sort_by(|a, b| a.initial_directory.cmp(&b.initial_directory));
            groups.push((self.language.detached_other_group().to_owned(), orphans));
        }

        let mut list = div().flex().flex_col().py_1();
        for (title, members) in groups {
            list = list.child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(rgb(colors.muted))
                    .child(title),
            );
            for session in members {
                let session_id = session.id;
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .mx_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .hover(move |element| element.bg(rgb(colors.hover)))
                        .child(icon_sized(IconName::Terminal, 14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .child(session.title.clone()),
                        )
                        .child(icon_button(
                            IconName::CombineIcon,
                            SharedString::from(format!("claim-detached-{session_id}")),
                            colors,
                            Some(claim_tooltip.clone()),
                            cx.listener(move |app, _, window, cx| {
                                app.claim_detached_session(session_id, window, cx)
                            }),
                        ))
                        .child(icon_button(
                            IconName::Close,
                            SharedString::from(format!("destroy-detached-{session_id}")),
                            colors,
                            Some(destroy_tooltip.clone()),
                            cx.listener(move |app, _, _, cx| {
                                app.destroy_detached_session(session_id, cx)
                            }),
                        )),
                );
            }
        }

        let popover = div()
            .flex()
            .flex_col()
            .w(px(260.))
            .max_h(px(360.))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            // Floating overlay: block mouse events for the sidebar rows behind it so clicks on the
            // popover's blank areas don't bubble through (see [[gpui-overlay-occlude]]).
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |app, _, cx| {
                    if app.detached_popover_open {
                        app.detached_popover_open = false;
                        cx.notify();
                    }
                });
            }))
            .child(list);

        // Anchor the popover just below the toggle button. Anchoring the popover's top-right corner
        // to the button's bottom-right opens it downward and leftward, keeping it inside the sidebar.
        // Fall back to a window-snapped corner until the button's bounds have been captured once.
        let anchored_popover = match self.detached_toggle_bounds {
            Some(bounds) => anchored()
                .anchor(Anchor::TopRight)
                .position(point(bounds.right(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(popover),
            None => anchored()
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(px(8.))
                .child(popover),
        };

        Some(deferred(anchored_popover).priority(2).into_any_element())
    }

    /// The copy-path + open-with split button pair shown under a directory row in the Info tab. The
    /// open button's left half launches the persisted [`OpenWith`] app on `path`; its right half is a
    /// caret that toggles a dropdown to pick (and persist) a different opener.
    fn render_directory_actions(
        &self,
        field: DirectoryField,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let language = self.language;
        let opener = self.settings.read(cx).config().open_directory_with;
        let slug = field.slug();

        let copy_value = path.display().to_string();
        let copy_button = div()
            .id(SharedString::from(format!("copy-path-{slug}")))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(24.))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .text_color(rgb(colors.muted))
            .cursor_pointer()
            .hover(move |element| {
                element
                    .bg(rgb(colors.panel_alt))
                    .text_color(rgb(colors.text))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_value.clone()));
            }))
            .child(icon_sized(IconName::Copy, 14.))
            .child(div().text_size(px(12.)).child(language.copy_path()));

        let open_path = path.to_path_buf();
        // Left half: launch the persisted opener on the directory.
        let open_action = div()
            .id(SharedString::from(format!("open-with-{slug}")))
            .flex()
            .flex_1()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .cursor_pointer()
            .hover(move |element| element.bg(rgb(colors.panel_alt)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, _| {
                open_path_with(&open_path, opener);
            }))
            .child(icon_sized(icon_for_open_with(opener), 14.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .child(open_with_label(language, opener)),
            );

        // Right half: a caret that toggles this row's opener dropdown.
        let caret = div()
            .id(SharedString::from(format!("open-with-caret-{slug}")))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h_full()
            .border_l_1()
            .border_color(rgb(colors.border))
            .cursor_pointer()
            .hover(move |element| element.bg(rgb(colors.panel_alt)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |app, _, _, cx| {
                app.open_with_dropdown = if app.open_with_dropdown == Some(field) {
                    None
                } else {
                    Some(field)
                };
                cx.notify();
            }))
            .child(icon_sized(IconName::ArrowDown, 12.));

        let control = cx.entity().downgrade();
        let mut open_button = div()
            // Capture the split-button's bounds so the dropdown can anchor just below it.
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let control = control.clone();
                cx.defer(move |cx| {
                    control
                        .update(cx, |app, cx| {
                            if app.open_with_dropdown_bounds.get(&field) != Some(&bounds) {
                                app.open_with_dropdown_bounds.insert(field, bounds);
                                if app.open_with_dropdown == Some(field) {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(24.))
                    .rounded_lg()
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .text_color(rgb(colors.muted))
                    .child(open_action)
                    .child(caret),
            );
        if let Some(dropdown) = self.render_open_with_dropdown(field, cx) {
            open_button = open_button.child(dropdown);
        }

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(copy_button)
            .child(open_button)
            .into_any_element()
    }

    /// The dropdown listing the available [`OpenWith`] apps for `field`, anchored below its split
    /// button. Selecting an option persists it as the new default opener. Same anchored + deferred +
    /// `.occlude()` overlay pattern as the settings dropdowns.
    fn render_open_with_dropdown(
        &self,
        field: DirectoryField,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.open_with_dropdown != Some(field) {
            return None;
        }
        let colors = self.colors;
        let language = self.language;
        let selected = self.settings.read(cx).config().open_directory_with;
        let slug = field.slug();

        let mut list = div().flex().flex_col().py_1();
        for option in OpenWith::ALL {
            let is_selected = option == selected;
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "open-with-{slug}-{}",
                        option.slug()
                    )))
                    .flex()
                    .items_center()
                    .gap_2()
                    .mx_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(is_selected, |element| {
                        element
                            .bg(rgb(colors.panel_alt))
                            .text_color(rgb(colors.accent))
                    })
                    .hover(move |element| element.bg(rgb(colors.panel_alt)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_open_directory_with(option, cx);
                        app.open_with_dropdown = None;
                        cx.notify();
                    }))
                    .child(icon_sized(icon_for_open_with(option), 14.))
                    .child(div().text_size(px(12.)).child(open_with_label(language, option))),
            );
        }

        let popover = div()
            .flex()
            .flex_col()
            .w(px(160.))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            // Floating overlay: block clicks on blank areas from bubbling through (see
            // [[gpui-overlay-occlude]]).
            .occlude()
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |app, _, cx| {
                    if app.open_with_dropdown == Some(field) {
                        app.open_with_dropdown = None;
                        cx.notify();
                    }
                });
            }))
            .child(list);

        // Anchor the dropdown just below the split button (top-left corner to the button's
        // bottom-left), falling back to a window-snapped corner until bounds are captured once.
        let anchored_dropdown = match self.open_with_dropdown_bounds.get(&field).copied() {
            Some(bounds) => anchored()
                .anchor(Anchor::TopLeft)
                .position(point(bounds.left(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(popover),
            None => anchored()
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(popover),
        };

        Some(deferred(anchored_dropdown).priority(2).into_any_element())
    }

    /// Persist a new default directory opener. No-op (and no save) if it is already selected.
    fn set_open_directory_with(&mut self, opener: OpenWith, cx: &mut Context<Self>) {
        if self.settings.read(cx).config().open_directory_with == opener {
            return;
        }
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |config| config.open_directory_with = opener,
                cx,
            );
        });
    }

    fn render_left_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.left_sidebar_width))
            .h_full()
            .overflow_hidden()
            .bg(rgb(self.colors.panel))
            .child(self.render_window_drag_region(cx))
            .child(
                div().flex().flex_col().px_3().pb_3().gap_3().child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(section_label(self.language.projects_label(), self.colors))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .children(self.render_detached_sessions_button(cx))
                                .child(icon_button(
                                    IconName::Add,
                                    "add-project",
                                    self.colors,
                                    Some(self.language.add_project_tooltip().into()),
                                    cx.listener(|app, _, window, cx| {
                                        app.prompt_add_project(window, cx)
                                    }),
                                )),
                        )
                        .children(self.render_detached_sessions_popover(cx))
                        .children(self.render_update_popover(cx)),
                ),
            );
        for (index, project) in self.projects.iter().enumerate() {
            let project_id = project.id;
            let Some(view) = self.project_views.get(&project_id) else {
                continue;
            };
            let collapsed = self.collapsed_projects.contains(&project_id);
            let is_active_project = project_id == self.active_project;
            panel = panel.child(
                div()
                    .id(format!("project-{index}"))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_2()
                    .mx_3()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .mb_1()
                    .when(is_active_project, |element| {
                        element.bg(rgb(self.colors.panel_alt))
                    })
                    .hover({
                        let colors = self.colors;
                        move |element| element.bg(rgb(colors.hover))
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.activate_project(project_id, cx);
                        app.toggle_project_collapsed(project_id, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |app, _, window, cx| {
                            app.show_project_context_menu(project_id, window, cx)
                        }),
                    )
                    .child(div().flex_none().child(icon(if collapsed {
                        IconName::Folder
                    } else {
                        IconName::FolderOpen
                    })))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(project.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(self.colors.muted))
                            .child(icon_sized(
                                if collapsed {
                                    IconName::ArrowRight
                                } else {
                                    IconName::ArrowDown
                                },
                                14.,
                            )),
                    ),
            );
            if !collapsed {
                for session in self.sidebar_sessions_in_layout_order(project_id, view) {
                    let active = is_active_project
                        && self.active_session_id() == Some(session.id);
                    let session_id = session.id;
                    panel = panel.child(
                        div()
                            .id(format!("sidebar-session-{session_id}"))
                            .flex()
                            .items_center()
                            .min_w_0()
                            .overflow_hidden()
                        .h(px(SIDEBAR_SESSION_ROW_HEIGHT))
                        .gap_2()
                        .ml_5()
                        .mr_3()
                        .px_2()
                        .rounded_md()
                        .mb_1()
                        .text_size(px(12.))
                        .text_color(rgb(if active {
                            self.colors.text
                        } else {
                            self.colors.muted
                        }))
                        .when(active, |element| {
                            element.bg(rgb(self.colors.panel_alt))
                        })
                        .when_some(self.bell_flash_tint(session_id), |element, tint| {
                            element.bg(rgba(tint))
                        })
                        .hover({
                            let colors = self.colors;
                            move |element| element.bg(rgb(colors.hover))
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.activate_session(project_id, session_id, window, cx)
                        }))
                        .child(self.render_item_icon(
                            ItemKind::Terminal,
                            Some(session_id),
                            format!("sidebar-progress-{session_id}"),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(session.title.clone()),
                        ),
                );
            }
        }
        }
        panel.into_any_element()
    }

    fn render_layout(&self, node: &LayoutNode, cx: &mut Context<Self>) -> AnyElement {
        self.render_layout_with_outer_edges(node, true, true, true, true, cx)
    }

    fn render_layout_with_outer_edges(
        &self,
        node: &LayoutNode,
        touches_left_edge: bool,
        touches_right_edge: bool,
        touches_bottom_edge: bool,
        touches_top_edge: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            LayoutNode::Group { group } => self.render_group(
                group,
                touches_left_edge,
                touches_right_edge,
                touches_bottom_edge,
                touches_top_edge,
                cx,
            ),
            LayoutNode::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                let horizontal = *axis == SplitAxis::Horizontal;
                let ratio = ratio.clamp(0.05, 0.95);
                // Identify the split by its own stable id, not the first group inside it: nested
                // splits share the same "first group" and would otherwise collide, so dragging an
                // inner divider would resize the outer split instead.
                let split_id = *id;
                let split_bounds = self.split_bounds.clone();
                div()
                    .flex()
                    .when(!horizontal, |element| element.flex_col())
                    .size_full()
                    .min_w_0()
                    .on_children_prepainted(move |children, _, _| {
                        if children.len() < 2 {
                            return;
                        }
                        // Use the first and last children (the two panes) to compute the split
                        // container's bounds. The middle child is the drag handle, whose negative
                        // margins would otherwise skew the union.
                        let bounds = children[0].union(&children[children.len() - 1]);
                        split_bounds.borrow_mut().insert(split_id, bounds);
                    })
                    .child(
                        div()
                            .flex_grow(ratio)
                            .flex_shrink_0()
                            // Force the flex basis to zero so `flex_grow` distributes the *entire*
                            // container along the split axis in proportion to `ratio`. Without this
                            // the panes keep their large intrinsic (content) size as the basis and
                            // `ratio` only splits the leftover space, so dragging barely moves them.
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_layout_with_outer_edges(
                                first,
                                touches_left_edge,
                                touches_right_edge && !horizontal,
                                touches_bottom_edge && horizontal,
                                touches_top_edge,
                                cx,
                            )),
                    )
                    .child(self.render_split_divider(*axis, split_id, cx))
                    .child(
                        div()
                            .flex_grow(1.0 - ratio)
                            .flex_shrink_0()
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_layout_with_outer_edges(
                                second,
                                touches_left_edge && !horizontal,
                                touches_right_edge,
                                touches_bottom_edge,
                                touches_top_edge && horizontal,
                                cx,
                            )),
                    )
                    .into_any_element()
            }
        }
    }

    fn render_group(
        &self,
        group: &TabGroup,
        touches_left_edge: bool,
        touches_right_edge: bool,
        touches_bottom_edge: bool,
        touches_top_edge: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_id = group.id;
        let is_top_left_group = self.workspace().layout.top_left_group_id() == group_id;
        let is_top_right_group = self.workspace().layout.top_right_group_id() == group_id;
        let tab_insertion_index = self.tab_drop_target.and_then(|target| {
            if target.group_id != group_id {
                return None;
            }
            match target.zone {
                TabDropZone::TabBar { insertion_index } => Some(insertion_index),
                TabDropZone::Center | TabDropZone::Edge(_) => None,
            }
        });
        let mut tabs = div()
            .flex()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .p(px(TAB_BAR_PADDING))
            .bg(rgb(self.colors.panel));
        if self.left_sidebar_collapsed && is_top_left_group {
            tabs = tabs
                .child(self.render_traffic_light_tab_bar_region(group_id, cx))
                .child(top_bar_icon_button(
                    IconName::PanelLeftClose,
                    format!("expand-left-sidebar-{group_id}"),
                    self.colors,
                    false,
                    false,
                    Some(self.language.expand_sidebar_tooltip().into()),
                    cx.listener(|app, _, _, cx| app.toggle_left_sidebar(cx)),
                ));
        }
        // Tabs and the trailing drag/drop strip live in a horizontally scrollable strip so tabs
        // keep their minimum width and overflow into a scroll instead of being squeezed. The
        // `+` and collapse controls stay outside this strip so they never scroll away.
        let scroll_handle = self
            .tab_bar_scroll_handles
            .borrow_mut()
            .entry(group_id)
            .or_insert_with(ScrollHandle::new)
            .clone();
        let mut tabs_scroll = div()
            .id(format!("tabbar-scroll-{group_id}"))
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(&scroll_handle);
        for (tab_index, item) in group.items.iter().enumerate() {
            let item_id = item.id;
            let active = group.active_item_id == Some(item.id);
            // The first tab's "before" indicator sits at a negative offset that the scroll strip's
            // overflow would clip, so it is drawn once on the non-clipping scroll wrapper instead
            // (see below). Every other tab draws its own indicator in the preceding gap.
            let show_insertion_before = tab_insertion_index == Some(tab_index) && tab_index != 0;
            let show_insertion_after = tab_insertion_index == Some(group.items.len())
                && tab_index + 1 == group.items.len();
            let dragged_tab = DraggedTab {
                source_group_id: group_id,
                item_id,
                item_kind: item.kind,
                title: item.title.clone().into(),
                active,
                colors: self.colors,
            };
            tabs_scroll = tabs_scroll.child(
                div()
                    .id(format!("tab-{item_id}"))
                    .flex()
                    .items_center()
                    .relative()
                    .flex_shrink_0()
                    .min_w(px(TAB_MIN_WIDTH))
                    .max_w(px(TAB_MAX_WIDTH))
                    .h_full()
                    .pl_2()
                    .pr_1()
                    .gap_2()
                    .text_size(px(TAB_TITLE_FONT_SIZE))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .when(tab_index + 1 < group.items.len(), |element| {
                        element.mr(px(TAB_GAP))
                    })
                    .when(active, |element| element.bg(rgb(self.colors.panel_alt)))
                    .when_some(
                        item.session_id.and_then(|sid| self.bell_flash_tint(sid)),
                        |element, tint| element.bg(rgba(tint)),
                    )
                    .cursor_pointer()
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.activate_item(group_id, item_id, window, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |app, _, window, cx| {
                            app.show_native_tab_context_menu(group_id, item_id, window, cx);
                        }),
                    )
                    .on_drag(dragged_tab, |tab, _, _, cx| {
                        cx.new(|_| DraggedTabPreview { tab: tab.clone() })
                    })
                    .on_drag_move::<DraggedTab>(cx.listener(move |app, event, window, cx| {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                        app.update_tab_bar_drop_target(group_id, tab_index, event, cx)
                    }))
                    .on_drop(cx.listener(|app, dragged_tab: &DraggedTab, _, cx| {
                        app.handle_tab_drop(dragged_tab, cx)
                    }))
                    .child(self.render_item_icon(
                        item.kind,
                        item.session_id,
                        format!("tab-progress-{item_id}"),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(item.title.clone()),
                    )
                    .child(
                        div()
                            .id(format!("close-tab-{item_id}"))
                            .flex_none()
                            .text_color(rgb(self.colors.muted))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.close_item(group_id, item_id, cx)
                            }))
                            .child(icon_sized(IconName::Close, TAB_CLOSE_ICON_SIZE)),
                    )
                    .when(show_insertion_before, |element| {
                        // Centered in the gap before this tab. (The first tab is handled on the
                        // scroll wrapper to avoid overflow clipping — see `show_insertion_before`.)
                        element.child(
                            div()
                                .absolute()
                                .left(px(-TAB_DROP_INDICATOR_OFFSET))
                                .top_0()
                                .bottom_0()
                                .w(px(TAB_DROP_INDICATOR_WIDTH))
                                .bg(rgb(self.colors.accent)),
                        )
                    })
                    .when(show_insertion_after, |element| {
                        element.child(
                            div()
                                .absolute()
                                .right(px(-TAB_DROP_INDICATOR_OFFSET))
                                .top_0()
                                .bottom_0()
                                .w(px(TAB_DROP_INDICATOR_WIDTH))
                                .bg(rgb(self.colors.accent)),
                        )
                    }),
            );
        }
        let item_count = group.items.len();
        let show_empty_group_insertion = item_count == 0 && tab_insertion_index == Some(0);
        let mut trailing_space = div()
            .id(format!("tabbar-trailing-space-{group_id}"))
            .flex_1()
            .relative()
            .h_full()
            .on_drag_move::<DraggedTab>(cx.listener(move |app, event, window, cx| {
                cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                app.update_tab_bar_trailing_drop_target(group_id, item_count, event, cx)
            }))
            .on_drop(cx.listener(|app, dragged_tab: &DraggedTab, _, cx| {
                app.handle_tab_drop(dragged_tab, cx)
            }))
            .when(show_empty_group_insertion, |element| {
                element.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(TAB_DROP_INDICATOR_WIDTH))
                        .bg(rgb(self.colors.accent)),
                )
            });
        // Only a tab bar flush against the top window edge doubles as a drag region; a lower
        // group's tab bar sits mid-window and must not move the window.
        if touches_top_edge {
            trailing_space = Self::with_window_drag_handlers(trailing_space, cx);
        }
        // Fade masks on the left/right edges of the scroll strip indicate that more tabs are
        // available off-screen. They are plain visual overlays with no hitbox, so wheel/trackpad
        // scrolling and tab clicks pass straight through to the scroll container beneath.
        let offset_x = -f32::from(scroll_handle.offset().x);
        let max_offset_x = f32::from(scroll_handle.max_offset().x);
        let show_left_mask = offset_x > 0.5;
        let show_right_mask = offset_x < max_offset_x - 0.5;
        let panel_color = rgb(self.colors.panel);
        let mask_width = px(24.);
        let mut scroll_wrapper = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(tabs_scroll.child(trailing_space));
        if show_left_mask {
            scroll_wrapper = scroll_wrapper.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(mask_width)
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    )),
            );
        }
        if show_right_mask {
            scroll_wrapper = scroll_wrapper.child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(mask_width)
                    .bg(linear_gradient(
                        270.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    )),
            );
        }
        // The indicator for inserting before the FIRST tab lives on the (non-clipping) scroll
        // wrapper rather than inside the scroll strip: at the strip's x=0 origin its negative
        // offset would fall outside the overflow viewport and be clipped away. Drawn here it lands
        // at the very same spot — in the tab bar's left padding — without being cut off.
        if item_count > 0 && tab_insertion_index == Some(0) {
            scroll_wrapper = scroll_wrapper.child(
                div()
                    .absolute()
                    .left(px(-TAB_DROP_INDICATOR_OFFSET))
                    .top_0()
                    .bottom_0()
                    .w(px(TAB_DROP_INDICATOR_WIDTH))
                    .bg(rgb(self.colors.accent)),
            );
        }
        // A fixed gap between the scrollable tab strip and the new-tab button gives the user a
        // reliable grab handle to move the window, even when the tab bar is packed full.
        let mut tab_bar_drag_gap = div()
            .id(format!("tabbar-drag-gap-{group_id}"))
            .flex_none()
            .w(px(40.))
            .h_full();
        if touches_top_edge {
            tab_bar_drag_gap = Self::with_window_drag_handlers(tab_bar_drag_gap, cx);
        }
        tabs = tabs
            .child(scroll_wrapper)
            .child(tab_bar_drag_gap)
            .child(top_bar_icon_button(
                IconName::Add,
                format!("new-terminal-{group_id}"),
                self.colors,
                false,
                false,
                Some(self.language.new_terminal_tooltip().into()),
                cx.listener(move |app, _, _, cx| app.new_terminal(group_id, None, cx)),
            ));
        if is_top_right_group {
            let right_sidebar_icon = if self.right_sidebar_collapsed {
                IconName::PanelRightOpen
            } else {
                IconName::PanelRightClose
            };
            tabs = tabs.child(top_bar_icon_button(
                right_sidebar_icon,
                format!("toggle-right-sidebar-{group_id}"),
                self.colors,
                false,
                false,
                Some(self.language.toggle_right_sidebar_tooltip().into()),
                cx.listener(|app, _, _, cx| app.toggle_right_sidebar(cx)),
            ));
        }

        let content_drop_zone = self.tab_drop_target.and_then(|target| {
            if target.group_id != group_id {
                return None;
            }
            match target.zone {
                TabDropZone::Center => Some(TabDropZone::Center),
                TabDropZone::Edge(direction) => Some(TabDropZone::Edge(direction)),
                TabDropZone::TabBar { .. } => None,
            }
        });
        let terminal_captures_mouse = group
            .active_item()
            .and_then(|item| item.session_id)
            .and_then(|session_id| self.snapshots.get(&session_id))
            .is_some_and(|snapshot| snapshot.input_modes.captures_mouse());
        // Whether an auto-detected URL is hovered in this group's active session (pointer cursor).
        let link_hovered = group
            .active_item()
            .and_then(|item| item.session_id)
            .zip(self.terminal_hovered_link.as_ref())
            .is_some_and(|(session_id, (hovered_session, _))| session_id == *hovered_session);
        let content = group
            .active_item()
            .and_then(|item| item.session_id)
            .and_then(|session_id| self.snapshots.get(&session_id).cloned())
            .map(|snapshot| {
                let session_id = snapshot.session_id;
                let snapshot_size = snapshot.size;
                // The daemon projects its authoritative selection into the viewport and ships the
                // range inside the snapshot, so the highlight always matches this frame's cells.
                let selection = snapshot.selection.map(|range| TerminalSelection {
                    anchor: TerminalPoint {
                        line: range.start.line,
                        column: range.start.column,
                    },
                    head: TerminalPoint {
                        line: range.end.line,
                        column: range.end.column,
                    },
                });
                let ime = self.terminal_ime_states.get(&session_id).cloned();
                let search = self
                    .terminal_search_matches(session_id)
                    .map(TerminalSearchHighlights::from_result);
                // Underline the hovered auto-detected URL, but only for the session it belongs to.
                let url_hover = self
                    .terminal_hovered_link
                    .as_ref()
                    .filter(|(hovered_session, _)| *hovered_session == session_id)
                    .map(|(_, link)| link.clone());
                let app = cx.entity().downgrade();
                let input = (self.workspace().active_group_id == group_id)
                    .then(|| TerminalInputContext::new(app.clone(), self.focus_handle.clone()));
                // Resolve the blink phase before `snapshot` is moved into the renderer.
                let cursor_visible = self.cursor_visible_this_frame(&snapshot);
                let metric_adjustments = self.terminal_metric_adjustments;
                div()
                    .size_full()
                    .overflow_hidden()
                    .bg(rgba(terminal_background(&snapshot, self.terminal_theme)))
                    .px(px(self.terminal_padding_x))
                    .py(px(self.terminal_padding_y))
                    .font_family(self.terminal_font_family.clone())
                    .text_size(px(self.terminal_font_size))
                    .on_children_prepainted(move |children, window, cx| {
                        let Some(bounds) = children.first().copied() else {
                            return;
                        };
                        let (cell_width, line_height) =
                            terminal_cell_metrics(window, metric_adjustments);
                        let size = terminal_size_for_viewport(
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            f32::from(cell_width),
                            f32::from(line_height),
                            window.scale_factor(),
                        );
                        let app = app.clone();
                        cx.defer(move |cx| {
                            app.update(cx, |app, cx| {
                                app.record_terminal_viewport(
                                    group_id,
                                    session_id,
                                    bounds,
                                    f32::from(cell_width),
                                    f32::from(line_height),
                                    snapshot_size,
                                );
                                app.resize_terminal(session_id, size, cx);
                            })
                            .ok();
                        });
                    })
                    .child(self.terminal_renderer.render(
                        snapshot,
                        TerminalRenderOptions::new(
                            self.terminal_theme,
                            self.colors.accent,
                            self.terminal_minimum_contrast,
                            selection,
                            ime,
                            input,
                            self.input_latency.clone(),
                        )
                        .with_search(search)
                        .with_url_hover(url_hover)
                        .with_cursor_visible(cursor_visible)
                        .with_metric_adjustments(metric_adjustments)
                        .with_font_families(self.terminal_font_families.clone())
                        .with_font_features(
                            self.terminal_font_features.clone(),
                            self.terminal_ligatures,
                            self.terminal_shaping_break_cursor,
                        )
                        .with_font_variations(
                            self.terminal_font_variations.clone(),
                            self.terminal_font_thicken,
                        )
                        .with_codepoint_map(self.terminal_font_codepoint_map.clone()),
                    ))
                    .into_any_element()
            })
            .unwrap_or_else(|| {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(self.colors.muted))
                    .child(
                        self.poll_error
                            .clone()
                            .unwrap_or_else(|| "Connecting to terminal…".to_owned()),
                    )
                    .into_any_element()
            });

        let corners = terminal_container_corners(
            touches_left_edge,
            touches_right_edge,
            touches_bottom_edge,
            self.left_sidebar_collapsed,
            self.right_sidebar_collapsed,
        );
        let borders = terminal_container_borders(
            touches_left_edge,
            touches_right_edge,
            touches_bottom_edge,
            self.left_sidebar_collapsed,
            self.right_sidebar_collapsed,
        );
        // The tab bar sits above the content region, so a vertical split seam drawn only on the
        // content region leaves a gap at tab-bar height. Extend the same left/right seam border up
        // through the tab bar. Skip the side that is rounded (an outer layout edge), where the
        // content region's rounded corner owns the look and a straight tab-bar border would spoil it.
        tabs = tabs
            .border_color(rgb(self.colors.border))
            .when(borders.left && !corners.top_left, |element| {
                element.border_l_1()
            })
            .when(borders.right && !corners.top_right, |element| {
                element.border_r_1()
            });
        let mut content_region = div()
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .shadow_xs()
            .border_t_1()
            .border_color(rgb(self.colors.border))
            .when(borders.bottom, |element| element.border_b_1())
            .when(corners.top_left, |element| element.rounded_tl_md())
            .when(corners.top_right, |element| element.rounded_tr_md())
            .when(corners.bottom_left, |element| element.rounded_bl_md())
            .when(corners.bottom_right, |element| element.rounded_br_md())
            .when(borders.left, |element| element.border_l_1())
            .when(borders.right, |element| element.border_r_1())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event, window, cx| {
                    app.terminal_mouse_down(group_id, event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |app, event, window, cx| {
                    app.terminal_mouse_down(group_id, event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |app, event, window, cx| {
                    app.terminal_mouse_down(group_id, event, window, cx)
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |app, event, _, cx| app.terminal_mouse_up(group_id, event, cx)),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |app, event, _, cx| app.terminal_mouse_up(group_id, event, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |app, event, _, cx| app.terminal_mouse_up(group_id, event, cx)),
            )
            .on_mouse_move(
                cx.listener(move |app, event, _, cx| app.terminal_mouse_move(group_id, event, cx)),
            )
            .on_scroll_wheel(
                cx.listener(move |app, event, _, cx| app.terminal_scroll(group_id, event, cx)),
            )
            .when(link_hovered, |element| {
                element.cursor(gpui::CursorStyle::PointingHand)
            })
            .when(terminal_captures_mouse && !link_hovered, |element| {
                element.cursor_default()
            })
            .when(!terminal_captures_mouse && !link_hovered, |element| {
                element.cursor_text()
            })
            .child(content);
        if let Some(zone) = content_drop_zone {
            let overlay = div()
                .absolute()
                .border_2()
                .border_color(rgb(self.colors.accent))
                .bg(rgba((self.colors.accent << 8) | 0x33))
                .map(|overlay| match zone {
                    TabDropZone::Center => overlay.inset_0(),
                    TabDropZone::Edge(Direction::Up) => {
                        overlay.top_0().left_0().right_0().h(relative(0.5))
                    }
                    TabDropZone::Edge(Direction::Down) => {
                        overlay.bottom_0().left_0().right_0().h(relative(0.5))
                    }
                    TabDropZone::Edge(Direction::Left) => {
                        overlay.top_0().bottom_0().left_0().w(relative(0.5))
                    }
                    TabDropZone::Edge(Direction::Right) => {
                        overlay.top_0().bottom_0().right_0().w(relative(0.5))
                    }
                    TabDropZone::TabBar { .. } => unreachable!(),
                })
                .on_drop(cx.listener(|app, dragged_tab: &DraggedTab, _, cx| {
                    app.handle_tab_drop(dragged_tab, cx)
                }));
            content_region = content_region.child(overlay);
        }

        // Overlay the search bar when it is bound to this group's active session.
        let active_session_id = group.active_item().and_then(|item| item.session_id);
        if let Some(search_bar) = active_session_id
            .filter(|session_id| {
                self.terminal_search
                    .as_ref()
                    .is_some_and(|search| search.session_id == *session_id)
            })
            .and_then(|_| self.render_terminal_search_bar(cx))
        {
            content_region = content_region.child(search_bar);
        }

        // Overlay the scrollbar on the right edge when the active session has scrollback.
        if let Some(scrollbar) = active_session_id.and_then(|session_id| {
            let snapshot = self.snapshots.get(&session_id)?;
            self.render_scrollbar(
                group_id,
                session_id,
                snapshot.history_size,
                snapshot.display_offset,
                snapshot.size.rows,
                cx,
            )
        }) {
            content_region = content_region.child(scrollbar);
        }

        div()
            .id(format!("terminal-group-{group_id}"))
            .flex()
            .flex_col()
            .relative()
            .size_full()
            .min_w_0()
            .bg(rgb(self.colors.background))
            .on_drag_move::<DraggedTab>(cx.listener(move |app, event, window, cx| {
                cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                app.update_content_drop_target(group_id, event, cx)
            }))
            .child(tabs)
            .child(content_region)
            .into_any_element()
    }

    fn render_right_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let language = self.language;
        let tabs = [
            (RightTab::Info, language.info_tab(), IconName::Info),
            (RightTab::Files, language.files_tab(), IconName::Folder),
            (RightTab::Git, language.git_tab(), IconName::GitBranch),
        ]
        .into_iter()
        .fold(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(TAB_BAR_HEIGHT))
                .p(px(TAB_BAR_PADDING))
                .gap(px(TAB_GAP)),
            |tabs, (tab, title, tab_icon)| {
                tabs.child(
                    div()
                        .id(format!("right-tab-{title}"))
                        .flex_1()
                        .flex()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .rounded_lg()
                        .cursor_pointer()
                        .when(self.right_tab == tab, |element| {
                            element
                                .bg(rgb(self.colors.panel_alt))
                                .text_color(rgb(self.colors.accent))
                        })
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.right_tab = tab;
                            cx.notify();
                        }))
                        .child(icon(tab_icon))
                        .child(title),
                )
            },
        );
        div()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.right_sidebar_width))
            .h_full()
            .bg(rgb(self.colors.panel))
            .child(tabs)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("right-sidebar-scroll-content")
                            .size_full()
                            .min_h_0()
                            .overflow_y_scroll()
                            .scrollbar_width(px(SIDEBAR_SCROLLBAR_WIDTH))
                            .track_scroll(&self.right_sidebar_scroll_handle)
                            .p_4()
                            .text_size(px(12.))
                            .child(self.render_right_content(cx)),
                    )
                    .child(self.render_right_sidebar_scrollbar()),
            )
            .into_any_element()
    }

    fn render_right_sidebar_scrollbar(&self) -> AnyElement {
        let scroll_handle = self.right_sidebar_scroll_handle.clone();
        let muted = self.colors.muted;
        canvas(
            |_, _, _| (),
            move |track_bounds, _, window, _| {
                let max_offset = f32::from(scroll_handle.max_offset().y);
                if max_offset <= 0. {
                    return;
                }

                let viewport_height = f32::from(scroll_handle.bounds().size.height);
                let track_height = f32::from(track_bounds.size.height);
                let content_height = viewport_height + max_offset;
                let thumb_height = (track_height * viewport_height / content_height)
                    .max(SIDEBAR_SCROLLBAR_MIN_THUMB_HEIGHT)
                    .min(track_height);
                let progress = (-f32::from(scroll_handle.offset().y) / max_offset).clamp(0., 1.);
                let thumb_top =
                    f32::from(track_bounds.origin.y) + (track_height - thumb_height) * progress;
                let thumb_bounds = Bounds::new(
                    point(track_bounds.origin.x, px(thumb_top)),
                    size(track_bounds.size.width, px(thumb_height)),
                );
                window.paint_quad(quad(
                    thumb_bounds,
                    px(SIDEBAR_SCROLLBAR_THUMB_WIDTH / 2.),
                    rgba((muted << 8) | 0x99),
                    px(0.),
                    rgba(0x00000000),
                    Default::default(),
                ));
            },
        )
        .absolute()
        .top(px(4.))
        .right(px(2.))
        .bottom(px(4.))
        .w(px(SIDEBAR_SCROLLBAR_THUMB_WIDTH))
        .into_any_element()
    }

    fn render_right_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.right_tab {
            RightTab::Info => {
                let session = self
                    .active_session_id()
                    .and_then(|id| self.sessions.iter().find(|session| session.id == id));
                let mut content = div().flex().flex_col().gap_4();
                if let Some(session) = session {
                    content = content
                        .child(process_summary(&session.current_process, self.colors, self.language))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(info_row(
                                    self.language.current_directory(),
                                    session.current_directory.display().to_string(),
                                    self.colors,
                                ))
                                .child(self.render_directory_actions(
                                    DirectoryField::Current,
                                    &session.current_directory,
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(info_row(
                                    self.language.initial_directory(),
                                    session.initial_directory.display().to_string(),
                                    self.colors,
                                ))
                                .child(self.render_directory_actions(
                                    DirectoryField::Initial,
                                    &session.initial_directory,
                                    cx,
                                )),
                        );
                    if let Some(inspection) = self.session_inspections.get(&session.id) {
                        content =
                            content.child(process_section(&inspection.processes, self.colors, self.language, cx));
                        if let Some(ports) = ports_section(inspection, self.colors, self.language) {
                            content = content.child(ports);
                        }
                    } else {
                        let status = self
                            .session_inspection_errors
                            .get(&session.id)
                            .cloned()
                            .unwrap_or_else(|| self.language.loading_process_info().to_owned());
                        content = content
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(process_section_header(self.colors, self.language))
                                    .child(placeholder(status, self.colors)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(section_label(self.language.ports_label(), self.colors))
                                    .child(placeholder(
                                        self.language.loading_ports(),
                                        self.colors,
                                    )),
                            );
                    }
                }
                content.into_any_element()
            }
            RightTab::Files => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(section_label(self.language.current_directory_label(), self.colors))
                .child(placeholder(
                    self.language.file_tree_scaffold_note(),
                    self.colors,
                ))
                .into_any_element(),
            RightTab::Git => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(section_label(self.language.source_control_label(), self.colors))
                .child(placeholder(
                    self.language.git_scaffold_note(),
                    self.colors,
                ))
                .into_any_element(),
        }
    }
}

impl gpui::Render for EggieApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = self.settings.read(cx).config().clone();
        // Read the *resolved* theme (persisted theme, or the settings window's live hover preview if
        // one is active) so a hovered theme lights up the real terminals here, not just the preview.
        self.terminal_theme = self
            .settings
            .read(cx)
            .resolved_theme(is_dark_appearance(window.appearance()));
        let appearance = self.terminal_theme.appearance();
        if appearance != self.terminal_appearance {
            self.terminal_appearance = appearance;
            for session_id in self.window_session_ids() {
                if let Err(error) = self.client.request(ClientRequest::SetAppearance {
                    session_id,
                    appearance,
                }) {
                    eprintln!("failed to update terminal appearance: {error:#}");
                }
            }
        }
        self.colors = UiColors::from_theme(self.terminal_theme);
        self.terminal_font_families = config.resolved_font_families();
        self.terminal_font_features = config.resolved_font_features().into();
        self.terminal_ligatures = config.ligatures;
        self.terminal_shaping_break_cursor = config.font_shaping_break_cursor;
        self.terminal_font_variations = config.resolved_font_variations().into();
        self.terminal_font_thicken =
            config.font_thicken.then_some(config.font_thicken_strength);
        self.terminal_font_codepoint_map = config.resolved_codepoint_map().into();
        self.terminal_font_family = config.font_family.into();
        self.terminal_font_size = config.font_size;
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
        // Keep the search input's edit-menu labels in sync with the current language. Cheap: clone
        // the handle first to avoid overlapping the `self.terminal_search` borrow with `cx`.
        if let Some(input) = self.terminal_search.as_ref().map(|s| s.input.clone()) {
            input.update(cx, |input, _| input.set_language(config.language));
        }
        // Keep the blink timer running while the active cursor blinks; it self-terminates otherwise.
        self.ensure_cursor_blink(cx);
        div()
            .id("eggie-workspace")
            .relative()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
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
            .on_action(cx.listener(|app, _: &NewTab, _, cx| {
                if app.new_tab_in_active_group(cx) {
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
            .on_action(cx.listener(|app, _: &SplitRight, _, cx| {
                if app.split_active_group(Direction::Right, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &SplitDown, _, cx| {
                if app.split_active_group(Direction::Down, cx) {
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
            .on_action(cx.listener(|app, _: &SplitLeft, _, cx| {
                if app.split_active_group(Direction::Left, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|app, _: &SplitUp, _, cx| {
                if app.split_active_group(Direction::Up, cx) {
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
                    .child(self.render_layout(&self.workspace().layout, cx)),
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

fn visible_active_terminal_sessions(layout: &LayoutNode) -> Vec<SessionId> {
    fn collect(layout: &LayoutNode, sessions: &mut Vec<SessionId>) {
        match layout {
            LayoutNode::Group { group } => {
                if let Some(session_id) = group.active_item().and_then(|item| item.session_id) {
                    sessions.push(session_id);
                }
            }
            LayoutNode::Split { first, second, .. } => {
                collect(first, sessions);
                collect(second, sessions);
            }
        }
    }

    let mut sessions = Vec::new();
    collect(layout, &mut sessions);
    sessions
}

fn layout_has_visible_active_terminal(layout: &LayoutNode, session_id: SessionId) -> bool {
    match layout {
        LayoutNode::Group { group } => group
            .active_item()
            .is_some_and(|item| item.session_id == Some(session_id)),
        LayoutNode::Split { first, second, .. } => {
            layout_has_visible_active_terminal(first, session_id)
                || layout_has_visible_active_terminal(second, session_id)
        }
    }
}

fn terminal_container_corners(
    touches_left_layout_edge: bool,
    touches_right_layout_edge: bool,
    touches_bottom_layout_edge: bool,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
) -> TerminalContainerCorners {
    // The tab bar always separates terminal content from the top window edge. A side corner is
    // rounded only on an outer layout edge that is separated from the window by an open sidebar;
    // corners meeting another group at a horizontal split stay square.
    let left_edge_allows_rounding = touches_left_layout_edge && !left_sidebar_collapsed;
    let right_edge_allows_rounding = touches_right_layout_edge && !right_sidebar_collapsed;
    TerminalContainerCorners {
        top_left: left_edge_allows_rounding,
        top_right: right_edge_allows_rounding,
        bottom_left: left_edge_allows_rounding && !touches_bottom_layout_edge,
        bottom_right: right_edge_allows_rounding && !touches_bottom_layout_edge,
    }
}

fn terminal_container_borders(
    touches_left_layout_edge: bool,
    touches_right_layout_edge: bool,
    touches_bottom_layout_edge: bool,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
) -> TerminalContainerBorders {
    TerminalContainerBorders {
        // The left-hand group owns each internal vertical seam, so the border is segmented by
        // terminal content regions and never crosses a nested group's tab bar.
        left: touches_left_layout_edge && !left_sidebar_collapsed,
        right: !touches_right_layout_edge || !right_sidebar_collapsed,
        // The upper group owns each internal horizontal seam.
        bottom: !touches_bottom_layout_edge,
    }
}

fn progress_label(progress: TerminalProgress, language: Language) -> String {
    let status = match (progress.state, progress.percent) {
        (TerminalProgressState::Normal, Some(100)) => language.progress_complete(),
        (TerminalProgressState::Normal, _) => language.progress_running(),
        (TerminalProgressState::Error, _) => language.progress_error(),
        (TerminalProgressState::Indeterminate, _) => language.progress_indeterminate(),
        (TerminalProgressState::Paused, _) => language.progress_paused(),
    };
    let percent = progress
        .percent
        .map(|percent| format!(" · {percent}%"))
        .unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let elapsed_seconds = now.saturating_sub(progress.updated_at_unix_ms) / 1_000;
    let recency = match elapsed_seconds {
        0..=1 => language.updated_just_now().to_owned(),
        2..=59 => language.updated_seconds_ago(elapsed_seconds),
        60..=3_599 => language.updated_minutes_ago(elapsed_seconds / 60),
        _ => language.updated_hours_ago(elapsed_seconds / 3_600),
    };
    format!("{status}{percent} · {recency}")
}

fn paint_progress_ring(
    bounds: Bounds<Pixels>,
    progress: TerminalProgress,
    phase: f32,
    color: u32,
    track_color: u32,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let radius = (width.min(height) / 2. - 1.5).max(1.);
    let center_x = f32::from(bounds.origin.x) + width / 2.;
    let center_y = f32::from(bounds.origin.y) + height / 2.;
    let top = point(px(center_x), px(center_y - radius));
    let bottom = point(px(center_x), px(center_y + radius));
    let radii = point(px(radius), px(radius));

    let mut track = PathBuilder::stroke(px(1.5));
    track.move_to(top);
    track.arc_to(radii, px(0.), false, true, bottom);
    track.arc_to(radii, px(0.), false, true, top);
    if let Ok(path) = track.build() {
        window.paint_path(path, rgb(track_color));
    }

    let (start_turn, fraction) = match progress.state {
        TerminalProgressState::Indeterminate => (phase, 0.28),
        _ => (0., f32::from(progress.percent.unwrap_or(100)) / 100.),
    };
    if fraction <= 0. {
        return;
    }

    let start_angle = -std::f32::consts::FRAC_PI_2 + start_turn * std::f32::consts::TAU;
    let start = point(
        px(center_x + radius * start_angle.cos()),
        px(center_y + radius * start_angle.sin()),
    );
    let mut arc = PathBuilder::stroke(px(1.5));
    arc.move_to(start);
    if fraction >= 0.999 {
        let opposite_angle = start_angle + std::f32::consts::PI;
        let opposite = point(
            px(center_x + radius * opposite_angle.cos()),
            px(center_y + radius * opposite_angle.sin()),
        );
        arc.arc_to(radii, px(0.), false, true, opposite);
        arc.arc_to(radii, px(0.), false, true, start);
    } else {
        let end_angle = start_angle + fraction * std::f32::consts::TAU;
        arc.arc_to(
            radii,
            px(0.),
            fraction > 0.5,
            true,
            point(
                px(center_x + radius * end_angle.cos()),
                px(center_y + radius * end_angle.sin()),
            ),
        );
    }
    if let Ok(path) = arc.build() {
        window.paint_path(path, rgb(color));
    }
}

fn should_show_content_drop_target(
    source_group_id: GroupId,
    target_group_id: GroupId,
    source_item_count: usize,
) -> bool {
    source_group_id != target_group_id || source_item_count >= 2
}

fn content_drop_zone(width: f32, height: f32, x: f32, y: f32) -> TabDropZone {
    let inside_center = x > width * CONTENT_SPLIT_EDGE_FACTOR
        && x < width * (1. - CONTENT_SPLIT_EDGE_FACTOR)
        && y > height * CONTENT_SPLIT_EDGE_FACTOR
        && y < height * (1. - CONTENT_SPLIT_EDGE_FACTOR);
    if inside_center {
        TabDropZone::Center
    } else if x < width / 3. {
        TabDropZone::Edge(Direction::Left)
    } else if x > width * 2. / 3. {
        TabDropZone::Edge(Direction::Right)
    } else if y < height / 2. {
        TabDropZone::Edge(Direction::Up)
    } else {
        TabDropZone::Edge(Direction::Down)
    }
}

fn tab_bar_insertion_index(tab_index: usize, tab_width: f32, relative_x: f32) -> usize {
    if relative_x <= tab_width / 2. {
        tab_index
    } else {
        tab_index + 1
    }
}

#[cfg(test)]
fn terminal_bytes(event: &KeyDownEvent) -> Option<Vec<u8>> {
    terminal_bytes_with_mode(event, 0)
}

#[cfg(test)]
const KITTY_DISAMBIGUATE_ESCAPE_CODES: u8 = 1;
const KITTY_REPORT_EVENT_TYPES: u8 = 2;
const KITTY_REPORT_ALL_KEYS: u8 = 8;
const KITTY_REPORT_ASSOCIATED_TEXT: u8 = 16;

fn terminal_bytes_with_mode(event: &KeyDownEvent, keyboard_flags: u8) -> Option<Vec<u8>> {
    if keyboard_flags != 0 {
        return kitty_key_bytes(
            &event.keystroke,
            keyboard_flags,
            if event.is_held { 2 } else { 1 },
        );
    }

    let key = event.keystroke.key.as_str();
    if event.keystroke.modifiers.platform {
        return None;
    }
    let mut bytes = match key {
        "enter" => vec![b'\r'],
        "backspace" => vec![0x7f],
        "tab" => vec![b'\t'],
        "escape" => vec![0x1b],
        "up" => b"\x1b[A".to_vec(),
        "down" => b"\x1b[B".to_vec(),
        "right" => b"\x1b[C".to_vec(),
        "left" => b"\x1b[D".to_vec(),
        "home" => b"\x1b[H".to_vec(),
        "end" => b"\x1b[F".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        _ if event.keystroke.modifiers.control && key.is_ascii() && key.len() == 1 => {
            vec![key.as_bytes()[0].to_ascii_uppercase() & 0x1f]
        }
        _ if event.keystroke.modifiers.function => return None,
        _ => terminal_key_char_bytes(event.keystroke.key_char.as_deref()?)?,
    };
    if event.keystroke.modifiers.alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

fn kitty_key_bytes(keystroke: &Keystroke, flags: u8, event_type: u8) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let modifiers = 1
        + u8::from(keystroke.modifiers.shift)
        + u8::from(keystroke.modifiers.alt) * 2
        + u8::from(keystroke.modifiers.control) * 4
        + u8::from(keystroke.modifiers.platform) * 8;
    let modifier_field = if flags & KITTY_REPORT_EVENT_TYPES != 0 {
        format!("{modifiers}:{event_type}")
    } else {
        modifiers.to_string()
    };

    let csi_final = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(final_byte) = csi_final {
        return Some(format!("\x1b[1;{modifier_field}{final_byte}").into_bytes());
    }

    let tilde_code = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(code) = tilde_code {
        return Some(format!("\x1b[{code};{modifier_field}~").into_bytes());
    }

    let f_final = match key {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(final_byte) = f_final {
        return Some(format!("\x1b[1;{modifier_field}{final_byte}").into_bytes());
    }

    // AppKit uses the Fn modifier to transform known navigation/function keys.
    // It is not a Kitty protocol modifier of its own, so only discard an
    // otherwise unknown Fn chord after giving transformed keys a chance above.
    if keystroke.modifiers.function {
        return None;
    }

    let primary = match key {
        "enter" => 13,
        "tab" => 9,
        "backspace" => 127,
        "escape" => 27,
        _ => key
            .chars()
            .next()
            .filter(|_| key.chars().count() == 1)
            .map(u32::from)
            .or_else(|| {
                keystroke
                    .key_char
                    .as_deref()
                    .and_then(|text| text.chars().next())
                    .map(u32::from)
            })?,
    };

    let printable_text = keystroke
        .key_char
        .as_deref()
        .filter(|text| !text.is_empty() && !text.chars().any(is_reserved_key_character));
    let must_escape = flags & KITTY_REPORT_ALL_KEYS != 0
        || modifiers != 1
        || matches!(key, "enter" | "tab" | "backspace" | "escape");
    if !must_escape {
        return (event_type != 3)
            .then(|| printable_text.map(|text| text.as_bytes().to_vec()))
            .flatten();
    }

    let associated = (flags & KITTY_REPORT_ASSOCIATED_TEXT != 0 && event_type != 3)
        .then_some(printable_text)
        .flatten()
        // GPUI exposes the physical character for Ctrl chords even though the
        // generated terminal text is a C0 control. Kitty forbids control codes
        // in the associated-text field, so do not report that physical value.
        .filter(|_| !keystroke.modifiers.control)
        .map(|text| {
            text.chars()
                .map(|character| u32::from(character).to_string())
                .collect::<Vec<_>>()
                .join(":")
        });
    let mut sequence = format!("\x1b[{primary};{modifier_field}");
    if let Some(associated) = associated {
        sequence.push(';');
        sequence.push_str(&associated);
    }
    sequence.push('u');
    Some(sequence.into_bytes())
}

fn is_character_palette_shortcut(event: &KeyDownEvent) -> bool {
    let modifiers = event.keystroke.modifiers;
    modifiers.platform && modifiers.control && event.keystroke.key == "space"
}

fn terminal_text_bytes(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

fn terminal_key_char_bytes(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.chars().any(is_reserved_key_character) {
        return None;
    }
    Some(terminal_text_bytes(text))
}

fn is_reserved_key_character(character: char) -> bool {
    let codepoint = character as u32;
    character.is_control()
        || (0xf700..=0xf8ff).contains(&codepoint)
        || (0xfdd0..=0xfdef).contains(&codepoint)
        || codepoint & 0xffff == 0xfffe
        || codepoint & 0xffff == 0xffff
}

fn terminal_point_from_position(
    viewport: TerminalViewport,
    position: gpui::Point<Pixels>,
) -> Option<TerminalPoint> {
    if viewport.rows == 0
        || viewport.columns == 0
        || viewport.cell_width <= 0.
        || viewport.line_height <= 0.
    {
        return None;
    }
    let x = f32::from(position.x - viewport.bounds.origin.x);
    let y = f32::from(position.y - viewport.bounds.origin.y);
    Some(TerminalPoint {
        line: (y / viewport.line_height)
            .floor()
            .clamp(0., (viewport.rows - 1) as f32) as u16,
        column: (x / viewport.cell_width)
            .floor()
            .clamp(0., (viewport.columns - 1) as f32) as u16,
    })
}

/// Which half of its cell a pixel position falls in. The daemon uses this as the selection
/// endpoint's `Side`, so a selection can include or exclude the cell under the cursor depending on
/// whether the pointer is past the cell's midpoint.
fn terminal_selection_side(
    viewport: TerminalViewport,
    position: gpui::Point<Pixels>,
) -> TerminalSelectionSide {
    if viewport.cell_width <= 0. {
        return TerminalSelectionSide::Left;
    }
    let x = f32::from(position.x - viewport.bounds.origin.x);
    let fraction = (x / viewport.cell_width).fract();
    if fraction >= 0.5 {
        TerminalSelectionSide::Right
    } else {
        TerminalSelectionSide::Left
    }
}

/// Convert a viewport point (renderer coordinate) into the protocol cell position sent to the
/// daemon. Both are 0-based viewport line/column; only the type differs across the crate boundary.
fn cell_position(point: TerminalPoint) -> eggie_protocol::TerminalCellPosition {
    eggie_protocol::TerminalCellPosition {
        line: point.line,
        column: point.column,
    }
}

fn terminal_hyperlink_at(snapshot: &TerminalSnapshot, point: TerminalPoint) -> Option<String> {
    snapshot
        .cells
        .iter()
        .find(|cell| cell.line == point.line && cell.column == point.column)
        .and_then(|cell| cell.hyperlink.clone())
}

/// The auto-detected URL range covering `point`, if any. Ranges are per-row, so a hit is a simple
/// line match plus column containment.
fn detected_link_at(
    snapshot: &TerminalSnapshot,
    point: TerminalPoint,
) -> Option<&eggie_protocol::TerminalLinkRange> {
    snapshot.detected_links.iter().find(|link| {
        link.start.line == point.line
            && point.column >= link.start.column
            && point.column <= link.end.column
    })
}

fn url_has_safe_external_scheme(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

fn protocol_mouse_position(
    point: TerminalPoint,
    viewport: TerminalViewport,
    position: gpui::Point<Pixels>,
) -> TerminalMousePosition {
    let x = f32::from(position.x - viewport.bounds.origin.x);
    let y = f32::from(position.y - viewport.bounds.origin.y);
    let pixel_width = viewport.cell_width * viewport.columns as f32;
    let pixel_height = viewport.line_height * viewport.rows as f32;
    TerminalMousePosition {
        column: point.column,
        row: point.line,
        pixel_x: x.floor().clamp(0., (pixel_width - 1.).max(0.)) as u32,
        pixel_y: y.floor().clamp(0., (pixel_height - 1.).max(0.)) as u32,
    }
}

fn protocol_mouse_button(button: MouseButton) -> Option<TerminalMouseButton> {
    match button {
        MouseButton::Left => Some(TerminalMouseButton::Left),
        MouseButton::Middle => Some(TerminalMouseButton::Middle),
        MouseButton::Right => Some(TerminalMouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

fn protocol_terminal_modifiers(modifiers: gpui::Modifiers) -> TerminalModifiers {
    TerminalModifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
}

fn notification_tag(session_id: SessionId, notification_id: &str) -> String {
    format!("eggie:{session_id}:{notification_id}")
}

fn clipboard_item_from_contents(contents: Vec<TerminalClipboardContent>) -> Option<ClipboardItem> {
    let mut entries = Vec::new();
    let mut fallback_text = None;
    for content in contents {
        if content.mime_type.starts_with("text/plain") {
            if let Ok(text) = String::from_utf8(content.data) {
                entries.push(ClipboardEntry::String(ClipboardString::new(text)));
            }
        } else if content.mime_type.starts_with("text/") {
            if fallback_text.is_none() {
                fallback_text = String::from_utf8(content.data).ok();
            }
        } else if let Some(format) = ImageFormat::from_mime_type(&content.mime_type) {
            entries.push(ClipboardEntry::Image(Image::from_bytes(
                format,
                content.data,
            )));
        }
    }
    if !entries
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::String(_)))
        && let Some(text) = fallback_text
    {
        entries.insert(0, ClipboardEntry::String(ClipboardString::new(text)));
    }
    (!entries.is_empty()).then_some(ClipboardItem { entries })
}

fn clipboard_contents_from_item(
    item: ClipboardItem,
    requested_mime_types: &[String],
) -> Vec<TerminalClipboardContent> {
    let accepts = |mime_type: &str| {
        let base = mime_type.split(';').next().unwrap_or(mime_type);
        requested_mime_types.is_empty()
            || requested_mime_types.iter().any(|requested| {
                let requested_base = requested.split(';').next().unwrap_or(requested);
                requested == "."
                    || requested == mime_type
                    || requested_base == base
                    || requested == "*/*"
                    || requested
                        .strip_suffix("/*")
                        .is_some_and(|prefix| mime_type.starts_with(prefix))
            })
    };
    let mut contents = Vec::new();
    for entry in item.into_entries() {
        match entry {
            ClipboardEntry::String(value) if accepts("text/plain") => {
                contents.push(TerminalClipboardContent {
                    mime_type: "text/plain".to_owned(),
                    data: value.into_text().into_bytes(),
                });
            }
            ClipboardEntry::Image(image) if accepts(image.format().mime_type()) => {
                contents.push(TerminalClipboardContent {
                    mime_type: image.format().mime_type().to_owned(),
                    data: image.bytes().to_vec(),
                });
            }
            ClipboardEntry::ExternalPaths(paths) if accepts("text/uri-list") => {
                let text = paths
                    .0
                    .into_iter()
                    .map(|path| format!("file://{}", path.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                contents.push(TerminalClipboardContent {
                    mime_type: "text/uri-list".to_owned(),
                    data: text.into_bytes(),
                });
            }
            _ => {}
        }
    }
    contents
}

fn fixed_terminal_scroll_delta(delta: f32) -> i32 {
    if !delta.is_finite() {
        return 0;
    }
    (f64::from(delta) * f64::from(TERMINAL_SCROLL_DELTA_SCALE))
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// Thumb geometry for the terminal scrollbar, in pixels relative to the track's top.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScrollbarThumb {
    top: f32,
    height: f32,
}

/// Compute the scrollbar thumb's height and top offset.
///
/// `H` = history_size (scrollback lines), `R` = visible rows, `D` = display_offset (0 = live bottom,
/// H = scrolled to the oldest line). The thumb represents the R visible rows out of the (H + R) total
/// scrollable rows. Viewport-top content index is `H - D`, mapped onto pixel travel `[0, travel]`:
/// D = 0 → thumb flush bottom, D = H → thumb flush top. Returns `None` when there is no scrollback
/// (`H == 0`) or the track is too short to be meaningful.
fn scrollbar_thumb_geometry(track_height: f32, history: u32, rows: u16, offset: u32) -> Option<ScrollbarThumb> {
    if history == 0 || track_height <= 0. {
        return None;
    }
    let h = history as f32;
    let r = (rows.max(1)) as f32;
    let frac = r / (h + r);
    let thumb_height = (track_height * frac).max(TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT).min(track_height);
    let travel = (track_height - thumb_height).max(0.);
    let d = (offset.min(history)) as f32;
    let top = ((h - d) / h) * travel;
    Some(ScrollbarThumb {
        top,
        height: thumb_height,
    })
}

/// Inverse of [`scrollbar_thumb_geometry`]: map a thumb-top pixel position back to a scroll offset D.
/// `thumb_top` is clamped to `[0, travel]` by the caller's grab math; here we invert the linear map.
/// Returns the current offset unchanged when there is no travel (thumb fills the track).
fn scrollbar_offset_from_thumb_top(track_height: f32, history: u32, rows: u16, thumb_top: f32) -> u32 {
    if history == 0 || track_height <= 0. {
        return 0;
    }
    let h = history as f32;
    let r = (rows.max(1)) as f32;
    let frac = r / (h + r);
    let thumb_height = (track_height * frac).max(TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT).min(track_height);
    let travel = track_height - thumb_height;
    if travel <= 0. {
        return 0;
    }
    let clamped = thumb_top.clamp(0., travel);
    // top = ((H - D) / H) * travel  ⇒  D = H * (1 - top/travel)
    let d = (h * (1. - clamped / travel)).round();
    (d.max(0.) as u32).min(history)
}

fn terminal_size_for_viewport(
    width: f32,
    height: f32,
    cell_width: f32,
    line_height: f32,
    scale_factor: f32,
) -> TerminalSize {
    fn cells_that_fit(extent: f32, cell_extent: f32, minimum: u16) -> u16 {
        if !extent.is_finite() || !cell_extent.is_finite() || cell_extent <= 0. {
            return minimum;
        }
        ((extent / cell_extent).next_up().floor() as u32).clamp(minimum as u32, u16::MAX as u32)
            as u16
    }

    TerminalSize {
        columns: cells_that_fit(width, cell_width, 2),
        rows: cells_that_fit(height, line_height, 1),
        cell_width: (cell_width * scale_factor.max(1.))
            .round()
            .clamp(1., u16::MAX as f32) as u16,
        cell_height: (line_height * scale_factor.max(1.))
            .round()
            .clamp(1., u16::MAX as f32) as u16,
    }
}

fn section_label(label: &'static str, colors: UiColors) -> impl IntoElement {
    div().text_xs().text_color(rgb(colors.muted)).child(label)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessTreeRow {
    process_index: usize,
    branch_has_next: Vec<bool>,
}

fn kill_process(pid: u32, signal: i32) {
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

#[cfg(target_os = "macos")]
fn executable_path_for_pid(pid: u32) -> Option<String> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe {
        libc::proc_pidpath(
            pid as libc::pid_t,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buffer.truncate(len as usize);
    String::from_utf8(buffer).ok()
}

#[cfg(not(target_os = "macos"))]
fn executable_path_for_pid(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()?.to_str().map(str::to_owned)
}

fn process_tree_rows(processes: &[ProcessInfo]) -> Vec<ProcessTreeRow> {
    fn sort_indices(indices: &mut [usize], processes: &[ProcessInfo]) {
        indices.sort_by(|left, right| {
            processes[*left]
                .pid
                .cmp(&processes[*right].pid)
                .then_with(|| processes[*left].name.cmp(&processes[*right].name))
        });
    }

    fn visit(
        process_index: usize,
        processes: &[ProcessInfo],
        children: &HashMap<u32, Vec<usize>>,
        branch_has_next: &mut Vec<bool>,
        visited: &mut HashSet<usize>,
        rows: &mut Vec<ProcessTreeRow>,
    ) {
        if !visited.insert(process_index) {
            return;
        }

        rows.push(ProcessTreeRow {
            process_index,
            branch_has_next: branch_has_next.clone(),
        });

        if let Some(child_indices) = children.get(&processes[process_index].pid) {
            for (position, child_index) in child_indices.iter().copied().enumerate() {
                branch_has_next.push(position + 1 < child_indices.len());
                visit(
                    child_index,
                    processes,
                    children,
                    branch_has_next,
                    visited,
                    rows,
                );
                branch_has_next.pop();
            }
        }
    }

    let pids = processes
        .iter()
        .map(|process| process.pid)
        .collect::<HashSet<_>>();
    let mut roots = Vec::new();
    let mut children = HashMap::<u32, Vec<usize>>::new();
    for (index, process) in processes.iter().enumerate() {
        match process.parent_pid {
            Some(parent_pid) if parent_pid != process.pid && pids.contains(&parent_pid) => {
                children.entry(parent_pid).or_default().push(index);
            }
            _ => roots.push(index),
        }
    }
    sort_indices(&mut roots, processes);
    for child_indices in children.values_mut() {
        sort_indices(child_indices, processes);
    }

    let mut rows = Vec::with_capacity(processes.len());
    let mut visited = HashSet::with_capacity(processes.len());
    for root in roots {
        visit(
            root,
            processes,
            &children,
            &mut Vec::new(),
            &mut visited,
            &mut rows,
        );
    }

    let mut malformed_roots = (0..processes.len())
        .filter(|index| !visited.contains(index))
        .collect::<Vec<_>>();
    sort_indices(&mut malformed_roots, processes);
    for root in malformed_roots {
        visit(
            root,
            processes,
            &children,
            &mut Vec::new(),
            &mut visited,
            &mut rows,
        );
    }
    rows
}

fn process_section(
    processes: &[ProcessInfo],
    colors: UiColors,
    language: Language,
    cx: &mut Context<EggieApp>,
) -> AnyElement {
    let body = if processes.is_empty() {
        placeholder(language.no_running_processes(), colors).into_any_element()
    } else {
        let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        let root_pids: HashSet<u32> = processes
            .iter()
            .filter(|p| match p.parent_pid {
                Some(parent) => parent == p.pid || !pids.contains(&parent),
                None => true,
            })
            .map(|p| p.pid)
            .collect();

        let mut rows = div().flex().flex_col();
        for row in process_tree_rows(processes) {
            let process = &processes[row.process_index];
            let is_root = root_pids.contains(&process.pid);
            rows = rows.child(process_info_row(
                process,
                row.branch_has_next,
                colors,
                is_root,
                cx,
            ));
        }
        rows.into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(process_section_header(colors, language))
        .child(body)
        .into_any_element()
}

fn process_section_header(colors: UiColors, language: Language) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .text_size(px(10.))
        .text_color(rgb(colors.muted))
        .child(language.processes_label())
}

fn ports_section(
    inspection: &SessionInspection,
    colors: UiColors,
    language: Language,
) -> Option<AnyElement> {
    if inspection.ports.is_empty() {
        return None;
    }

    let mut rows = div().flex().flex_col().gap_2();
    for port in &inspection.ports {
        let process_name = inspection
            .processes
            .iter()
            .find(|process| process.pid == port.pid)
            .map(|process| process.name.as_str());
        rows = rows.child(listening_port_row(port, process_name, colors));
    }
    Some(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_label(language.ports_label(), colors))
            .child(rows)
            .into_any_element(),
    )
}

fn info_row(label: &'static str, value: String, colors: UiColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap_1()
        .child(div().text_xs().text_color(rgb(colors.muted)).child(label))
        .child(div().min_w_0().truncate().child(value))
}

fn process_summary(
    process: &eggie_protocol::ProcessSummary,
    colors: UiColors,
    language: Language,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .child(process.name.clone()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(colors.muted))
                .child(format!("{} {}", language.pid_label(), process.pid)),
        )
}

fn process_info_row(
    process: &ProcessInfo,
    branch_has_next: Vec<bool>,
    colors: UiColors,
    is_root: bool,
    cx: &mut Context<EggieApp>,
) -> impl IntoElement {
    let pid = process.pid;
    let mut content = div()
        .id(format!("process-row-{pid}"))
        .flex()
        .flex_1()
        .items_center()
        .min_w_0()
        .h(px(PROCESS_ROW_HEIGHT))
        .rounded_md()
        .ml_neg_1()
        .mr_neg_2();

    if !is_root {
        content = content
            .cursor_pointer()
            .hover(|element| element.bg(rgb(colors.hover)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, _, window, cx| {
                app.show_process_context_menu(pid, window, cx);
            }));
    }

    content = content
        .child(
            div()
                .ml_1()
                .min_w_0()
                .truncate()
                .child(process.name.clone()),
        )
        .child(
            div()
                .ml_1()
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(rgb(colors.muted))
                .child(process.pid.to_string()),
        )
        .child(
            div()
                .ml_auto()
                .mr_2()
                .flex_shrink_0()
                .text_right()
                .text_size(px(10.))
                .text_color(rgb(colors.muted))
                .child(format_process_metrics(process)),
        );

    div()
        .flex()
        .items_center()
        .w_full()
        .h(px(PROCESS_ROW_HEIGHT))
        .text_size(px(12.))
        .child(process_tree_connector(branch_has_next, colors))
        .child(content)
}

fn process_tree_connector(branch_has_next: Vec<bool>, colors: UiColors) -> AnyElement {
    let width = branch_has_next.len() as f32 * PROCESS_TREE_LEVEL_WIDTH;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin_x = f32::from(bounds.origin.x);
            let origin_y = f32::from(bounds.origin.y);
            let height = f32::from(bounds.size.height);
            let middle_y = origin_y + height / 2.;

            let mut builder = PathBuilder::stroke(px(1.));
            for (depth, has_next) in branch_has_next.iter().copied().enumerate() {
                let line_x =
                    origin_x + depth as f32 * PROCESS_TREE_LEVEL_WIDTH + PROCESS_TREE_LINE_X;
                let is_current = depth + 1 == branch_has_next.len();

                if !is_current {
                    if has_next {
                        builder.move_to(point(px(line_x), px(origin_y)));
                        builder.line_to(point(px(line_x), px(origin_y + height)));
                    }
                    continue;
                }

                if has_next {
                    builder.move_to(point(px(line_x), px(origin_y)));
                    builder.line_to(point(px(line_x), px(origin_y + height)));
                    builder.move_to(point(px(line_x), px(middle_y)));
                    builder.line_to(point(px(line_x + PROCESS_TREE_ARM_LENGTH), px(middle_y)));
                } else {
                    builder.move_to(point(px(line_x), px(origin_y)));
                    builder.line_to(point(px(line_x), px(middle_y - PROCESS_TREE_CORNER_RADIUS)));
                    builder.curve_to(
                        point(px(line_x + PROCESS_TREE_CORNER_RADIUS), px(middle_y)),
                        point(px(line_x), px(middle_y)),
                    );
                    builder.line_to(point(px(line_x + PROCESS_TREE_ARM_LENGTH), px(middle_y)));
                }
            }

            if let Ok(path) = builder.build() {
                window.paint_path(path, rgb(colors.muted));
            }
        },
    )
    .w(px(width))
    .h(px(PROCESS_ROW_HEIGHT))
    .flex_shrink_0()
    .into_any_element()
}

fn cpu_usage_tenths_percent(usage: f32) -> u32 {
    if usage.is_finite() && usage > 0. {
        (usage * 10.).round().min(u32::MAX as f32) as u32
    } else {
        0
    }
}

fn format_cpu_usage(usage_tenths_percent: Option<u32>) -> String {
    usage_tenths_percent
        .map(|usage| format!("{}.{:01}%", usage / 10, usage % 10))
        .unwrap_or_else(|| "—".to_owned())
}

fn format_memory_usage(memory_bytes: Option<u64>) -> String {
    let Some(bytes) = memory_bytes else {
        return "—".to_owned();
    };
    const KIB: f64 = 1024.;
    const MIB: f64 = KIB * 1024.;
    const GIB: f64 = MIB * 1024.;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1}GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1}MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0}KB", bytes / KIB)
    } else {
        format!("{}B", bytes as u64)
    }
}

fn format_process_metrics(process: &ProcessInfo) -> String {
    format!(
        "{} / {}",
        format_cpu_usage(process.cpu_usage_tenths_percent),
        format_memory_usage(process.memory_bytes)
    )
}

fn listening_port_row(
    port: &ListeningPort,
    process_name: Option<&str>,
    colors: UiColors,
) -> impl IntoElement {
    let owner = listening_port_owner(port.pid, process_name);
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(1.))
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(format!("{} {}:{}", port.protocol, port.address, port.port)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(10.))
                .text_color(rgb(colors.muted))
                .child(owner),
        )
}

fn listening_port_owner(pid: u32, process_name: Option<&str>) -> String {
    process_name
        .map(|name| format!("{name}({pid})"))
        .unwrap_or_else(|| pid.to_string())
}

fn placeholder(text: impl Into<SharedString>, colors: UiColors) -> impl IntoElement {
    div()
        .p_3()
        .rounded_md()
        .bg(rgb(colors.panel_alt))
        .text_color(rgb(colors.muted))
        .child(text.into())
}

fn icon_button(
    icon_name: IconName,
    id: impl Into<gpui::ElementId>,
    colors: UiColors,
    tooltip: Option<SharedString>,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_lg()
        .text_color(rgb(colors.muted))
        .cursor_pointer()
        .hover(move |element| {
            element
                .bg(rgb(colors.panel_alt))
                .text_color(rgb(colors.text))
        })
        .when_some(tooltip, |element, text| {
            element.tooltip(tooltip_text(text, colors))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(listener)
        .child(icon(icon_name))
}

/// Small button used inside the update popover.
fn popover_button(
    label: &str,
    primary: bool,
    colors: UiColors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(format!("update-popover-{label}"))
        .flex_none()
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |this| {
            this.bg(rgb(colors.accent))
                .text_color(rgb(0xffffff))
                .hover(move |element| element.bg(mix_toward_black(colors.accent, 0.12)))
        })
        .when(!primary, |this| {
            this.bg(rgb(colors.panel_alt))
                .text_color(rgb(colors.text))
                .border_1()
                .border_color(rgb(colors.border))
                .hover(move |element| element.bg(rgb(colors.hover)))
        })
        .on_click(on_click)
        .child(label.to_string())
        .into_any_element()
}

/// Darken a packed 0xRRGGBB color toward black by `amount` (0..1), for button hover states.
fn mix_toward_black(color: u32, amount: f32) -> gpui::Rgba {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xff) as f32;
        (value * (1. - amount)).round().clamp(0., 255.) as u32
    };
    rgb((channel(16) << 16) | (channel(8) << 8) | channel(0))
}

fn format_update_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_000_000 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1_000_000.)
    } else if bytes_per_sec >= 1_000 {
        format!("{:.0} KB/s", bytes_per_sec as f64 / 1_000.)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}

fn top_bar_icon_button(
    icon_name: IconName,
    id: impl Into<gpui::ElementId>,
    colors: UiColors,
    border_left: bool,
    border_right: bool,
    tooltip: Option<SharedString>,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(TOP_BAR_ACTION_SLOT_WIDTH))
        .h_full()
        .when(border_left, |element| {
            element.border_l_1().border_color(rgb(colors.border))
        })
        .when(border_right, |element| {
            element.border_r_1().border_color(rgb(colors.border))
        })
        .child(icon_button(icon_name, id, colors, tooltip, listener))
}

fn item_kind_icon(kind: ItemKind) -> IconName {
    match kind {
        ItemKind::Terminal => IconName::Terminal,
        ItemKind::File => IconName::File,
        ItemKind::Browser => IconName::Browser,
        ItemKind::Settings => IconName::Settings,
    }
}

/// The button glyph for a directory opener. A future custom opener falls back to
/// [`IconName::AppWindow`].
fn icon_for_open_with(opener: OpenWith) -> IconName {
    match opener {
        OpenWith::Finder => IconName::Finder,
        OpenWith::VsCode => IconName::VsCode,
    }
}

fn open_with_label(language: Language, opener: OpenWith) -> &'static str {
    match opener {
        OpenWith::Finder => language.open_with_finder(),
        OpenWith::VsCode => language.open_with_vscode(),
    }
}

/// Open `path` in the chosen application via macOS `open`. Finder opens the directory directly;
/// VS Code is launched by bundle name so it works without the `code` CLI on `PATH`. Failures are
/// logged, not surfaced — the button is best-effort.
fn open_path_with(path: &Path, opener: OpenWith) {
    let mut command = std::process::Command::new("/usr/bin/open");
    match opener {
        OpenWith::Finder => {
            command.arg(path);
        }
        OpenWith::VsCode => {
            command.args(["-a", "Visual Studio Code"]).arg(path);
        }
    }
    if let Err(error) = command.spawn() {
        eprintln!("failed to open {} with {}: {error}", path.display(), opener.slug());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};
    use uuid::Uuid;

    #[test]
    fn scrollbar_thumb_hidden_without_scrollback() {
        // No history → no thumb (scrollbar is hidden entirely).
        assert_eq!(scrollbar_thumb_geometry(400., 0, 24, 0), None);
        // Zero-height track is degenerate.
        assert_eq!(scrollbar_thumb_geometry(0., 100, 24, 0), None);
    }

    #[test]
    fn scrollbar_thumb_sits_at_bottom_and_top_at_the_extremes() {
        let track = 400.;
        let (history, rows) = (100u32, 25u16);
        let thumb = scrollbar_thumb_geometry(track, history, rows, 0).unwrap();
        let travel = track - thumb.height;
        // D = 0 (live bottom) → thumb flush bottom.
        assert!((thumb.top - travel).abs() < 0.01, "bottom: {thumb:?}");
        // D = H (scrolled to top) → thumb flush top.
        let top = scrollbar_thumb_geometry(track, history, rows, history).unwrap();
        assert!(top.top.abs() < 0.01, "top: {top:?}");
        // Thumb represents R/(H+R) of the track: 25/125 = 0.2 → 80px.
        assert!((thumb.height - 80.).abs() < 0.01, "height: {thumb:?}");
    }

    #[test]
    fn scrollbar_thumb_respects_minimum_height() {
        // Huge history would give a sub-pixel thumb; clamp to the minimum.
        let thumb = scrollbar_thumb_geometry(400., 1_000_000, 24, 0).unwrap();
        assert!((thumb.height - TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT).abs() < 0.01);
    }

    #[test]
    fn scrollbar_offset_inverts_thumb_top() {
        let track = 400.;
        let (history, rows) = (100u32, 25u16);
        // Round-trip a few offsets through geometry → thumb top → offset.
        for offset in [0u32, 25, 50, 75, 100] {
            let thumb = scrollbar_thumb_geometry(track, history, rows, offset).unwrap();
            let recovered = scrollbar_offset_from_thumb_top(track, history, rows, thumb.top);
            assert_eq!(recovered, offset, "offset {offset} did not round-trip");
        }
    }

    #[test]
    fn scrollbar_offset_clamps_out_of_range_positions() {
        let track = 400.;
        let (history, rows) = (100u32, 25u16);
        // Above the track → oldest line (full history). Below → live bottom (0).
        assert_eq!(scrollbar_offset_from_thumb_top(track, history, rows, -50.), history);
        assert_eq!(scrollbar_offset_from_thumb_top(track, history, rows, 10_000.), 0);
    }

    fn process(pid: u32, parent_pid: Option<u32>, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid,
            name: name.to_owned(),
            cpu_usage_tenths_percent: Some(0),
            memory_bytes: Some(0),
        }
    }

    #[test]
    fn terminal_container_rounding_respects_sidebars_and_split_edges() {
        let all_rounded = TerminalContainerCorners {
            top_left: true,
            top_right: true,
            bottom_left: true,
            bottom_right: true,
        };
        let square_bottom = TerminalContainerCorners {
            top_left: true,
            top_right: true,
            bottom_left: false,
            bottom_right: false,
        };
        let square_left = TerminalContainerCorners {
            top_left: false,
            top_right: true,
            bottom_left: false,
            bottom_right: true,
        };
        let square_right = TerminalContainerCorners {
            top_left: true,
            top_right: false,
            bottom_left: true,
            bottom_right: false,
        };

        // Both sidebars separate the root terminal from the window, but its bottom still touches.
        assert_eq!(
            terminal_container_corners(true, true, true, false, false),
            square_bottom,
        );
        // A top group in a vertical split is also separated from the bottom window edge.
        assert_eq!(
            terminal_container_corners(true, true, false, false, false),
            all_rounded,
        );
        // A collapsed left sidebar squares both corners on that window edge.
        assert_eq!(
            terminal_container_corners(true, true, false, true, false),
            square_left,
        );
        // The two terminal corners meeting at a horizontal split stay square.
        assert_eq!(
            terminal_container_corners(true, false, false, false, false),
            square_right,
        );
        assert_eq!(
            terminal_container_corners(false, true, false, false, false),
            square_left,
        );
        // With every exposed root edge touching the window, no corner is rounded.
        assert_eq!(
            terminal_container_corners(true, true, true, true, true),
            TerminalContainerCorners {
                top_left: false,
                top_right: false,
                bottom_left: false,
                bottom_right: false,
            },
        );
    }

    #[test]
    fn terminal_container_owns_only_its_content_edge_borders() {
        let no_borders = TerminalContainerBorders {
            left: false,
            right: false,
            bottom: false,
        };

        // A root terminal touching the window on every exposed side has no outer side borders.
        assert_eq!(
            terminal_container_borders(true, true, true, true, true),
            no_borders,
        );
        // At a horizontal split, the left-hand terminal owns the internal vertical seam.
        assert_eq!(
            terminal_container_borders(true, false, true, false, false),
            TerminalContainerBorders {
                left: true,
                right: true,
                bottom: false,
            },
        );
        assert_eq!(
            terminal_container_borders(false, true, true, false, false),
            TerminalContainerBorders {
                left: false,
                right: true,
                bottom: false,
            },
        );
        // At a vertical split, the upper terminal owns the internal horizontal seam.
        assert_eq!(
            terminal_container_borders(true, true, false, false, false),
            TerminalContainerBorders {
                left: true,
                right: true,
                bottom: true,
            },
        );
    }

    #[test]
    fn every_split_groups_active_terminal_is_visible() {
        let first_session = Uuid::new_v4();
        let hidden_session = Uuid::new_v4();
        let right_session = Uuid::new_v4();
        let first_item = TabItem::terminal(first_session, "first");
        let first_item_id = first_item.id;
        let mut layout = LayoutNode::group(first_item);
        let first_group_id = layout.first_group_id();
        layout
            .find_group_mut(first_group_id)
            .expect("first group")
            .add_item(TabItem::terminal(hidden_session, "hidden"));
        layout
            .split_group_direction(
                first_group_id,
                Direction::Right,
                TabItem::terminal(right_session, "right"),
            )
            .expect("right group");

        let visible = visible_active_terminal_sessions(&layout)
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(visible, HashSet::from([hidden_session, right_session]));
        assert!(!layout_has_visible_active_terminal(&layout, first_session));
        assert!(layout_has_visible_active_terminal(&layout, right_session));

        layout
            .find_group_mut(first_group_id)
            .expect("first group")
            .active_item_id = Some(first_item_id);
        let visible = visible_active_terminal_sessions(&layout)
            .into_iter()
            .collect::<HashSet<_>>();
        assert_eq!(visible, HashSet::from([first_session, right_session]));
        assert!(!layout_has_visible_active_terminal(&layout, hidden_session));
    }

    fn key_event(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers,
                key: key.to_owned(),
                key_char: key_char.map(str::to_owned),
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn terminal_size_matches_the_visible_cell_grid() {
        assert_eq!(
            terminal_size_for_viewport(800., 360., 8., 18., 1.),
            TerminalSize {
                columns: 100,
                rows: 20,
                cell_width: 8,
                cell_height: 18,
            }
        );
        assert_eq!(
            terminal_size_for_viewport(1., 1., 8., 18., 1.),
            TerminalSize {
                columns: 2,
                rows: 1,
                cell_width: 8,
                cell_height: 18,
            }
        );
        assert_eq!(
            terminal_size_for_viewport(800., 360., 8.5, 18., 2.),
            TerminalSize {
                columns: 94,
                rows: 20,
                cell_width: 17,
                cell_height: 36,
            }
        );
    }

    #[test]
    fn processes_are_flattened_as_a_stable_parent_child_tree() {
        let processes = vec![
            process(13, Some(11), "worker"),
            process(10, Some(1), "zsh"),
            process(12, Some(10), "bash-b"),
            process(11, Some(10), "bash-a"),
            process(14, Some(12), "ruby"),
        ];

        let rows = process_tree_rows(&processes);
        assert_eq!(
            rows.iter()
                .map(|row| processes[row.process_index].pid)
                .collect::<Vec<_>>(),
            [10, 11, 13, 12, 14]
        );
        assert_eq!(
            rows.iter()
                .map(|row| row.branch_has_next.clone())
                .collect::<Vec<_>>(),
            [
                vec![],
                vec![true],
                vec![true, false],
                vec![false],
                vec![false, false]
            ]
        );
    }

    #[test]
    fn malformed_process_cycles_are_rendered_once_instead_of_recursing_forever() {
        let processes = vec![process(20, Some(21), "a"), process(21, Some(20), "b")];
        let rows = process_tree_rows(&processes);

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .map(|row| processes[row.process_index].pid)
                .collect::<HashSet<_>>(),
            HashSet::from([20, 21])
        );
    }

    #[test]
    fn process_metrics_use_compact_sidebar_units() {
        assert_eq!(format_cpu_usage(Some(123)), "12.3%");
        assert_eq!(format_cpu_usage(None), "—");
        assert_eq!(format_memory_usage(Some(512)), "512B");
        assert_eq!(format_memory_usage(Some(12 * 1024)), "12KB");
        assert_eq!(format_memory_usage(Some(12 * 1024 * 1024)), "12.0MB");
        assert_eq!(format_memory_usage(None), "—");

        let mut process = process(42, None, "node");
        process.cpu_usage_tenths_percent = Some(12);
        process.memory_bytes = Some(1536 * 1024);
        assert_eq!(format_process_metrics(&process), "1.2% / 1.5MB");
    }

    #[test]
    fn listening_port_owners_use_compact_process_pid_format() {
        assert_eq!(listening_port_owner(41_238, Some("node")), "node(41238)");
        assert_eq!(listening_port_owner(41_238, None), "41238");
    }

    #[test]
    fn ports_section_is_omitted_when_nothing_is_listening() {
        let inspection = SessionInspection {
            session_id: Uuid::nil(),
            processes: Vec::new(),
            ports: Vec::new(),
        };
        let colors = UiColors {
            background: 0,
            panel: 0,
            panel_alt: 0,
            hover: 0,
            border: 0,
            text: 0,
            muted: 0,
            accent: 0,
        };

        assert!(ports_section(&inspection, colors, Language::English).is_none());
    }

    #[test]
    fn terminal_input_preserves_non_bmp_utf8_text() {
        let text = "你好🙂👩🏽‍💻";
        assert_eq!(terminal_text_bytes(text), text.as_bytes());
        assert_eq!(
            terminal_bytes(&key_event("🙂", Some("🙂"), Modifiers::default())),
            Some("🙂".as_bytes().to_vec())
        );
    }

    #[test]
    fn terminal_input_does_not_forward_appkit_reserved_characters() {
        for character in ['\u{f700}', '\u{f8ff}', '\u{fdd0}', '\u{fffe}', '\u{ffff}'] {
            let text = character.to_string();
            assert_eq!(
                terminal_bytes(&key_event(&text, Some(&text), Modifiers::default())),
                None,
                "reserved key character U+{:04X} must stay in AppKit",
                character as u32
            );
        }

        assert_eq!(
            terminal_bytes(&key_event(
                "e",
                Some("e"),
                Modifiers {
                    function: true,
                    ..Modifiers::default()
                },
            )),
            None
        );
    }

    #[test]
    fn terminal_input_keeps_known_navigation_keys_with_function_modifier() {
        assert_eq!(
            terminal_bytes(&key_event(
                "up",
                None,
                Modifiers {
                    function: true,
                    ..Modifiers::default()
                },
            )),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn kitty_keyboard_mode_reports_press_repeat_release_and_associated_text() {
        let flags = KITTY_DISAMBIGUATE_ESCAPE_CODES
            | KITTY_REPORT_EVENT_TYPES
            | KITTY_REPORT_ALL_KEYS
            | KITTY_REPORT_ASSOCIATED_TEXT;
        let control_c = key_event(
            "c",
            Some("c"),
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(
            terminal_bytes_with_mode(&control_c, flags),
            Some(b"\x1b[99;5:1u".to_vec())
        );
        assert_eq!(
            kitty_key_bytes(&control_c.keystroke, flags, 2),
            Some(b"\x1b[99;5:2u".to_vec())
        );
        assert_eq!(
            kitty_key_bytes(&control_c.keystroke, flags, 3),
            Some(b"\x1b[99;5:3u".to_vec())
        );
        assert_eq!(
            terminal_bytes_with_mode(&key_event("up", None, Modifiers::default()), flags,),
            Some(b"\x1b[1;1:1A".to_vec())
        );
        assert_eq!(
            terminal_bytes_with_mode(
                &key_event(
                    "up",
                    None,
                    Modifiers {
                        function: true,
                        ..Modifiers::default()
                    }
                ),
                flags,
            ),
            Some(b"\x1b[1;1:1A".to_vec())
        );
    }

    #[test]
    fn macos_character_palette_shortcut_is_recognized() {
        assert!(is_character_palette_shortcut(&key_event(
            "space",
            Some(" "),
            Modifiers {
                control: true,
                platform: true,
                ..Modifiers::default()
            },
        )));
        assert!(!is_character_palette_shortcut(&key_event(
            "space",
            Some(" "),
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        )));
    }

    #[test]
    fn terminal_mouse_coordinates_clamp_to_the_visible_grid() {
        let session_id = Uuid::nil();
        let viewport = TerminalViewport {
            session_id,
            bounds: Bounds::new(point(px(20.), px(40.)), size(px(80.), px(36.))),
            cell_width: 8.,
            line_height: 18.,
            rows: 2,
            columns: 10,
        };
        assert_eq!(
            terminal_point_from_position(viewport, point(px(44.), px(59.))),
            Some(TerminalPoint { line: 1, column: 3 })
        );
        assert_eq!(
            terminal_point_from_position(viewport, point(px(-100.), px(1000.))),
            Some(TerminalPoint { line: 1, column: 0 })
        );
    }

    #[test]
    fn terminal_pointer_protocol_conversion_preserves_buttons_modifiers_and_subpixels() {
        assert_eq!(
            protocol_mouse_button(MouseButton::Left),
            Some(TerminalMouseButton::Left)
        );
        assert_eq!(
            protocol_mouse_button(MouseButton::Middle),
            Some(TerminalMouseButton::Middle)
        );
        assert_eq!(
            protocol_mouse_button(MouseButton::Right),
            Some(TerminalMouseButton::Right)
        );
        assert_eq!(
            protocol_mouse_button(MouseButton::Navigate(gpui::NavigationDirection::Back)),
            None
        );
        assert_eq!(
            protocol_terminal_modifiers(gpui::Modifiers {
                control: true,
                alt: true,
                shift: true,
                platform: true,
                function: true,
            }),
            TerminalModifiers {
                control: true,
                alt: true,
                shift: true,
            }
        );
        assert_eq!(fixed_terminal_scroll_delta(0.5), 512);
        assert_eq!(fixed_terminal_scroll_delta(-1.25), -1280);
        assert_eq!(fixed_terminal_scroll_delta(f32::NAN), 0);
        assert_eq!(fixed_terminal_scroll_delta(f32::MAX), i32::MAX);
    }

    #[test]
    fn a_sole_tab_dragged_over_its_own_content_has_no_drop_target() {
        let group_id = GroupId::new_v4();

        assert!(!should_show_content_drop_target(group_id, group_id, 1));
        assert!(should_show_content_drop_target(group_id, group_id, 2));
        assert!(should_show_content_drop_target(
            group_id,
            GroupId::new_v4(),
            1
        ));
    }

    #[test]
    fn tab_bar_uses_each_tabs_halfway_point_as_its_insertion_boundary() {
        assert_eq!(tab_bar_insertion_index(2, 100., 50.), 2);
        assert_eq!(tab_bar_insertion_index(2, 100., 50.1), 3);
    }

    #[test]
    fn content_drop_zones_follow_vscode_style_edge_thresholds() {
        assert_eq!(
            content_drop_zone(1000., 600., 500., 300.),
            TabDropZone::Center
        );
        assert_eq!(
            content_drop_zone(1000., 600., 50., 300.),
            TabDropZone::Edge(Direction::Left)
        );
        assert_eq!(
            content_drop_zone(1000., 600., 950., 300.),
            TabDropZone::Edge(Direction::Right)
        );
        assert_eq!(
            content_drop_zone(1000., 600., 500., 30.),
            TabDropZone::Edge(Direction::Up)
        );
        assert_eq!(
            content_drop_zone(1000., 600., 500., 570.),
            TabDropZone::Edge(Direction::Down)
        );
    }

    #[test]
    fn progress_labels_include_status_percent_and_recency() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let label = progress_label(
            TerminalProgress {
                state: TerminalProgressState::Paused,
                percent: Some(73),
                updated_at_unix_ms: now.saturating_sub(3_000),
            },
            Language::English,
        );
        assert_eq!(label, "Paused · 73% · updated 3s ago");

        let completed = progress_label(
            TerminalProgress {
                state: TerminalProgressState::Normal,
                percent: Some(100),
                updated_at_unix_ms: now,
            },
            Language::English,
        );
        assert!(completed.starts_with("Complete · 100% · updated"));
    }
}

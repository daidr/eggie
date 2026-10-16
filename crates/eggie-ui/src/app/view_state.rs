use super::*;

#[derive(Clone, Copy, Default)]
pub(super) struct MarqueeState {
    pub(super) overflow: f32,
    pub(super) hovered: bool,
    pub(super) ever_hovered: bool,
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) started_at: Option<Instant>,
    pub(super) duration: Duration,
    pub(super) revision: u64,
}

pub(super) fn marquee_duration(distance: f32, hovered: bool) -> Duration {
    if hovered {
        Duration::from_millis(((distance / 28.).clamp(0.7, 8.) * 1000.) as u64)
    } else {
        Duration::from_millis(260)
    }
}

pub(super) fn marquee_offset(state: MarqueeState, now: Instant) -> f32 {
    let Some(started_at) = state.started_at else {
        return state.to;
    };
    let progress = (now.saturating_duration_since(started_at).as_secs_f32()
        / state.duration.as_secs_f32())
    .clamp(0., 1.);
    state.from + (state.to - state.from) * gpui::ease_in_out(progress)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalPreviewAnchor {
    Sidebar(SessionId),
    Tab(ItemId, SessionId),
}
impl TerminalPreviewAnchor {
    pub(super) fn session_id(self) -> SessionId {
        match self { Self::Sidebar(id) | Self::Tab(_, id) => id }
    }
}

pub(super) struct TerminalPreviewState {
    pub(super) anchor: TerminalPreviewAnchor,
    pub(super) session_id: SessionId,
    pub(super) content_revision: u64,
    pub(super) window: gpui::WindowHandle<PopupWindow>,
    pub(super) anchor_hovered: bool,
    pub(super) popup_hovered: bool,
    pub(super) leave_revision: u64,
}

impl TerminalPreviewState {
    pub(super) fn is_current_target(&self, anchor: TerminalPreviewAnchor, revision: u64) -> bool {
        self.anchor == anchor && self.content_revision == revision
    }
}

pub(super) struct ProjectPopoverState {
    pub(super) project_id: ProjectId,
    pub(super) window: gpui::WindowHandle<PopupWindow>,
    pub(super) anchor_hovered: bool,
    pub(super) popup_hovered: bool,
    pub(super) leave_revision: u64,
    pub(super) keyboard_target: Rc<std::cell::Cell<Option<ItemId>>>,
    pub(super) scroll: ScrollHandle,
}

pub(super) struct ProjectSelectorState {
    pub(super) window: gpui::WindowHandle<PopupWindow>,
    pub(super) keyboard_target: Rc<std::cell::Cell<ProjectId>>,
    pub(super) scroll: ScrollHandle,
}

pub(super) struct ProjectPopoverTab {
    pub(super) location: eggie_domain::ItemLocation,
    pub(super) session_id: Option<SessionId>,
    pub(super) item_id: ItemId,
    pub(super) title: String,
    pub(super) subtitle: Option<String>,
    pub(super) icon: ProcessIcon,
    pub(super) active: bool,
}

pub(super) fn project_popover_selection_index(
    tabs: &[ProjectPopoverTab],
    keyboard_target: Option<ItemId>,
) -> Option<usize> {
    keyboard_target
        .and_then(|id| tabs.iter().position(|tab| tab.item_id == id))
        .or_else(|| tabs.iter().position(|tab| tab.active))
}

pub(super) fn program_state_label(state: TerminalProgramState, language: Language) -> &'static str {
    match (state, language) {
        (TerminalProgramState::Idle, Language::English) => "Idle",
        (TerminalProgramState::Working, Language::English) => "Working",
        (TerminalProgramState::Done, Language::English) => "Done",
        (TerminalProgramState::Blocked, Language::English) => "Blocked",
        (TerminalProgramState::Error, Language::English) => "Error",
        (TerminalProgramState::Idle, Language::SimplifiedChinese) => "空闲",
        (TerminalProgramState::Working, Language::SimplifiedChinese) => "工作中",
        (TerminalProgramState::Done, Language::SimplifiedChinese) => "已完成",
        (TerminalProgramState::Blocked, Language::SimplifiedChinese) => "等待操作",
        (TerminalProgramState::Error, Language::SimplifiedChinese) => "出错",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IconIndicator {
    Working(Option<u8>),
    Attention(TerminalProgramState),
    Progress(TerminalProgress),
}

impl IconIndicator {
    pub(super) fn is_animated(self) -> bool {
        matches!(
            self,
            Self::Working(None)
                | Self::Progress(TerminalProgress {
                    state: TerminalProgressState::Indeterminate,
                    ..
                })
        )
    }
}

pub(super) fn icon_indicator(
    status: Option<&TerminalProgramStatus>,
    progress: Option<TerminalProgress>,
) -> Option<IconIndicator> {
    match status {
        Some(status) if status.state == TerminalProgramState::Working => {
            Some(IconIndicator::Working(status.progress))
        }
        Some(status) if status.state != TerminalProgramState::Idle => {
            Some(IconIndicator::Attention(status.state))
        }
        _ => progress.map(IconIndicator::Progress),
    }
}
pub(super) const CONTENT_SPLIT_EDGE_FACTOR: f32 = 0.1;
pub(super) const LEFT_SIDEBAR_MIN_WIDTH: f32 = 180.;
pub(super) const LEFT_SIDEBAR_MAX_WIDTH: f32 = 420.;
pub(super) const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 220.;
pub(super) const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 520.;
pub(super) const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 5.;
// The split divider has a wide invisible grab area for easy clicking, and a thin visible line.
pub(super) const SPLIT_DIVIDER_HANDLE_WIDTH: f32 = 8.;
pub(super) const SPLIT_DIVIDER_LINE_WIDTH: f32 = 2.;
pub(super) const SIDEBAR_SCROLLBAR_WIDTH: f32 = 8.;
pub(super) const SIDEBAR_SCROLLBAR_THUMB_WIDTH: f32 = 4.;
pub(super) const SIDEBAR_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 24.;
// The terminal scrollbar occupies a thin strip on the right edge of each pane. It is always shown
// while there is scrollback, and darkens slightly on hover/drag. The track is a wider transparent
// grab/hit area; the visible thumb is narrower.
pub(super) const TERMINAL_SCROLLBAR_WIDTH: f32 = 12.;
pub(super) const TERMINAL_SCROLLBAR_THUMB_WIDTH: f32 = 6.;
pub(super) const TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT: f32 = 24.;
pub(super) const SESSION_INSPECTION_INTERVAL: Duration = Duration::from_secs(1);
pub(super) const SNAPSHOT_WAIT_TIMEOUT: Duration = Duration::from_millis(50);
pub(super) const PROCESS_ROW_HEIGHT: f32 = 20.;
pub(super) const TERMINAL_IMAGE_CHUNK_SIZE: u32 = 16 * 1024 * 1024;
/// A bell triggers a single smooth pulse: the tab tint fades in, then back out, over this span.
pub(super) const BELL_FLASH_DURATION: Duration = Duration::from_millis(420);
/// Repaint cadence during the pulse — small enough for a continuous fade.
pub(super) const BELL_FLASH_STEP: Duration = Duration::from_millis(16);
/// Peak alpha of the accent tint at the middle of the pulse (0x00..=0xff).
pub(super) const BELL_FLASH_PEAK_ALPHA: f32 = 0x66 as f32;
/// Full cursor blink cycle (on + off). The cursor is solid for the first half and hidden for the
/// second. ~1s matches the customary terminal blink rate.
pub(super) const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TerminalImageStreamKey {
    pub(super) session_id: SessionId,
    pub(super) image_id: u32,
}

/// Pull one terminal image over an already-open, reusable daemon connection.
///
/// The caller owns the [`DaemonConnection`] and keeps it across generations, so an animated image
/// no longer pays a fresh `connect()` per frame. Retries/backoff are the caller's responsibility
/// (the image-stream worker reconnects and paces itself), which is why this takes `&mut` rather
/// than owning the connection: on success the caller keeps reusing it, on error the caller drops
/// it and reconnects next round.
pub(super) fn fetch_terminal_image_chunked(
    connection: &mut DaemonConnection,
    session_id: SessionId,
    descriptor: &TerminalImageDescriptor,
) -> anyhow::Result<TerminalImageData> {
    let expected_length = descriptor
        .width
        .checked_mul(descriptor.height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("terminal image dimensions overflow"))?;

    // Whole-image fast path: the daemon delivers the image either as an shm segment (mapped
    // zero-copy here) or as a single complete inline frame. An image larger than one wire chunk
    // comes back as InlineTooLarge and falls through to the chunked loop below.
    let frame = connection.fetch_terminal_image(session_id, descriptor.key, expected_length)?;
    let pixels = match frame {
        TerminalImageFrame::Shm {
            metadata,
            segment_name,
        } => {
            // Map (and immediately unlink) the segment BEFORE validating metadata: otherwise a
            // metadata mismatch would return before we ever open the segment, leaking it (the
            // daemon has already handed off the unlink). Mapping first means any early return drops
            // the mapping and reclaims the object.
            let mmap =
                eggie_ipc::wire::map_image_shm_segment(&segment_name, expected_length as usize)?;
            verify_image_metadata(&metadata, descriptor, expected_length)?;
            Some(PixelStore::Mapped {
                mmap: Arc::new(mmap),
                len: expected_length as usize,
            })
        }
        TerminalImageFrame::Inline { metadata, pixels } => {
            verify_image_metadata(&metadata, descriptor, expected_length)?;
            if pixels.len() != expected_length as usize {
                anyhow::bail!("inline terminal image length mismatch");
            }
            Some(PixelStore::Owned(Arc::new(pixels)))
        }
        // Image spans multiple wire chunks; fall through to the chunked loop.
        TerminalImageFrame::InlineTooLarge { .. } => None,
    };
    if let Some(pixels) = pixels {
        return Ok(TerminalImageData {
            key: TerminalTextureKey {
                session_id,
                image: descriptor.key,
            },
            width: descriptor.width,
            height: descriptor.height,
            pixels,
        });
    }

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
        pixels: PixelStore::Owned(Arc::new(pixels)),
    })
}

/// Validate a fetched image's metadata against the descriptor the UI requested, guarding against a
/// generation swap racing the transfer.
pub(super) fn verify_image_metadata(
    metadata: &eggie_ipc::TerminalImageChunkMetadata,
    descriptor: &TerminalImageDescriptor,
    expected_length: u32,
) -> anyhow::Result<()> {
    if metadata.key != descriptor.key
        || metadata.width != descriptor.width
        || metadata.height != descriptor.height
        || metadata.total_length != expected_length
    {
        anyhow::bail!("terminal image metadata changed during transfer");
    }
    Ok(())
}

pub(super) const PROCESS_TREE_LEVEL_WIDTH: f32 = 16.;
pub(super) const PROCESS_TREE_LINE_X: f32 = 3.;
pub(super) const PROCESS_TREE_ARM_LENGTH: f32 = 9.;
pub(super) const PROCESS_TREE_CORNER_RADIUS: f32 = 3.;
pub(super) const TRAFFIC_LIGHT_DIAMETER: f32 = 14.;
pub(super) const TRAFFIC_LIGHT_INSET: f32 = (TITLEBAR_HEIGHT - TRAFFIC_LIGHT_DIAMETER) / 2.;
pub(super) const TRAFFIC_LIGHT_INSET_WIDTH: f32 = 72.;

pub(super) static NEXT_INPUT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct NotificationRoute {
    pub(crate) client: DaemonClient,
    pub(crate) session_id: SessionId,
    pub(crate) notification_id: String,
}

pub(crate) type NotificationRoutes = Rc<RefCell<HashMap<String, NotificationRoute>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RightTab {
    #[default]
    Info,
    Commands,
    Notes,
}

/// State for the Notes tab's multi-line editor: the input entity, its change subscription, and the
/// debounce epoch used to coalesce autosaves (only the latest scheduled save writes to disk).
pub(super) struct NotesEditor {
    pub(super) input: Entity<TextInput>,
    pub(super) _subscription: Subscription,
    /// The textarea's own vertical scroll handle, so the scrollbar sits on the notes editor
    /// (which grows by line count) instead of the whole sidebar.
    pub(super) scroll_handle: ScrollHandle,
    /// Incremented on every change; a scheduled debounce save only persists if the epoch it
    /// captured still matches, so bursts of keystrokes collapse into one write.
    pub(super) save_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarEdge {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TabDropZone {
    Center,
    Edge(Direction),
    TabBar { insertion_index: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TabDropTarget {
    pub(super) group_id: GroupId,
    pub(super) zone: TabDropZone,
}

#[derive(Clone, Copy)]
pub(super) struct TerminalViewport {
    pub(super) session_id: SessionId,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) cell_width: f32,
    pub(super) line_height: f32,
    pub(super) rows: u16,
    pub(super) columns: u16,
}

#[derive(Clone, Copy)]
pub(super) struct TerminalSelectionDrag {
    pub(super) group_id: GroupId,
    pub(super) session_id: SessionId,
    pub(super) dragged: bool,
}

/// State for an in-progress scrollbar thumb drag. `grab_offset` is the pointer's Y distance from the
/// thumb's top at grab time, so the thumb doesn't jump under the cursor. `pending_offset` is the
/// last offset we asked the daemon for; it drives optimistic thumb rendering so the thumb tracks the
/// pointer instantly rather than waiting for the round-tripped snapshot.
#[derive(Clone, Copy)]
pub(super) struct ScrollbarDrag {
    pub(super) group_id: GroupId,
    pub(super) session_id: SessionId,
    pub(super) grab_offset: f32,
    pub(super) pending_offset: u32,
}

/// State for the in-terminal search bar (⌘F). At most one search is active at a time, targeting the
/// currently focused session. Closing the bar or switching sessions resets it.
pub(super) struct TerminalSearchUi {
    /// The session the search bar is bound to.
    pub(super) session_id: SessionId,
    /// The reusable text-input component holding the query text, selection, cursor, and focus.
    pub(super) input: Entity<TextInput>,
    /// Whether the query is interpreted as a regular expression.
    pub(super) regex: bool,
    /// The most recent result from the daemon, used to render highlights and the match counter.
    pub(super) result: Option<TerminalSearchResult>,
    /// Subscription to the input's events; dropped (unsubscribed) when the search bar closes.
    pub(super) _subscription: Subscription,
}

/// State for the command palette (⌘⇧P). A centered modal listing every command; the query filters
/// it, ↑/↓ move the selection, Enter/click runs the selected command, Esc closes. At most one is
/// open at a time. The command data comes from [`crate::command_palette::palette_entries`].
///
/// The same modal doubles as the profile picker ([`PaletteMode::Profiles`]): identical chrome and key
/// handling, but the rows are profiles and confirming creates a terminal instead of dispatching an
/// action. Reusing the overlay keeps one focus/Esc path rather than two competing modals.
pub(super) struct CommandPaletteUi {
    /// The reusable text-input component holding the filter query, cursor, and focus.
    pub(super) input: Entity<TextInput>,
    /// Index of the highlighted row within the *filtered* list (clamped on every change).
    pub(super) selected_index: usize,
    /// Scroll handle for the results list, so the selected row can be kept in view.
    pub(super) scroll_handle: ScrollHandle,
    /// Whether this modal is listing commands or profiles.
    pub(super) mode: PaletteMode,
    /// Subscription to the input's events; dropped (unsubscribed) when the palette closes.
    pub(super) _subscription: Subscription,
}

/// What the palette modal is currently listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PaletteMode {
    Commands,
    Terminals,
    Windows,
    MoveToWindow(eggie_domain::ItemLocation),
    /// The profile picker: rows are profiles, confirming launches a terminal from the chosen one.
    Profiles,
    /// The same profile list, but confirming sets the named project's default profile instead of
    /// launching anything. A distinct mode rather than a flag on `Profiles` so `confirm` cannot
    /// accidentally do both.
    ProjectDefaultProfile(ProjectId),
}

/// One rendered row of the palette modal, flattened from either a command or a profile so the render
/// path has a single shape to draw.
pub(super) struct PaletteRow {
    pub(super) label: String,
    /// Right-aligned muted text: a command's shortcut, or a profile's "project default" marker.
    pub(super) trailing: Option<String>,
    /// Leading accent dot (`0xRRGGBB`) for a profile that defines one.
    pub(super) badge_color: Option<u32>,
}

#[derive(Clone, Copy)]
pub(super) struct TabIconState {
    pub(super) icon: ProcessIcon,
    pub(super) previous: Option<ProcessIcon>,
    pub(super) revision: u64,
    pub(super) started_at: Instant,
}

#[derive(Clone, Copy)]
pub(super) struct SidebarStatusRowState {
    pub(super) expanded: bool,
    pub(super) from_height: f32,
    pub(super) revision: u64,
    pub(super) started_at: Instant,
}

impl SidebarStatusRowState {
    pub(super) fn target_height(self) -> f32 {
        if self.expanded {
            SIDEBAR_STATUS_ROW_HEIGHT
        } else {
            SIDEBAR_SESSION_ROW_HEIGHT
        }
    }

    pub(super) fn current_height(self, now: Instant) -> f32 {
        let progress = (now.duration_since(self.started_at).as_secs_f32()
            / SIDEBAR_STATUS_TRANSITION.as_secs_f32())
        .clamp(0., 1.);
        self.from_height + (self.target_height() - self.from_height) * gpui::ease_in_out(progress)
    }
}

#[derive(Clone, Debug)]
pub(super) struct DraggedTab {
    pub(super) source_window_id: WindowId,
    pub(super) project_id: ProjectId,
    pub(super) source_group_id: GroupId,
    pub(super) item_id: ItemId,
    pub(super) icon: ProcessIcon,
    pub(super) dark: bool,
    pub(super) title: SharedString,
    pub(super) active: bool,
    pub(super) colors: UiColors,
}

impl DraggedTab {
    pub(super) fn preview_element(&self) -> AnyElement {
        div()
            .flex()
            .items_center()
            .min_w(px(TAB_MIN_WIDTH))
            .max_w(px(TAB_MAX_WIDTH))
            .h(px(TAB_BAR_HEIGHT - TAB_BAR_PADDING * 2.))
            .overflow_hidden()
            .pl(px(TAB_ICON_LEADING_PADDING))
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
                img(
                    process_icons::card_stack(&[self.icon], self.dark, SIDEBAR_CARD_SIZE)
                        .expect("single process card"),
                )
                .flex_none()
                .w(px(process_icons::stack_width(1, SIDEBAR_CARD_SIZE)))
                .h(px(SIDEBAR_CARD_SIZE + 5.)),
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

pub(super) struct DraggedTabPreview {
    pub(super) tab: DraggedTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TerminalContainerCorners {
    pub(super) top_left: bool,
    pub(super) top_right: bool,
    pub(super) bottom_left: bool,
    pub(super) bottom_right: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TerminalContainerBorders {
    pub(super) left: bool,
    pub(super) right: bool,
    pub(super) bottom: bool,
}

/// A reusable single-line text tooltip for controls and icon indicators.
pub(super) struct TextTooltip {
    pub(super) text: SharedString,
    pub(super) colors: UiColors,
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
pub(crate) fn tooltip_text(
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

/// Identifies one of the two directory rows in the right sidebar's Info tab. Used as the key for the
/// per-row "open with" dropdown state so each row's split-button anchors its own dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DirectoryField {
    Current,
    Initial,
}

impl DirectoryField {
    /// A stable ascii slug for element ids.
    pub(super) fn slug(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Initial => "initial",
        }
    }
}

pub(crate) enum NewWindowInit {
    Empty,
    EmptyAt(Point<Pixels>),
    OpenDir { project_id: ProjectId },
    Restore(eggie_domain::WindowPresentation),
}

/// Fallback working directory when a project's root can't be resolved: the user's home, or `.`.
pub(super) fn fallback_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

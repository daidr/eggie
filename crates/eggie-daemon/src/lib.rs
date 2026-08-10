use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::{Boundary, Column, Direction, Line, Point, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        ClipboardType, Config, Osc52, Term, TermMode,
        cell::{Cell, Flags},
        color::COUNT,
        kitty_graphics::{ImageKey as KittyImageKey, PixelBuffer},
        point_to_viewport,
        search::{RegexIter, RegexSearch},
    },
    tty,
    vte::ansi::{
        Color, CursorShape, CursorStyle, NamedColor, ProgressState as VteProgressState, Rgb,
        SemanticPrompt, SemanticPromptAction,
    },
};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::read::DecoderReader;
use eggie_domain::{ProjectId, SessionId};
#[cfg(test)]
use eggie_protocol::encode_line;
use eggie_protocol::{
    ClientRequest, DaemonResponse, ListeningPort, PROTOCOL_VERSION, ProcessInfo, ProcessSummary,
    SessionInspection, SessionStatus, SessionSummary, TERMINAL_SCROLL_DELTA_SCALE,
    TerminalAppearance, TerminalAttentionRequest, TerminalCell, TerminalCellPosition,
    TerminalClipboardContent, TerminalClipboardSelection, TerminalColorOverride,
    TerminalCommandRecord, TerminalCursorShape, TerminalFileTransferOffer,
    TerminalFileTransferProtocol, TerminalImageDescriptor, TerminalImageKey,
    TerminalImagePlacement, TerminalInputModes, TerminalModifiers, TerminalMouseAction,
    TerminalMouseButton, TerminalMouseEncoding, TerminalMouseEvent, TerminalMousePosition,
    TerminalMouseTracking, TerminalNotification, TerminalOscEvent, TerminalOscEventPayload,
    TerminalOscEventUpdate, TerminalProgress, TerminalProgressState, TerminalProgressTimeouts,
    TerminalProgressUpdate, TerminalReportedLocation, TerminalScrollCommand, TerminalScrollEvent,
    TerminalScrollPhase, TerminalScrollUnit, TerminalSearchDirection, TerminalSearchMatch,
    TerminalSearchRequest,
    TerminalSearchResult, TerminalSemanticPhase, TerminalSelectionKind, TerminalSelectionRange,
    TerminalSelectionSide, TerminalShellIntegrationState, TerminalSize,
    TerminalLinkRange, TerminalSnapshot, TerminalSnapshotDelta, TerminalUserVariable,
};
use flate2::write::ZlibDecoder;
use parking_lot::{Condvar, Mutex, RwLock};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, OpenOptions},
    io::{BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    ops::RangeInclusive,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const DAEMON_ARGUMENT: &str = "--eggie-daemon";
const BUNDLED_ALACRITTY_TERMINFO: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/alacritty.terminfo"));
static INSTALLED_TERMINFO: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
const BUNDLED_ZSH_ZSHENV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eggie-zsh-zshenv"));
const BUNDLED_ZSH_INTEGRATION: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/eggie-zsh-integration"));
const BUNDLED_BASH_INTEGRATION: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eggie-bash"));
static INSTALLED_SHELL_INTEGRATION: OnceLock<std::result::Result<PathBuf, String>> =
    OnceLock::new();
static RENDER_METRICS_ENABLED: OnceLock<bool> = OnceLock::new();
/// Regex for auto-detecting bare URLs. regex-automata (the terminal core's engine) has no
/// look-around, so the scheme is anchored on `://` (or the literal `mailto:`) and the body is a
/// greedy run of non-whitespace, non-control, non-delimiter characters. Trailing punctuation and
/// unbalanced brackets are stripped in `refine_url_range` afterwards.
const URL_PATTERN: &str =
    r#"(?i)((https?|ftps?|ssh|git)://|mailto:)[^ \t\x00-\x1f\x7f<>"{}|\\^`\[\]]+"#;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_METADATA_REFRESH_INTERVAL: Duration = Duration::from_millis(200);
const MAX_SNAPSHOT_WAIT: Duration = Duration::from_secs(1);
const MAX_INPUT_BATCH_MESSAGES: usize = 8;
const TERMINAL_FRAME_INTERVAL: Duration = Duration::from_millis(8);
const PROGRESS_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const MAX_WIRE_MESSAGE_SIZE: usize = 64 * 1024 * 1024;
const MAX_IMAGE_CHUNK_SIZE: usize = 16 * 1024 * 1024;
const PUBLISHED_SNAPSHOT_HISTORY: usize = 16;
const OSC_EVENT_HISTORY: usize = 256;
const MAX_PENDING_NOTIFICATIONS: usize = 64;
const MAX_ITERM2_TRANSFER_BYTES: u64 = 512 * 1024 * 1024;
const COMMAND_HISTORY: usize = 1_000;
/// Scrollback capacity (lines) configured on every terminal. Also the eviction window for the
/// jump-to-prompt index.
const TERMINAL_SCROLLBACK_LIMIT: usize = 10_000;
const MAX_RICH_CLIPBOARD_BYTES: usize = 64 * 1024 * 1024;
const KITTY_CLIPBOARD_CHUNK_BYTES: usize = 4_096;
const PASTE_CLIPBOARD_GRANT_TTL: Duration = Duration::from_secs(30);
/// Minimum spacing between forwarded bell events; a burst of `\a` within this window collapses to
/// one, protecting the bounded OSC queue and preventing UI flash thrash.
const BELL_THROTTLE: Duration = Duration::from_millis(100);
const RAW_TERMINAL_IMAGE_WIRE_FLAG: u32 = 1 << 31;
const RAW_TERMINAL_IMAGE_METADATA_SIZE: usize = 28;

#[derive(Clone, Copy)]
struct GridSize(TerminalSize);

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        self.0.rows.max(1) as usize
    }

    fn columns(&self) -> usize {
        self.0.columns.max(2) as usize
    }
}

fn window_size(size: TerminalSize) -> WindowSize {
    WindowSize {
        num_lines: size.rows.max(1),
        num_cols: size.columns.max(2),
        cell_width: size.cell_width.max(1),
        cell_height: size.cell_height.max(1),
    }
}

struct ListenerState {
    session_id: SessionId,
    title: RwLock<String>,
    size: RwLock<TerminalSize>,
    status: RwLock<SessionStatus>,
    revision: AtomicU64,
    revision_wait: Mutex<()>,
    revision_changed: Condvar,
    sender: Mutex<Option<EventLoopSender>>,
    appearance: RwLock<TerminalAppearance>,
    last_input_sequence: Arc<AtomicU64>,
    published_snapshots: RwLock<VecDeque<Arc<TerminalSnapshot>>>,
    published_images: RwLock<HashMap<TerminalImageKey, PublishedTerminalImage>>,
    progress: ProgressTracker,
    reported_location: RwLock<Option<TerminalReportedLocation>>,
    shell_integration: Mutex<ShellIntegrationTracker>,
    osc_events: OscEventTracker,
    next_osc_request_id: AtomicU64,
    pending_clipboard_reads: Mutex<HashMap<u64, ClipboardReadResponder>>,
    paste_clipboard_grants: Mutex<HashMap<String, PasteClipboardGrant>>,
    notification_replies: Mutex<HashMap<String, NotificationReply>>,
    live_notifications: Mutex<HashSet<String>>,
    pending_notifications: Mutex<HashMap<String, PendingNotification>>,
    rich_clipboard_write: Mutex<Option<RichClipboardWrite>>,
    pending_file_transfers: Mutex<HashMap<u64, PendingFileTransfer>>,
    kitty_file_transfers: Mutex<HashMap<String, KittyIncomingTransfer>>,
    iterm2_multipart: Mutex<Option<Iterm2MultipartTransfer>>,
    allow_clipboard_read: AtomicBool,
    /// Whether bare-URL auto-detection is enabled for this session (client-configurable, default on).
    detect_urls: AtomicBool,
    /// Lazily-compiled regex for auto-detecting bare URLs in terminal output. `None` = not yet
    /// built; `Some(None)` = compilation failed (detection permanently disabled); `Some(Some(_))` =
    /// the compiled DFA, reused across every publish so the DFA is never rebuilt per frame.
    url_regex: Mutex<Option<Option<RegexSearch>>>,
    /// Timestamp of the last bell we forwarded, used to throttle a burst of `\a` so a runaway
    /// program cannot flood the bounded OSC event queue or thrash the UI flash animation.
    last_bell: Mutex<Option<Instant>>,
}

enum ClipboardReadResponder {
    Osc52(Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
    Kitty {
        id: String,
        terminator: String,
        selection: TerminalClipboardSelection,
        list_only: bool,
    },
}

struct PasteClipboardGrant {
    selection: TerminalClipboardSelection,
    contents: Vec<TerminalClipboardContent>,
    expires_at: Instant,
}

struct RichClipboardWrite {
    id: String,
    selection: TerminalClipboardSelection,
    contents: Vec<TerminalClipboardContent>,
    aliases: Vec<(String, String)>,
    total_size: usize,
    terminator: String,
}

enum PendingFileTransfer {
    Iterm2 { temp_path: PathBuf },
    Kitty { transfer_id: String },
}

struct KittyIncomingTransfer {
    destination: Option<PathBuf>,
    terminator: String,
    files: HashMap<String, KittyIncomingFile>,
}

struct KittyIncomingFile {
    path: PathBuf,
    sink: Option<KittyIncomingSink>,
    size: u64,
    expected_size: Option<u64>,
    permissions: Option<u32>,
    directory: bool,
}

enum KittyIncomingSink {
    Plain(fs::File),
    Zlib(ZlibDecoder<fs::File>),
}

impl KittyIncomingSink {
    fn write_chunk(&mut self, data: &[u8]) -> std::io::Result<u64> {
        match self {
            Self::Plain(file) => {
                file.write_all(data)?;
                Ok(data.len() as u64)
            }
            Self::Zlib(decoder) => {
                let before = decoder.total_out();
                decoder.write_all(data)?;
                Ok(decoder.total_out().saturating_sub(before))
            }
        }
    }

    fn finish(self) -> std::io::Result<fs::File> {
        match self {
            Self::Plain(mut file) => {
                file.flush()?;
                Ok(file)
            }
            Self::Zlib(decoder) => decoder.finish(),
        }
    }
}

struct Iterm2MultipartTransfer {
    request_id: u64,
    suggested_name: String,
    temp_path: PathBuf,
    file: fs::File,
    size: u64,
}

#[derive(Clone)]
struct NotificationReply {
    protocol_id: String,
    terminator: String,
    activation_response_requested: bool,
    close_response_requested: bool,
}

#[derive(Clone, Default)]
struct PendingNotification {
    title: String,
    body: String,
    activation_response_requested: bool,
    close_response_requested: bool,
}

struct ReadyNotification {
    id: String,
    protocol_id: String,
    title: String,
    body: String,
    activation_response_requested: bool,
    close_response_requested: bool,
    terminator: String,
}

struct OscEventTracker {
    session_id: SessionId,
    state: Mutex<OscEventTrackerState>,
    changed: Condvar,
}

struct OscEventTrackerState {
    revision: u64,
    events: VecDeque<TerminalOscEvent>,
}

impl OscEventTracker {
    fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: Mutex::new(OscEventTrackerState {
                revision: 0,
                events: VecDeque::with_capacity(OSC_EVENT_HISTORY),
            }),
            changed: Condvar::new(),
        }
    }

    fn push(&self, payload: TerminalOscEventPayload) {
        let mut state = self.state.lock();
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        state
            .events
            .push_back(TerminalOscEvent { revision, payload });
        if state.events.len() > OSC_EVENT_HISTORY {
            state.events.pop_front();
        }
        self.changed.notify_all();
    }

    fn wait_after(&self, after_revision: u64, timeout: Duration) -> Option<TerminalOscEventUpdate> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        while state.revision <= after_revision {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            self.changed.wait_for(&mut state, deadline - now);
        }
        let revision = state.revision;
        let events = state
            .events
            .iter()
            .filter(|event| event.revision > after_revision)
            .cloned()
            .collect();
        Some(TerminalOscEventUpdate {
            session_id: self.session_id,
            revision,
            events,
        })
    }

    fn revision(&self) -> u64 {
        self.state.lock().revision
    }
}

/// Direction for jump-to-prompt navigation (daemon-internal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalJumpDirection {
    /// Toward older prompts (further up in scrollback).
    Up,
    /// Toward newer prompts (further down toward the live bottom).
    Down,
}

struct ShellIntegrationTracker {
    phase: TerminalSemanticPhase,
    /// Grid line where the active prompt started (relative to the top of the visible screen, as
    /// reported with the OSC 133 A/prompt-start marker). Used to clear exactly the prompt region on
    /// resize. `None` when not currently on a prompt.
    prompt_start_line: Option<i32>,
    current_command: Option<TerminalCommandRecord>,
    history: VecDeque<TerminalCommandRecord>,
    next_command_id: u64,
    user_variables: HashMap<String, String>,
    /// Monotonic count of lines that have scrolled off the top of the active screen since the
    /// session started. Kept fresh each frame by [`ShellIntegrationTracker::observe_scroll`]. Used
    /// as a stable coordinate base: a prompt captured at active-screen line `cursor_line` is stored
    /// as `total_scrolled_lines + cursor_line`, which never shifts as scrollback grows.
    total_scrolled_lines: u64,
    /// The `history_size` seen at the previous `observe_scroll`, to derive scroll increments after
    /// the scrollback saturates (once at the limit, `history_size` stops growing).
    last_history_size: usize,
    /// Global line coordinates (see `total_scrolled_lines`) of each recorded prompt start, oldest
    /// first. This is the jump-to-prompt index. Bounded by `COMMAND_HISTORY` and pruned as
    /// scrollback evicts old lines.
    prompt_jump_points: VecDeque<u64>,
}

impl Default for ShellIntegrationTracker {
    fn default() -> Self {
        Self {
            phase: TerminalSemanticPhase::None,
            prompt_start_line: None,
            current_command: None,
            history: VecDeque::with_capacity(COMMAND_HISTORY),
            next_command_id: 1,
            user_variables: HashMap::new(),
            total_scrolled_lines: 0,
            last_history_size: 0,
            prompt_jump_points: VecDeque::new(),
        }
    }
}

impl ShellIntegrationTracker {
    /// Update the tracker from an OSC 133 marker. `cursor_line` is the grid line the cursor was on
    /// when the marker was emitted (relative to the top of the visible screen), used to record
    /// where the active prompt starts. `history_size` is the scrollback depth captured at the same
    /// instant (synchronously, under the terminal lock) — folding it in here keeps the jump-point
    /// coordinate accurate even when the throttled `observe_scroll` has not caught up with a burst
    /// of output.
    fn update(&mut self, prompt: SemanticPrompt, cursor_line: i32, history_size: usize) {
        let now = unix_time_ms();
        // Fold the freshly-captured scrollback depth into the coordinate base before recording any
        // jump point, so `record_prompt_jump_point` uses an up-to-date `total_scrolled_lines`.
        self.advance_scroll_base(history_size);
        match prompt.action {
            SemanticPromptAction::FreshLine => {}
            SemanticPromptAction::FreshLineAndPrompt | SemanticPromptAction::PromptStart => {
                // A/prompt-start marks where the prompt begins. Record it only when entering the
                // prompt from a non-prompt phase, so continuation/re-emitted marks on the same
                // prompt (e.g. our zle-line-init hook firing on every redraw) don't move the start
                // down to the input line.
                if !matches!(
                    self.phase,
                    TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
                ) {
                    self.prompt_start_line = Some(cursor_line);
                    self.record_prompt_jump_point(cursor_line);
                }
                self.phase = TerminalSemanticPhase::Prompt;
            }
            SemanticPromptAction::NewCommand => {
                self.begin_command(now, option_value(&prompt.options, "cmdline"));
                self.prompt_start_line = Some(cursor_line);
                self.record_prompt_jump_point(cursor_line);
                self.phase = TerminalSemanticPhase::Prompt;
            }
            SemanticPromptAction::InputStart
            | SemanticPromptAction::InputStartAndTerminatePrompt => {
                if self.current_command.is_none() {
                    self.begin_command(now, option_value(&prompt.options, "cmdline"));
                }
                // Entering input directly (no preceding A) still means the cursor is on a prompt;
                // record the start if we don't have one yet.
                if self.prompt_start_line.is_none() {
                    self.prompt_start_line = Some(cursor_line);
                }
                self.phase = TerminalSemanticPhase::Input;
            }
            SemanticPromptAction::OutputStart => {
                let command_line = option_value(&prompt.options, "cmdline")
                    .or_else(|| option_value(&prompt.options, "cmdline_url").map(percent_decode));
                if self.current_command.is_none() {
                    self.begin_command(now, command_line);
                } else if let Some(command_line) = command_line
                    && let Some(command) = self.current_command.as_mut()
                {
                    command.command_line = Some(command_line);
                }
                // Output has started; the prompt is no longer the active region.
                self.prompt_start_line = None;
                self.phase = TerminalSemanticPhase::Output;
            }
            SemanticPromptAction::CommandFinished => {
                if !matches!(
                    self.phase,
                    TerminalSemanticPhase::Input | TerminalSemanticPhase::Output
                ) {
                    return;
                }
                let exit_code = prompt
                    .options
                    .split(';')
                    .next()
                    .filter(|value| !value.is_empty())
                    .and_then(|value| value.parse::<i32>().ok());
                if self.phase == TerminalSemanticPhase::Input {
                    self.current_command = None;
                } else if let Some(mut command) = self.current_command.take() {
                    command.completed_at_unix_ms = Some(now);
                    command.exit_code = exit_code.filter(|code| (0..=255).contains(code));
                    self.history.push_back(command);
                    if self.history.len() > COMMAND_HISTORY {
                        self.history.pop_front();
                    }
                }
                self.prompt_start_line = None;
                self.phase = TerminalSemanticPhase::None;
            }
        }
    }

    fn begin_command(&mut self, now: u64, command_line: Option<String>) {
        if let Some(mut previous) = self.current_command.take() {
            previous.completed_at_unix_ms = Some(now);
            self.history.push_back(previous);
        }
        self.current_command = Some(TerminalCommandRecord {
            id: self.next_command_id,
            command_line,
            started_at_unix_ms: now,
            completed_at_unix_ms: None,
            exit_code: None,
        });
        self.next_command_id = self.next_command_id.wrapping_add(1).max(1);
        while self.history.len() > COMMAND_HISTORY {
            self.history.pop_front();
        }
    }

    /// Record the global line coordinate of a prompt start for jump-to-prompt. De-dupes a repeat of
    /// the current prompt (same global line) rather than appending a second jump point.
    fn record_prompt_jump_point(&mut self, cursor_line: i32) {
        let global = self.total_scrolled_lines as i64 + cursor_line as i64;
        if global < 0 {
            return;
        }
        let global = global as u64;
        if self.prompt_jump_points.back() == Some(&global) {
            return;
        }
        self.prompt_jump_points.push_back(global);
        while self.prompt_jump_points.len() > COMMAND_HISTORY {
            self.prompt_jump_points.pop_front();
        }
    }

    /// Advance the scroll-base (`total_scrolled_lines`) toward the given live `history_size`.
    /// Shared by the OSC-133 capture path (accurate at marker time) and the per-frame
    /// `observe_scroll`. While unsaturated, `history_size` equals the lines scrolled off the top, so
    /// the base tracks it exactly; after saturation `history_size` pins to the limit and this
    /// advances by the observed increase (best effort). A shrink (clear/reset) rebases downward.
    fn advance_scroll_base(&mut self, history_size: usize) {
        if history_size >= self.last_history_size {
            self.total_scrolled_lines += (history_size - self.last_history_size) as u64;
        } else {
            self.total_scrolled_lines = self
                .total_scrolled_lines
                .saturating_sub((self.last_history_size - history_size) as u64);
        }
        self.last_history_size = history_size;
    }

    /// Keep `total_scrolled_lines` current and prune jump points that have scrolled out of the
    /// buffer. Called once per published frame with the terminal's live `history_size`.
    fn observe_scroll(&mut self, history_size: usize, scrollback_limit: usize) {
        self.advance_scroll_base(history_size);
        let _ = scrollback_limit;
        // The oldest visible-or-scrollback line has global coordinate `total_scrolled - history`.
        // Anything older has been evicted from the buffer and can never be jumped to again.
        let oldest_live = self.total_scrolled_lines.saturating_sub(history_size as u64);
        while self
            .prompt_jump_points
            .front()
            .is_some_and(|&global| global < oldest_live)
        {
            self.prompt_jump_points.pop_front();
        }
    }

    /// Reset the jump-point index and its coordinate base. Used when a column resize reflows the
    /// buffer, invalidating every stored line coordinate.
    fn clear_jump_points(&mut self) {
        self.prompt_jump_points.clear();
        self.total_scrolled_lines = 0;
        self.last_history_size = 0;
    }

    /// Pick the jump target relative to the line currently at the top of the viewport.
    /// `viewport_top_global` is that line's global coordinate. Returns the target's global
    /// coordinate, or `None` when there is no strictly-earlier / strictly-later prompt.
    fn jump_target(&self, viewport_top_global: i64, direction: TerminalJumpDirection) -> Option<u64> {
        match direction {
            TerminalJumpDirection::Up => self
                .prompt_jump_points
                .iter()
                .rev()
                .find(|&&global| (global as i64) < viewport_top_global)
                .copied(),
            TerminalJumpDirection::Down => self
                .prompt_jump_points
                .iter()
                .find(|&&global| (global as i64) > viewport_top_global)
                .copied(),
        }
    }

    fn snapshot(&self) -> TerminalShellIntegrationState {
        let mut user_variables = self
            .user_variables
            .iter()
            .map(|(name, value)| TerminalUserVariable {
                name: name.clone(),
                value: value.clone(),
            })
            .collect::<Vec<_>>();
        user_variables.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        TerminalShellIntegrationState {
            phase: self.phase,
            current_command: self.current_command.clone(),
            history: self.history.iter().cloned().collect(),
            user_variables,
        }
    }
}

struct ProgressTracker {
    session_id: SessionId,
    state: Mutex<ProgressTrackerState>,
    changed: Condvar,
}

struct ProgressTrackerState {
    authoritative: Option<TerminalProgress>,
    authoritative_updated_at: Option<Instant>,
    published: Option<TerminalProgress>,
    revision: u64,
    dirty: bool,
    last_publish_at: Option<Instant>,
    timeouts: TerminalProgressTimeouts,
}

impl ProgressTracker {
    fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            state: Mutex::new(ProgressTrackerState {
                authoritative: None,
                authoritative_updated_at: None,
                published: None,
                revision: 0,
                dirty: false,
                last_publish_at: None,
                timeouts: TerminalProgressTimeouts::default(),
            }),
            changed: Condvar::new(),
        }
    }

    fn report(&self, report: Option<alacritty_terminal::vte::ansi::ProgressReport>) {
        let now = Instant::now();
        let mut state = self.state.lock();
        let progress = report.map(|report| TerminalProgress {
            state: match report.state {
                VteProgressState::Normal => TerminalProgressState::Normal,
                VteProgressState::Error => TerminalProgressState::Error,
                VteProgressState::Indeterminate => TerminalProgressState::Indeterminate,
                VteProgressState::Paused => TerminalProgressState::Paused,
            },
            percent: report.percent,
            updated_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        });

        // A clear while already clear is intentionally a no-op. RIS during startup should not
        // manufacture a progress revision, while every active report refreshes the idle timeout.
        if progress.is_none() && state.authoritative.is_none() {
            return;
        }
        state.authoritative = progress;
        state.authoritative_updated_at = progress.map(|_| now);
        state.dirty = true;
        self.changed.notify_all();
    }

    fn set_timeouts(&self, timeouts: TerminalProgressTimeouts) {
        let mut state = self.state.lock();
        state.timeouts = TerminalProgressTimeouts {
            completed_ms: timeouts.completed_ms.max(100),
            stale_ms: timeouts.stale_ms.max(100),
        };
        self.changed.notify_all();
    }

    fn wait_after(&self, after_revision: u64, timeout: Duration) -> Option<TerminalProgressUpdate> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock();
        loop {
            let now = Instant::now();
            Self::expire_if_needed(&mut state, now);

            if state.revision > after_revision {
                return Some(TerminalProgressUpdate {
                    session_id: self.session_id,
                    revision: state.revision,
                    progress: state.published,
                });
            }

            let next_publish = state
                .last_publish_at
                .map_or(now, |last| last + PROGRESS_FRAME_INTERVAL);
            if state.dirty && now >= next_publish {
                state.published = state.authoritative;
                state.revision = state.revision.wrapping_add(1).max(1);
                state.dirty = false;
                state.last_publish_at = Some(now);
                self.changed.notify_all();
                return Some(TerminalProgressUpdate {
                    session_id: self.session_id,
                    revision: state.revision,
                    progress: state.published,
                });
            }

            if now >= deadline {
                return None;
            }
            let wake_at = Self::expiry_at(&state)
                .into_iter()
                .chain(state.dirty.then_some(next_publish))
                .chain(Some(deadline))
                .min()
                .unwrap_or(deadline);
            self.changed
                .wait_for(&mut state, wake_at.saturating_duration_since(now));
        }
    }

    fn revision(&self) -> u64 {
        self.state.lock().revision
    }

    fn expiry_at(state: &ProgressTrackerState) -> Option<Instant> {
        let progress = state.authoritative?;
        let updated_at = state.authoritative_updated_at?;
        let timeout_ms =
            if progress.state == TerminalProgressState::Normal && progress.percent == Some(100) {
                state.timeouts.completed_ms
            } else {
                state.timeouts.stale_ms
            };
        Some(updated_at + Duration::from_millis(u64::from(timeout_ms)))
    }

    fn expire_if_needed(state: &mut ProgressTrackerState, now: Instant) {
        if Self::expiry_at(state).is_some_and(|expires| now >= expires) {
            state.authoritative = None;
            state.authoritative_updated_at = None;
            state.dirty = true;
        }
    }
}

struct PublishedTerminalImage {
    width: u32,
    height: u32,
    pixels: Arc<PixelBuffer>,
}

struct PublishedTerminalImageChunk {
    key: TerminalImageKey,
    width: u32,
    height: u32,
    total_length: u32,
    offset: u32,
    pixels: Arc<PixelBuffer>,
    end: usize,
}

impl PublishedTerminalImageChunk {
    fn bytes(&self) -> &[u8] {
        &self.pixels[self.offset as usize..self.end]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalImageChunkMetadata {
    pub key: TerminalImageKey,
    pub width: u32,
    pub height: u32,
    pub total_length: u32,
    pub offset: u32,
    pub chunk_length: u32,
}

impl ListenerState {
    fn new(
        session_id: SessionId,
        size: TerminalSize,
        appearance: TerminalAppearance,
        last_input_sequence: Arc<AtomicU64>,
    ) -> Self {
        Self {
            session_id,
            title: RwLock::new("shell".to_owned()),
            size: RwLock::new(size),
            status: RwLock::new(SessionStatus::Running),
            revision: AtomicU64::new(0),
            revision_wait: Mutex::new(()),
            revision_changed: Condvar::new(),
            sender: Mutex::new(None),
            appearance: RwLock::new(appearance),
            last_input_sequence,
            published_snapshots: RwLock::new(VecDeque::with_capacity(PUBLISHED_SNAPSHOT_HISTORY)),
            published_images: RwLock::new(HashMap::new()),
            progress: ProgressTracker::new(session_id),
            reported_location: RwLock::new(None),
            shell_integration: Mutex::new(ShellIntegrationTracker::default()),
            osc_events: OscEventTracker::new(session_id),
            next_osc_request_id: AtomicU64::new(1),
            pending_clipboard_reads: Mutex::new(HashMap::new()),
            paste_clipboard_grants: Mutex::new(HashMap::new()),
            notification_replies: Mutex::new(HashMap::new()),
            live_notifications: Mutex::new(HashSet::new()),
            pending_notifications: Mutex::new(HashMap::new()),
            rich_clipboard_write: Mutex::new(None),
            pending_file_transfers: Mutex::new(HashMap::new()),
            kitty_file_transfers: Mutex::new(HashMap::new()),
            iterm2_multipart: Mutex::new(None),
            allow_clipboard_read: AtomicBool::new(false),
            detect_urls: AtomicBool::new(true),
            url_regex: Mutex::new(None),
            last_bell: Mutex::new(None),
        }
    }

    fn publish_terminal(&self, terminal: &Term<DaemonEventListener>) {
        // Every caller already owns the terminal mutex. Build the immutable frame under that
        // existing ownership and only take the snapshot lock for the final Arc swap.
        let started = Instant::now();
        // Keep the jump-to-prompt coordinate base current with the live scrollback depth.
        self.shell_integration
            .lock()
            .observe_scroll(terminal.grid().history_size(), TERMINAL_SCROLLBACK_LIMIT);
        let revision = self.revision.load(Ordering::Relaxed).wrapping_add(1);
        let mut snapshot = snapshot_terminal(
            terminal,
            self.session_id,
            *self.size.read(),
            self.title.read().clone(),
            revision,
            self.last_input_sequence.load(Ordering::Acquire),
        );
        self.detect_urls_into(&mut snapshot, terminal);
        let snapshot = Arc::new(snapshot);
        let cell_count = snapshot.cells.len();
        // A snapshot and the image generations referenced by it must have the same lifetime.
        // Kitty animations can advance again before the UI has fetched the previous frame; asking
        // the live terminal for that old generation races and produces visible missing frames.
        // Retain immutable pixel Arcs alongside the bounded snapshot history so image fetches are
        // coherent without holding the terminal lock or copying image data during publication.
        let snapshot_images = snapshot
            .images
            .iter()
            .filter_map(|image| {
                terminal
                    .kitty_graphics_image_with_metadata(KittyImageKey {
                        id: image.key.id,
                        generation: image.key.generation,
                    })
                    .map(|(descriptor, pixels)| {
                        (
                            image.key,
                            PublishedTerminalImage {
                                width: descriptor.width,
                                height: descriptor.height,
                                pixels,
                            },
                        )
                    })
            })
            .collect::<Vec<_>>();
        let mut published = self.published_snapshots.write();
        published.push_back(snapshot);
        if published.len() > PUBLISHED_SNAPSHOT_HISTORY {
            published.pop_front();
        }
        let live_images = published
            .iter()
            .flat_map(|snapshot| snapshot.images.iter().map(|image| image.key))
            .collect::<HashSet<_>>();
        drop(published);
        let mut images = self.published_images.write();
        images.extend(snapshot_images);
        images.retain(|key, _| live_images.contains(key));
        drop(images);
        self.revision.store(revision, Ordering::Release);
        self.revision_changed.notify_all();
        if render_metrics_enabled() && revision.is_multiple_of(30) {
            eprintln!(
                "[eggie-render-metrics] publish revision={revision} cells={} elapsed_ms={:.3}",
                cell_count,
                started.elapsed().as_secs_f64() * 1_000.,
            );
        }
    }

    /// Detect bare URLs in the current viewport and fill `snapshot.detected_links`. Runs under the
    /// terminal lock (the caller owns it). Cheap when no URL is present: a substring precheck over
    /// the already-built snapshot cells short-circuits before touching the regex engine.
    fn detect_urls_into(
        &self,
        snapshot: &mut TerminalSnapshot,
        terminal: &Term<DaemonEventListener>,
    ) {
        if !self.detect_urls.load(Ordering::Acquire) {
            return;
        }
        // Precheck: only scan when a scheme separator is visible. This keeps high-throughput output
        // (which rarely contains URLs) from paying for a regex pass every frame.
        let has_candidate = snapshot.plain_lines().iter().any(|line| {
            line.contains("://") || line.contains("mailto:")
        });
        if !has_candidate {
            return;
        }

        let mut guard = self.url_regex.lock();
        let regex = guard.get_or_insert_with(|| RegexSearch::new(URL_PATTERN).ok());
        let Some(regex) = regex.as_mut() else {
            return;
        };

        let display_offset = terminal.grid().display_offset();
        let screen_lines = terminal.grid().screen_lines();
        let columns = terminal.grid().columns();
        snapshot.detected_links =
            detect_viewport_urls(terminal, regex, display_offset, screen_lines, columns);
    }

    fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.published_snapshots
            .read()
            .back()
            .expect("terminal publishes its initial frame before becoming visible")
            .clone()
    }

    fn snapshot_after(&self, after_revision: u64) -> TerminalSnapshotUpdate {
        let published = self.published_snapshots.read();
        let current = published
            .back()
            .expect("terminal publishes its initial frame before becoming visible")
            .clone();
        let Some(base) = published
            .iter()
            .find(|snapshot| snapshot.revision == after_revision)
            .cloned()
        else {
            return TerminalSnapshotUpdate::Full(current);
        };
        drop(published);
        snapshot_delta(&base, &current).map_or(
            TerminalSnapshotUpdate::Full(current),
            TerminalSnapshotUpdate::Delta,
        )
    }

    #[cfg(test)]
    fn signal_revision_for_test(&self) {
        self.revision.fetch_add(1, Ordering::Release);
        self.revision_changed.notify_all();
    }

    fn wait_for_revision(&self, after_revision: u64, timeout: Duration) -> bool {
        if self.revision.load(Ordering::Acquire) > after_revision {
            return true;
        }
        let deadline = Instant::now() + timeout;
        let mut guard = self.revision_wait.lock();
        loop {
            if self.revision.load(Ordering::Acquire) > after_revision {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            if self
                .revision_changed
                .wait_for(&mut guard, deadline - now)
                .timed_out()
            {
                return self.revision.load(Ordering::Acquire) > after_revision;
            }
        }
    }

    fn write_to_pty(&self, bytes: impl Into<Vec<u8>>) {
        let Some(sender) = self.sender.lock().clone() else {
            return;
        };
        let _ = sender.send(Msg::Input(bytes.into().into()));
    }

    fn report_working_directory(&self, value: &str) {
        let Some(mut location) = parse_reported_location(value) else {
            return;
        };
        let mut reported = self.reported_location.write();
        if !value.starts_with("file://")
            && let Some(previous) = reported.as_ref()
            && previous.host.is_some()
        {
            location.user.clone_from(&previous.user);
            location.host.clone_from(&previous.host);
            location.local = previous.local;
        }
        *reported = Some(location);
    }

    /// Forward a terminal bell to the UI as an OSC event, throttled so a burst of `\a` collapses to
    /// one event per [`BELL_THROTTLE`] window. Returns whether the bell was actually forwarded.
    fn ring_bell(&self) -> bool {
        let now = Instant::now();
        let mut last = self.last_bell.lock();
        if let Some(previous) = *last
            && now.duration_since(previous) < BELL_THROTTLE
        {
            return false;
        }
        *last = Some(now);
        drop(last);
        self.osc_events.push(TerminalOscEventPayload::Bell);
        true
    }

    fn update_remote_host(&self, value: &str) {
        let (user, host) = value
            .split_once('@')
            .map_or((None, value), |(user, host)| (Some(user.to_owned()), host));
        if host.is_empty() {
            return;
        }
        let mut reported = self.reported_location.write();
        let path = reported
            .as_ref()
            .map_or_else(String::new, |location| location.path.clone());
        *reported = Some(TerminalReportedLocation {
            user,
            host: Some(host.to_owned()),
            path,
            local: host_is_local(host),
        });
    }

    fn clipboard_store(&self, clipboard: ClipboardType, text: String) {
        self.osc_events
            .push(TerminalOscEventPayload::ClipboardWrite {
                selection: clipboard_selection(clipboard),
                contents: vec![TerminalClipboardContent {
                    mime_type: "text/plain;charset=utf-8".to_owned(),
                    data: text.into_bytes(),
                }],
            });
    }

    fn clipboard_load(
        &self,
        clipboard: ClipboardType,
        formatter: Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ) {
        if !self.allow_clipboard_read.load(Ordering::Acquire) {
            return;
        }
        let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
        self.pending_clipboard_reads
            .lock()
            .insert(request_id, ClipboardReadResponder::Osc52(formatter));
        self.osc_events
            .push(TerminalOscEventPayload::ClipboardRead {
                request_id,
                selection: clipboard_selection(clipboard),
                mime_types: vec!["text/plain;charset=utf-8".to_owned()],
            });
    }

    fn complete_clipboard_read(&self, request_id: u64, contents: Vec<TerminalClipboardContent>) {
        let Some(responder) = self.pending_clipboard_reads.lock().remove(&request_id) else {
            return;
        };
        match responder {
            ClipboardReadResponder::Osc52(formatter) => {
                let text = contents
                    .iter()
                    .find(|content| content.mime_type.starts_with("text/plain"))
                    .and_then(|content| String::from_utf8(content.data.clone()).ok())
                    .unwrap_or_default();
                self.write_to_pty(formatter(&text).into_bytes());
            }
            ClipboardReadResponder::Kitty {
                id,
                terminator,
                selection,
                list_only,
            } => self.send_kitty_clipboard_read_response(
                &id,
                selection,
                list_only,
                &contents,
                &terminator,
            ),
        }
    }

    fn send_kitty_clipboard_read_response(
        &self,
        id: &str,
        selection: TerminalClipboardSelection,
        list_only: bool,
        contents: &[TerminalClipboardContent],
        terminator: &str,
    ) {
        let id = protocol_id_field(id);
        let location = if selection == TerminalClipboardSelection::Primary {
            ":loc=primary"
        } else {
            ""
        };
        if !list_only && contents.is_empty() {
            self.write_to_pty(
                format!("\x1b]5522;type=read:status=ENOSYS{location}{id}{terminator}").into_bytes(),
            );
            return;
        }
        self.write_to_pty(
            format!("\x1b]5522;type=read:status=OK{location}{id}{terminator}").into_bytes(),
        );
        let mut seen_mime_types = HashSet::new();
        for content in contents {
            if !seen_mime_types.insert(content.mime_type.as_str()) {
                continue;
            }
            let mime = BASE64.encode(content.mime_type.as_bytes());
            if list_only || content.data.is_empty() {
                self.write_to_pty(
                    format!("\x1b]5522;type=read:status=DATA{id}:mime={mime}{terminator}")
                        .into_bytes(),
                );
                continue;
            }
            for chunk in content.data.chunks(KITTY_CLIPBOARD_CHUNK_BYTES) {
                let payload = BASE64.encode(chunk);
                self.write_to_pty(
                    format!(
                        "\x1b]5522;type=read:status=DATA{id}:mime={mime};{payload}{terminator}"
                    )
                    .into_bytes(),
                );
            }
        }
        self.write_to_pty(format!("\x1b]5522;type=read:status=DONE{id}{terminator}").into_bytes());
    }

    fn paste_clipboard(
        &self,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
    ) {
        let now = Instant::now();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut grants = self.paste_clipboard_grants.lock();
        grants.retain(|_, grant| grant.expires_at > now);
        while grants.len() >= 16 {
            let Some(oldest) = grants
                .iter()
                .min_by_key(|(_, grant)| grant.expires_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            grants.remove(&oldest);
        }
        grants.insert(
            token.clone(),
            PasteClipboardGrant {
                selection,
                contents: contents.clone(),
                expires_at: now + PASTE_CLIPBOARD_GRANT_TTL,
            },
        );
        drop(grants);

        let location = if selection == TerminalClipboardSelection::Primary {
            ":loc=primary"
        } else {
            ""
        };
        self.write_to_pty(
            format!(
                "\x1b]5522;type=read:status=OK{location}:pw={}\x1b\\",
                BASE64.encode(token.as_bytes())
            )
            .into_bytes(),
        );
        let mut seen = HashSet::new();
        for content in contents {
            if seen.insert(content.mime_type.clone()) {
                self.write_to_pty(
                    format!(
                        "\x1b]5522;type=read:status=DATA:mime={}\x1b\\",
                        BASE64.encode(content.mime_type.as_bytes())
                    )
                    .into_bytes(),
                );
            }
        }
        self.write_to_pty(b"\x1b]5522;type=read:status=DONE\x1b\\".to_vec());
    }

    fn handle_notification(
        &self,
        notification: alacritty_terminal::vte::ansi::DesktopNotification,
    ) {
        match notification.code {
            9 => self.publish_notification(ReadyNotification {
                id: generated_notification_id(self.session_id, &self.next_osc_request_id),
                protocol_id: "0".to_owned(),
                title: notification.payload,
                body: String::new(),
                activation_response_requested: false,
                close_response_requested: false,
                terminator: notification.terminator,
            }),
            777 => {
                let Some(payload) = notification.payload.strip_prefix("notify;") else {
                    return;
                };
                let (title, body) = payload.split_once(';').unwrap_or((payload, ""));
                self.publish_notification(ReadyNotification {
                    id: generated_notification_id(self.session_id, &self.next_osc_request_id),
                    protocol_id: "0".to_owned(),
                    title: title.to_owned(),
                    body: body.to_owned(),
                    activation_response_requested: false,
                    close_response_requested: false,
                    terminator: notification.terminator,
                });
            }
            99 => self.handle_kitty_notification(&notification.payload, notification.terminator),
            _ => {}
        }
    }

    fn handle_kitty_notification(&self, payload: &str, terminator: String) {
        let (metadata, payload) = payload.split_once(';').unwrap_or((payload, ""));
        let fields = colon_metadata(metadata);
        let explicit_id = fields
            .get("i")
            .filter(|id| !id.is_empty())
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty());
        let protocol_id = explicit_id.clone().unwrap_or_else(|| "0".to_owned());
        let id = explicit_id.clone().unwrap_or_else(|| {
            generated_notification_id(self.session_id, &self.next_osc_request_id)
        });
        let payload_type = fields.get("p").map_or("title", String::as_str);
        if payload_type == "?" {
            self.write_to_pty(
                format!(
                    "\x1b]99;i={protocol_id}:p=?;a=focus,report:o=unfocused,invisible:p=title,body,?,close,alive:c=1{terminator}"
                )
                .into_bytes(),
            );
            return;
        }
        if payload_type == "close" {
            let Some(id) = explicit_id else {
                return;
            };
            self.osc_events
                .push(TerminalOscEventPayload::NotificationClose { id: id.clone() });
            self.pending_notifications.lock().remove(&id);
            self.notification_replies.lock().remove(&id);
            self.live_notifications.lock().remove(&id);
            return;
        }
        if payload_type == "alive" {
            let mut live = self
                .live_notifications
                .lock()
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            live.sort_unstable();
            self.write_to_pty(
                format!(
                    "\x1b]99;i={protocol_id}:p=alive;{}{terminator}",
                    live.join(",")
                )
                .into_bytes(),
            );
            return;
        }

        let decoded = if fields.get("e").is_some_and(|value| value == "1") {
            BASE64
                .decode(payload)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default()
        } else {
            payload.to_owned()
        };
        let done = fields.get("d").is_none_or(|value| value != "0");
        let actions = fields.get("a").map_or("focus", String::as_str);
        let activation_response_requested = actions.split(',').any(|action| action == "report");
        let close_response_requested = fields.get("c").is_some_and(|value| value != "0");
        let mut pending = self.pending_notifications.lock();
        if !pending.contains_key(&id)
            && pending.len() >= MAX_PENDING_NOTIFICATIONS
            && let Some(expired) = pending.keys().next().cloned()
        {
            pending.remove(&expired);
        }
        let command = pending.entry(id.clone()).or_default();
        match payload_type {
            "body" => append_sanitized_osc_text(&mut command.body, &decoded, 4_096),
            "title" | "" => append_sanitized_osc_text(&mut command.title, &decoded, 1_024),
            _ => {}
        }
        command.activation_response_requested |= activation_response_requested;
        command.close_response_requested |= close_response_requested;
        if !done {
            return;
        }
        let command = pending.remove(&id).unwrap_or_default();
        drop(pending);
        self.publish_notification(ReadyNotification {
            id,
            protocol_id,
            title: command.title,
            body: command.body,
            activation_response_requested: command.activation_response_requested,
            close_response_requested: command.close_response_requested,
            terminator,
        });
    }

    fn publish_notification(&self, notification: ReadyNotification) {
        let ReadyNotification {
            id,
            protocol_id,
            mut title,
            mut body,
            activation_response_requested,
            close_response_requested,
            terminator,
        } = notification;
        title = sanitize_osc_text(&title, 1_024);
        body = sanitize_osc_text(&body, 4_096);
        if title.is_empty() {
            std::mem::swap(&mut title, &mut body);
        }
        if title.is_empty() {
            return;
        }
        if protocol_id != "0" {
            let mut live = self.live_notifications.lock();
            if !live.contains(&protocol_id)
                && live.len() >= MAX_PENDING_NOTIFICATIONS
                && let Some(expired) = live.iter().next().cloned()
            {
                live.remove(&expired);
            }
            live.insert(protocol_id.clone());
        }
        let mut close_response_requested = close_response_requested;
        #[cfg(target_os = "macos")]
        if close_response_requested {
            // macOS does not reliably report Notification Center dismissal. Kitty's protocol
            // defines `untracked` specifically for this platform limitation.
            self.write_to_pty(
                format!("\x1b]99;i={protocol_id}:p=close;untracked{terminator}").into_bytes(),
            );
            close_response_requested = false;
        }
        if activation_response_requested || close_response_requested {
            let mut replies = self.notification_replies.lock();
            if !replies.contains_key(&id)
                && replies.len() >= MAX_PENDING_NOTIFICATIONS
                && let Some(expired) = replies.keys().next().cloned()
            {
                replies.remove(&expired);
            }
            replies.insert(
                id.clone(),
                NotificationReply {
                    protocol_id,
                    terminator,
                    activation_response_requested,
                    close_response_requested,
                },
            );
        }
        self.osc_events.push(TerminalOscEventPayload::Notification {
            notification: TerminalNotification {
                id,
                title,
                body,
                activation_response_requested,
                close_response_requested,
            },
        });
    }

    fn notification_response(&self, notification_id: &str, activated: bool) {
        let mut replies = self.notification_replies.lock();
        let Some(reply) = replies.remove(notification_id) else {
            self.live_notifications.lock().remove(notification_id);
            return;
        };
        if activated && reply.activation_response_requested {
            self.write_to_pty(
                format!("\x1b]99;i={};{}", reply.protocol_id, reply.terminator).into_bytes(),
            );
        }
        if reply.close_response_requested {
            self.write_to_pty(
                format!(
                    "\x1b]99;i={}:p=close;{}",
                    reply.protocol_id, reply.terminator
                )
                .into_bytes(),
            );
        }
        self.live_notifications.lock().remove(&reply.protocol_id);
    }

    fn handle_iterm2_command(&self, payload: &str, terminator: &str) {
        if let Some(metadata) = payload.strip_prefix("MultipartFile=") {
            self.start_iterm2_multipart(metadata);
        } else if let Some(encoded) = payload.strip_prefix("FilePart=") {
            self.append_iterm2_multipart(encoded);
        } else if payload == "FileEnd" {
            self.finish_iterm2_multipart();
        } else if payload.starts_with("File=") {
            self.handle_iterm2_file(payload);
        } else if let Some(value) = payload.strip_prefix("RemoteHost=") {
            self.update_remote_host(value);
        } else if let Some(value) = payload.strip_prefix("SetUserVar=") {
            if let Some((name, encoded)) = value.split_once('=')
                && let Ok(decoded) = BASE64.decode(encoded)
                && let Ok(decoded) = String::from_utf8(decoded)
            {
                let name = sanitize_osc_text(name, 256);
                if name.is_empty() {
                    return;
                }
                let mut shell = self.shell_integration.lock();
                if shell.user_variables.len() >= 64
                    && !shell.user_variables.contains_key(&name)
                    && let Some(oldest) = shell.user_variables.keys().next().cloned()
                {
                    shell.user_variables.remove(&oldest);
                }
                shell
                    .user_variables
                    .insert(name, sanitize_osc_text(&decoded, 4_096));
            }
        } else if let Some(encoded) = payload.strip_prefix("ReportVariable=") {
            let name = decode_base64_text(encoded).unwrap_or_default();
            let value = self.iterm2_variable(&name).unwrap_or_default();
            self.write_to_pty(
                format!(
                    "\x1b]1337;ReportVariable={}{}",
                    BASE64.encode(value.as_bytes()),
                    terminator
                )
                .into_bytes(),
            );
        } else if let Some(encoded) = payload.strip_prefix("Copy=:") {
            if let Ok(data) = BASE64.decode(encoded) {
                self.osc_events
                    .push(TerminalOscEventPayload::ClipboardWrite {
                        selection: TerminalClipboardSelection::Clipboard,
                        contents: vec![TerminalClipboardContent {
                            mime_type: "text/plain;charset=utf-8".to_owned(),
                            data,
                        }],
                    });
            }
        } else if let Some(url) = payload.strip_prefix("OpenURL=") {
            let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
            let url = url
                .strip_prefix(':')
                .and_then(|encoded| BASE64.decode(encoded).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_else(|| url.to_owned());
            self.osc_events.push(TerminalOscEventPayload::OpenUrl {
                request_id,
                url: sanitize_osc_text(&url, 16 * 1024),
            });
        } else if let Some(directory) = payload.strip_prefix("CurrentDir=") {
            self.report_working_directory(directory);
        } else if payload == "StealFocus" {
            self.osc_events.push(TerminalOscEventPayload::FocusRequest);
        } else if let Some(message) = payload.strip_prefix("RequestAttention=") {
            let request = match message {
                "once" => TerminalAttentionRequest::Once,
                "yes" => TerminalAttentionRequest::Continuous,
                "fireworks" => TerminalAttentionRequest::Fireworks,
                "no" => TerminalAttentionRequest::Cancel,
                _ => return,
            };
            self.osc_events
                .push(TerminalOscEventPayload::AttentionRequest { request });
        }
    }

    fn iterm2_variable(&self, name: &str) -> Option<String> {
        match name {
            "session.name" => Some(self.title.read().clone()),
            "session.path" => self
                .reported_location
                .read()
                .as_ref()
                .map(|location| location.path.clone()),
            "session.hostname" => self
                .reported_location
                .read()
                .as_ref()
                .and_then(|location| location.host.clone())
                .or_else(|| Some(local_hostname().to_owned())),
            "session.username" => self
                .reported_location
                .read()
                .as_ref()
                .and_then(|location| location.user.clone())
                .or_else(|| std::env::var("USER").ok()),
            name => name.strip_prefix("user.").and_then(|name| {
                self.shell_integration
                    .lock()
                    .user_variables
                    .get(name)
                    .cloned()
            }),
        }
    }

    fn start_iterm2_multipart(&self, metadata: &str) {
        let (suggested_name, inline) = iterm2_file_metadata(metadata);
        if inline {
            return;
        }
        if let Some(previous) = self.iterm2_multipart.lock().take() {
            let _ = fs::remove_file(previous.temp_path);
        }
        let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
        let temp_path =
            std::env::temp_dir().join(format!("eggie-transfer-{}-{request_id}", self.session_id));
        let Ok(file) = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
        else {
            return;
        };
        *self.iterm2_multipart.lock() = Some(Iterm2MultipartTransfer {
            request_id,
            suggested_name,
            temp_path,
            file,
            size: 0,
        });
    }

    fn append_iterm2_multipart(&self, encoded: &str) {
        let mut multipart = self.iterm2_multipart.lock();
        let Some(transfer) = multipart.as_mut() else {
            return;
        };
        let mut encoded = encoded.as_bytes();
        let decoder = DecoderReader::new(&mut encoded, &BASE64);
        match std::io::copy(
            &mut decoder.take(MAX_ITERM2_TRANSFER_BYTES + 1),
            &mut transfer.file,
        ) {
            Ok(size) if transfer.size.saturating_add(size) <= MAX_ITERM2_TRANSFER_BYTES => {
                transfer.size = transfer.size.saturating_add(size);
            }
            Err(_) => {
                let transfer = multipart
                    .take()
                    .expect("iTerm2 multipart transfer is present");
                let _ = fs::remove_file(transfer.temp_path);
            }
            Ok(_) => {
                let transfer = multipart
                    .take()
                    .expect("iTerm2 multipart transfer is present");
                let _ = fs::remove_file(transfer.temp_path);
            }
        }
    }

    fn finish_iterm2_multipart(&self) {
        let Some(transfer) = self.iterm2_multipart.lock().take() else {
            return;
        };
        if transfer.file.sync_all().is_err() {
            let _ = fs::remove_file(transfer.temp_path);
            return;
        }
        self.pending_file_transfers.lock().insert(
            transfer.request_id,
            PendingFileTransfer::Iterm2 {
                temp_path: transfer.temp_path,
            },
        );
        self.osc_events.push(TerminalOscEventPayload::FileTransfer {
            offer: TerminalFileTransferOffer {
                request_id: transfer.request_id,
                protocol: TerminalFileTransferProtocol::Iterm2,
                suggested_name: transfer.suggested_name,
                size: transfer.size,
                directory: false,
            },
        });
    }

    fn handle_iterm2_file(&self, payload: &str) {
        let Some(value) = payload.strip_prefix("File=") else {
            return;
        };
        let Some((metadata, encoded)) = value.split_once(':') else {
            return;
        };
        let (suggested_name, inline) = iterm2_file_metadata(metadata);
        if inline || encoded.is_empty() {
            return;
        }
        let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
        let temp_path =
            std::env::temp_dir().join(format!("eggie-transfer-{}-{request_id}", self.session_id));
        let Ok(mut file) = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
        else {
            return;
        };
        let mut encoded = encoded.as_bytes();
        let mut decoder = DecoderReader::new(&mut encoded, &BASE64);
        let Ok(size) = std::io::copy(&mut decoder, &mut file) else {
            let _ = fs::remove_file(&temp_path);
            return;
        };
        if file.sync_all().is_err() {
            let _ = fs::remove_file(&temp_path);
            return;
        }
        let mut pending = self.pending_file_transfers.lock();
        while pending.len() >= 16 {
            let Some(oldest) = pending.keys().copied().min() else {
                break;
            };
            if let Some(old) = pending.remove(&oldest) {
                match old {
                    PendingFileTransfer::Iterm2 { temp_path } => {
                        let _ = fs::remove_file(temp_path);
                    }
                    PendingFileTransfer::Kitty { transfer_id } => {
                        self.kitty_file_transfers.lock().remove(&transfer_id);
                    }
                }
            }
        }
        pending.insert(request_id, PendingFileTransfer::Iterm2 { temp_path });
        drop(pending);
        self.osc_events.push(TerminalOscEventPayload::FileTransfer {
            offer: TerminalFileTransferOffer {
                request_id,
                protocol: TerminalFileTransferProtocol::Iterm2,
                suggested_name,
                size,
                directory: false,
            },
        });
    }

    fn complete_file_transfer(&self, request_id: u64, destination: Option<PathBuf>) -> Result<()> {
        let Some(pending) = self.pending_file_transfers.lock().remove(&request_id) else {
            bail!("unknown or expired terminal file-transfer request {request_id}");
        };
        let temp_path = match pending {
            PendingFileTransfer::Kitty { transfer_id } => {
                return self.complete_kitty_file_transfer(&transfer_id, destination);
            }
            PendingFileTransfer::Iterm2 { temp_path } => temp_path,
        };
        let Some(destination) = destination else {
            let _ = fs::remove_file(temp_path);
            return Ok(());
        };
        let copy_result = (|| -> Result<()> {
            let mut input = fs::File::open(&temp_path).with_context(|| {
                format!("failed to open pending transfer {}", temp_path.display())
            })?;
            let mut output = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&destination)
                .with_context(|| format!("failed to create {}", destination.display()))?;
            std::io::copy(&mut input, &mut output)
                .and_then(|_| output.sync_all())
                .with_context(|| {
                    format!(
                        "failed to save terminal transfer to {}",
                        destination.display()
                    )
                })?;
            Ok(())
        })();
        let _ = fs::remove_file(&temp_path);
        copy_result
    }

    fn handle_kitty_file_transfer(&self, payload: &str, terminator: &str) {
        let fields = payload
            .split(';')
            .filter_map(|field| field.split_once('='))
            .collect::<HashMap<_, _>>();
        let Some(action) = fields.get("ac").copied() else {
            return;
        };
        let Some(transfer_id) = fields
            .get("id")
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        match action {
            "send" => {
                if self.kitty_file_transfers.lock().contains_key(&transfer_id) {
                    self.kitty_transfer_status(
                        &transfer_id,
                        None,
                        "EINVAL:Duplicate transfer",
                        None,
                        terminator,
                    );
                    return;
                }
                if self.kitty_file_transfers.lock().len() >= 16 {
                    self.kitty_transfer_status(
                        &transfer_id,
                        None,
                        "EBUSY:Too many transfers",
                        None,
                        terminator,
                    );
                    return;
                }
                let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
                self.kitty_file_transfers.lock().insert(
                    transfer_id.clone(),
                    KittyIncomingTransfer {
                        destination: None,
                        terminator: terminator.to_owned(),
                        files: HashMap::new(),
                    },
                );
                self.pending_file_transfers
                    .lock()
                    .insert(request_id, PendingFileTransfer::Kitty { transfer_id });
                self.osc_events.push(TerminalOscEventPayload::FileTransfer {
                    offer: TerminalFileTransferOffer {
                        request_id,
                        protocol: TerminalFileTransferProtocol::Kitty,
                        suggested_name: "Received Files".to_owned(),
                        size: 0,
                        directory: true,
                    },
                });
            }
            "file" => self.kitty_receive_file(&transfer_id, &fields),
            "data" | "end_data" => {
                self.kitty_receive_data(&transfer_id, &fields, action == "end_data")
            }
            "finish" | "finished" => {
                if let Some(mut transfer) = self.kitty_file_transfers.lock().remove(&transfer_id) {
                    let mut failed = false;
                    for file in transfer.files.values_mut() {
                        if let Some(sink) = file.sink.take() {
                            match sink
                                .finish()
                                .and_then(|handle| handle.sync_all().map(|_| handle))
                            {
                                Ok(handle) => {
                                    file.size = handle
                                        .metadata()
                                        .map_or(file.size, |metadata| metadata.len());
                                    if let Some(mode) = file.permissions {
                                        let _ = fs::set_permissions(
                                            &file.path,
                                            fs::Permissions::from_mode(mode & 0o777),
                                        );
                                    }
                                    failed |=
                                        file.expected_size.is_some_and(|size| size != file.size);
                                }
                                Err(_) => failed = true,
                            }
                        }
                    }
                    self.remove_pending_kitty_transfer(&transfer_id);
                    self.kitty_transfer_status(
                        &transfer_id,
                        None,
                        if failed {
                            "EIO:Incomplete transfer"
                        } else {
                            "OK"
                        },
                        None,
                        &transfer.terminator,
                    );
                }
            }
            "cancel" => {
                let terminator = self
                    .kitty_file_transfers
                    .lock()
                    .remove(&transfer_id)
                    .map_or_else(
                        || terminator.to_owned(),
                        |transfer| {
                            let terminator = transfer.terminator.clone();
                            cleanup_kitty_transfer(transfer);
                            terminator
                        },
                    );
                self.remove_pending_kitty_transfer(&transfer_id);
                self.kitty_transfer_status(&transfer_id, None, "CANCELED", None, &terminator);
            }
            _ => {}
        }
    }

    fn complete_kitty_file_transfer(
        &self,
        transfer_id: &str,
        destination: Option<PathBuf>,
    ) -> Result<()> {
        let mut transfers = self.kitty_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(transfer_id) else {
            bail!("unknown or expired Kitty file-transfer session {transfer_id}");
        };
        let terminator = transfer.terminator.clone();
        if let Some(destination) = destination {
            if !destination.is_dir() {
                bail!("Kitty file-transfer destination is not a directory");
            }
            transfer.destination = Some(destination);
            drop(transfers);
            self.kitty_transfer_status(transfer_id, None, "OK", None, &terminator);
        } else {
            transfers.remove(transfer_id);
            drop(transfers);
            self.kitty_transfer_status(
                transfer_id,
                None,
                "EPERM:User refused the transfer",
                None,
                &terminator,
            );
        }
        Ok(())
    }

    fn kitty_receive_file(&self, transfer_id: &str, fields: &HashMap<&str, &str>) {
        let Some(file_id) = fields
            .get("fid")
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let mut transfers = self.kitty_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(transfer_id) else {
            return;
        };
        let Some(destination) = transfer.destination.as_ref() else {
            return;
        };
        let terminator = transfer.terminator.clone();
        if transfer.files.len() >= 4_096 {
            drop(transfers);
            self.kitty_transfer_status(
                transfer_id,
                Some(&file_id),
                "EFBIG:Too many files",
                None,
                &terminator,
            );
            return;
        }
        let relative_path = safe_kitty_transfer_path(fields.get("n").copied(), &file_id);
        let path = destination.join(relative_path);
        let file_type = fields.get("ft").copied().unwrap_or("regular");
        let result = match file_type {
            "directory" => fs::create_dir_all(&path).map(|_| None),
            "regular" => {
                let create_parent = path.parent().map_or(Ok(()), fs::create_dir_all);
                create_parent
                    .and_then(|_| {
                        OpenOptions::new()
                            .create(true)
                            .truncate(true)
                            .write(true)
                            .open(&path)
                    })
                    .map(|file| {
                        Some(if fields.get("zip").copied() == Some("zlib") {
                            KittyIncomingSink::Zlib(ZlibDecoder::new(file))
                        } else {
                            KittyIncomingSink::Plain(file)
                        })
                    })
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unsupported file type",
            )),
        };
        match result {
            Ok(file) => {
                transfer.files.insert(
                    file_id.clone(),
                    KittyIncomingFile {
                        path,
                        sink: file,
                        size: 0,
                        expected_size: fields.get("sz").and_then(|size| size.parse().ok()),
                        permissions: fields.get("prm").and_then(|mode| mode.parse().ok()),
                        directory: file_type == "directory",
                    },
                );
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    if file_type == "directory" {
                        "OK"
                    } else {
                        "STARTED"
                    },
                    Some(0),
                    &terminator,
                );
            }
            Err(error) => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    &format!("EIO:{error}"),
                    None,
                    &terminator,
                );
            }
        }
    }

    fn kitty_receive_data(&self, transfer_id: &str, fields: &HashMap<&str, &str>, finished: bool) {
        let Some(file_id) = fields
            .get("fid")
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let mut transfers = self.kitty_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(transfer_id) else {
            return;
        };
        let terminator = transfer.terminator.clone();
        let data = match fields.get("d").map(|data| BASE64.decode(data)) {
            Some(Ok(data)) if data.len() <= KITTY_CLIPBOARD_CHUNK_BYTES => data,
            Some(Ok(_)) => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    "EINVAL:Data chunk exceeds 4096 bytes",
                    None,
                    &terminator,
                );
                return;
            }
            Some(Err(_)) => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    "EINVAL:Invalid base64 data",
                    None,
                    &terminator,
                );
                return;
            }
            None if finished => Vec::new(),
            None => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    "EINVAL:Missing data",
                    None,
                    &terminator,
                );
                return;
            }
        };
        let Some(file) = transfer.files.get_mut(&file_id) else {
            drop(transfers);
            self.kitty_transfer_status(
                transfer_id,
                Some(&file_id),
                "EINVAL:Unknown file",
                None,
                &terminator,
            );
            return;
        };
        let result = file
            .sink
            .as_mut()
            .ok_or_else(|| std::io::Error::other("file has no data stream"))
            .and_then(|sink| sink.write_chunk(&data));
        if let Ok(written) = result {
            file.size = file.size.saturating_add(written);
            if finished {
                let finish = file
                    .sink
                    .take()
                    .expect("Kitty data stream exists after successful write")
                    .finish()
                    .and_then(|handle| handle.sync_all().map(|_| handle));
                match finish {
                    Ok(handle) => {
                        file.size = handle
                            .metadata()
                            .map_or(file.size, |metadata| metadata.len());
                        if let Some(mode) = file.permissions {
                            let _ = fs::set_permissions(
                                &file.path,
                                fs::Permissions::from_mode(mode & 0o777),
                            );
                        }
                    }
                    Err(error) => {
                        let path = file.path.clone();
                        let size = file.size;
                        drop(transfers);
                        let _ = fs::remove_file(path);
                        self.kitty_transfer_status(
                            transfer_id,
                            Some(&file_id),
                            &format!("EIO:{error}"),
                            Some(size),
                            &terminator,
                        );
                        return;
                    }
                }
            }
        }
        let size = file.size;
        let wrong_size = finished && file.expected_size.is_some_and(|expected| expected != size);
        let path = file.path.clone();
        drop(transfers);
        if wrong_size {
            let _ = fs::remove_file(&path);
        }
        match result {
            Ok(_) => self.kitty_transfer_status(
                transfer_id,
                Some(&file_id),
                if wrong_size {
                    "EIO:Size mismatch"
                } else if finished {
                    "OK"
                } else {
                    "PROGRESS"
                },
                Some(size),
                &terminator,
            ),
            Err(error) => {
                let _ = fs::remove_file(path);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    &format!("EIO:{error}"),
                    Some(size),
                    &terminator,
                );
            }
        }
    }

    fn remove_pending_kitty_transfer(&self, transfer_id: &str) {
        self.pending_file_transfers.lock().retain(|_, pending| {
            !matches!(pending, PendingFileTransfer::Kitty { transfer_id: pending_id } if pending_id == transfer_id)
        });
    }

    fn kitty_transfer_status(
        &self,
        transfer_id: &str,
        file_id: Option<&str>,
        status: &str,
        size: Option<u64>,
        terminator: &str,
    ) {
        let mut response = format!(
            "\x1b]5113;ac=status;id={transfer_id};st={}",
            BASE64.encode(status.as_bytes())
        );
        if let Some(file_id) = file_id {
            response.push_str(";fid=");
            response.push_str(file_id);
        }
        if let Some(size) = size {
            use std::fmt::Write as _;
            let _ = write!(response, ";sz={size}");
        }
        response.push_str(terminator);
        self.write_to_pty(response.into_bytes());
    }

    fn handle_kitty_clipboard(&self, payload: &str, terminator: &str) {
        let (metadata, encoded_payload) = payload.split_once(';').unwrap_or((payload, ""));
        let fields = colon_metadata(metadata);
        let kind = fields.get("type").map_or("", String::as_str);
        let id = fields
            .get("id")
            .map_or_else(String::new, |id| sanitize_protocol_id(id));
        let selection = if fields.get("loc").is_some_and(|value| value == "primary") {
            TerminalClipboardSelection::Primary
        } else {
            TerminalClipboardSelection::Clipboard
        };
        match kind {
            "write" => {
                *self.rich_clipboard_write.lock() = Some(RichClipboardWrite {
                    id,
                    selection,
                    contents: Vec::new(),
                    aliases: Vec::new(),
                    total_size: 0,
                    terminator: terminator.to_owned(),
                });
            }
            "wdata" => {
                let mut pending = self.rich_clipboard_write.lock();
                let Some(write) = pending.as_mut() else {
                    return;
                };
                if !id.is_empty() && id != write.id {
                    return;
                }
                if let Some(mime) = fields.get("mime").and_then(|mime| decode_base64_text(mime)) {
                    if mime.len() > 1_024
                        || (!write
                            .contents
                            .iter()
                            .any(|content| content.mime_type == mime)
                            && write.contents.len() >= 64)
                    {
                        let failed = pending.take().expect("rich clipboard write is present");
                        self.write_to_pty(
                            format!(
                                "\x1b]5522;type=write:status=EIO{}{}",
                                protocol_id_field(&failed.id),
                                failed.terminator
                            )
                            .into_bytes(),
                        );
                        return;
                    }
                    let Ok(data) = BASE64.decode(encoded_payload) else {
                        let failed = pending.take().expect("rich clipboard write is present");
                        self.write_to_pty(
                            format!(
                                "\x1b]5522;type=write:status=EINVAL{}{}",
                                protocol_id_field(&failed.id),
                                failed.terminator
                            )
                            .into_bytes(),
                        );
                        return;
                    };
                    if data.len() > KITTY_CLIPBOARD_CHUNK_BYTES
                        || write.total_size.saturating_add(data.len()) > MAX_RICH_CLIPBOARD_BYTES
                    {
                        let failed = pending.take().expect("rich clipboard write is present");
                        self.write_to_pty(
                            format!(
                                "\x1b]5522;type=write:status=EIO{}{}",
                                protocol_id_field(&failed.id),
                                failed.terminator
                            )
                            .into_bytes(),
                        );
                        return;
                    }
                    write.total_size += data.len();
                    if let Some(existing) = write
                        .contents
                        .iter_mut()
                        .find(|content| content.mime_type == mime)
                    {
                        existing.data.extend(data);
                    } else {
                        write.contents.push(TerminalClipboardContent {
                            mime_type: mime,
                            data,
                        });
                    }
                } else {
                    let mut write = pending.take().expect("rich clipboard write is present");
                    let mut committed_size = write.total_size;
                    for (target, alias) in &write.aliases {
                        if write
                            .contents
                            .iter()
                            .any(|content| content.mime_type == *alias)
                        {
                            continue;
                        }
                        if let Some(content) = write
                            .contents
                            .iter()
                            .find(|content| content.mime_type == *target)
                            .cloned()
                        {
                            if committed_size.saturating_add(content.data.len())
                                > MAX_RICH_CLIPBOARD_BYTES
                            {
                                continue;
                            }
                            committed_size += content.data.len();
                            write.contents.push(TerminalClipboardContent {
                                mime_type: alias.clone(),
                                data: content.data,
                            });
                        }
                    }
                    self.osc_events
                        .push(TerminalOscEventPayload::ClipboardWrite {
                            selection: write.selection,
                            contents: write.contents,
                        });
                    self.write_to_pty(
                        format!(
                            "\x1b]5522;type=write:status=DONE{}{}",
                            protocol_id_field(&write.id),
                            write.terminator
                        )
                        .into_bytes(),
                    );
                }
            }
            "walias" => {
                let Some(target) = fields.get("mime").and_then(|mime| decode_base64_text(mime))
                else {
                    return;
                };
                let aliases = BASE64
                    .decode(encoded_payload)
                    .ok()
                    .and_then(|aliases| String::from_utf8(aliases).ok())
                    .unwrap_or_default();
                if let Some(write) = self.rich_clipboard_write.lock().as_mut() {
                    if !id.is_empty() && id != write.id {
                        return;
                    }
                    for alias in aliases.split_whitespace().take(32) {
                        if alias.len() <= 1_024 && write.aliases.len() < 64 {
                            write.aliases.push((target.clone(), alias.to_owned()));
                        }
                    }
                }
            }
            "read" => {
                let requested = BASE64
                    .decode(encoded_payload)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .or_else(|| fields.get("mime").and_then(|mime| decode_base64_text(mime)))
                    .unwrap_or_else(|| "text/plain;charset=utf-8".to_owned());
                let list_only = requested.trim() == ".";
                let mime_types = if list_only {
                    vec![".".to_owned()]
                } else {
                    requested
                        .split_whitespace()
                        .filter(|mime| mime.len() <= 1_024)
                        .take(64)
                        .map(str::to_owned)
                        .collect()
                };
                let authorized_contents = fields
                    .get("pw")
                    .and_then(|password| decode_base64_text(password))
                    .and_then(|password| {
                        let now = Instant::now();
                        let mut grants = self.paste_clipboard_grants.lock();
                        grants.retain(|_, grant| grant.expires_at > now);
                        grants.remove(&password).and_then(|grant| {
                            (grant.selection == selection).then_some(grant.contents)
                        })
                    });
                if let Some(contents) = authorized_contents {
                    let filtered = filter_clipboard_contents(contents, &mime_types);
                    self.send_kitty_clipboard_read_response(
                        &id, selection, false, &filtered, terminator,
                    );
                    return;
                }
                if !list_only && !self.allow_clipboard_read.load(Ordering::Acquire) {
                    self.write_to_pty(
                        format!(
                            "\x1b]5522;type=read:status=EPERM{}{terminator}",
                            protocol_id_field(&id)
                        )
                        .into_bytes(),
                    );
                    return;
                }
                let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
                self.pending_clipboard_reads.lock().insert(
                    request_id,
                    ClipboardReadResponder::Kitty {
                        id,
                        terminator: terminator.to_owned(),
                        selection,
                        list_only,
                    },
                );
                self.osc_events
                    .push(TerminalOscEventPayload::ClipboardRead {
                        request_id,
                        selection,
                        mime_types,
                    });
            }
            _ => {}
        }
    }
}

impl Drop for ListenerState {
    fn drop(&mut self) {
        if let Some(transfer) = self.iterm2_multipart.get_mut().take() {
            let _ = fs::remove_file(transfer.temp_path);
        }
        for (_, pending) in self.pending_file_transfers.get_mut().drain() {
            if let PendingFileTransfer::Iterm2 { temp_path } = pending {
                let _ = fs::remove_file(temp_path);
            }
        }
        for (_, transfer) in self.kitty_file_transfers.get_mut().drain() {
            cleanup_kitty_transfer(transfer);
        }
    }
}

fn cleanup_kitty_transfer(transfer: KittyIncomingTransfer) {
    let mut directories = Vec::new();
    for file in transfer.files.into_values() {
        drop(file.sink);
        if file.directory {
            directories.push(file.path);
        } else {
            let _ = fs::remove_file(file.path);
        }
    }
    directories.sort_unstable_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        let _ = fs::remove_dir(directory);
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn option_value(options: &str, key: &str) -> Option<String> {
    options.split(';').find_map(|option| {
        let (candidate, value) = option.split_once('=')?;
        (candidate == key).then(|| value.to_owned())
    })
}

fn percent_decode(value: String) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                decoded.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn parse_reported_location(value: &str) -> Option<TerminalReportedLocation> {
    if value.is_empty() {
        return None;
    }
    if let Some(rest) = value.strip_prefix("file://") {
        let (authority, path) = rest
            .split_once('/')
            .map_or((rest, ""), |(host, path)| (host, path));
        let path = percent_decode(format!("/{path}"));
        let (user, host) = authority
            .split_once('@')
            .map_or((None, authority), |(user, host)| {
                (Some(user.to_owned()), host)
            });
        let host = (!host.is_empty()).then(|| host.to_owned());
        return Some(TerminalReportedLocation {
            user,
            local: host.as_deref().is_none_or(host_is_local),
            host,
            path,
        });
    }
    Some(TerminalReportedLocation {
        user: None,
        host: None,
        path: percent_decode(value.to_owned()),
        local: true,
    })
}

fn host_is_local(host: &str) -> bool {
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case(local_hostname())
        || host
            .split_once('.')
            .map(|(short, _)| short)
            .is_some_and(|short| short.eq_ignore_ascii_case(local_hostname()))
}

fn local_hostname() -> &'static str {
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME.get_or_init(|| {
        let mut buffer = [0_u8; 256];
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if result != 0 {
            return String::new();
        }
        let end = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        String::from_utf8_lossy(&buffer[..end]).into_owned()
    })
}

fn clipboard_selection(clipboard: ClipboardType) -> TerminalClipboardSelection {
    match clipboard {
        ClipboardType::Clipboard => TerminalClipboardSelection::Clipboard,
        ClipboardType::Selection => TerminalClipboardSelection::Primary,
    }
}

fn generated_notification_id(session_id: SessionId, sequence: &AtomicU64) -> String {
    format!(
        "{}-{}",
        session_id,
        sequence.fetch_add(1, Ordering::Relaxed)
    )
}

fn colon_metadata(metadata: &str) -> HashMap<String, String> {
    metadata
        .split(':')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn decode_base64_text(value: &str) -> Option<String> {
    BASE64
        .decode(value)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn sanitize_protocol_id(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || "-_+.".contains(*character))
        .take(256)
        .collect()
}

fn protocol_id_field(id: &str) -> String {
    if id.is_empty() {
        String::new()
    } else {
        format!(":id={id}")
    }
}

fn filter_clipboard_contents(
    contents: Vec<TerminalClipboardContent>,
    requested_mime_types: &[String],
) -> Vec<TerminalClipboardContent> {
    contents
        .into_iter()
        .filter(|content| {
            let base = content
                .mime_type
                .split(';')
                .next()
                .unwrap_or(&content.mime_type);
            requested_mime_types.iter().any(|requested| {
                let requested_base = requested.split(';').next().unwrap_or(requested);
                requested == "."
                    || requested == "*/*"
                    || requested == &content.mime_type
                    || requested_base == base
                    || requested
                        .strip_suffix("/*")
                        .is_some_and(|prefix| base.starts_with(prefix))
            })
        })
        .collect()
}

fn sanitize_osc_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(limit)
        .collect()
}

fn append_sanitized_osc_text(output: &mut String, value: &str, limit: usize) {
    let remaining = limit.saturating_sub(output.chars().count());
    output.extend(
        value
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .take(remaining),
    );
}

fn sanitize_file_name(value: &str) -> String {
    let mut name = value
        .chars()
        .filter(|character| !character.is_control() && *character != '/' && *character != '\\')
        .take(255)
        .collect::<String>();
    if name.is_empty() || name == "." || name == ".." {
        name = "terminal-download".to_owned();
    }
    name
}

fn safe_kitty_transfer_path(encoded_name: Option<&str>, file_id: &str) -> PathBuf {
    let decoded = encoded_name
        .and_then(|name| BASE64.decode(name).ok())
        .and_then(|name| String::from_utf8(name).ok());
    let Some(decoded) = decoded else {
        return PathBuf::from(sanitize_file_name(file_id));
    };
    let mut safe = PathBuf::new();
    let mut total_length = 0_usize;
    for component in Path::new(&decoded).components().take(64) {
        match component {
            std::path::Component::Normal(component) => {
                let component = sanitize_file_name(&component.to_string_lossy());
                total_length = total_length.saturating_add(component.len());
                if total_length > 4_096 {
                    break;
                }
                safe.push(component);
            }
            std::path::Component::ParentDir => {
                return PathBuf::from(sanitize_file_name(file_id));
            }
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::CurDir => {}
        }
    }
    if safe.as_os_str().is_empty() {
        PathBuf::from(sanitize_file_name(file_id))
    } else {
        safe
    }
}

fn iterm2_file_metadata(metadata: &str) -> (String, bool) {
    let fields = metadata
        .split(';')
        .filter_map(|field| field.split_once('='))
        .collect::<HashMap<_, _>>();
    let suggested_name = fields
        .get("name")
        .and_then(|name| BASE64.decode(name).ok())
        .and_then(|name| String::from_utf8(name).ok())
        .and_then(|name| {
            Path::new(&name)
                .file_name()
                .and_then(|name| name.to_str())
                .map(sanitize_file_name)
        })
        .unwrap_or_else(|| "terminal-download".to_owned());
    (suggested_name, fields.get("inline").copied() == Some("1"))
}

fn render_metrics_enabled() -> bool {
    *RENDER_METRICS_ENABLED.get_or_init(|| {
        std::env::var_os("EGGIE_RENDER_METRICS")
            .is_some_and(|value| !value.is_empty() && value != "0")
    })
}

/// Scan the visible viewport for bare URLs and return them in viewport-relative coordinates. A URL
/// that soft-wraps across rows is split into one range per row so each row can be drawn and
/// hit-tested independently. The scan mirrors `collect_viewport_matches`: the viewport maps to grid
/// lines `[-display_offset, screen_lines - display_offset)`.
fn detect_viewport_urls<T>(
    terminal: &Term<T>,
    regex: &mut RegexSearch,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Vec<TerminalLinkRange> {
    if screen_lines == 0 || columns == 0 {
        return Vec::new();
    }
    let top_line = Line(-(display_offset as i32));
    let bottom_line = Line(screen_lines as i32 - 1 - display_offset as i32);
    let start = Point::new(top_line, Column(0));
    let end = Point::new(bottom_line, terminal.grid().last_column());
    let mut links = Vec::new();
    for regex_match in RegexIter::new(start, end, Direction::Right, terminal, regex) {
        let Some((url, refined)) = refine_url_range(terminal, regex_match) else {
            continue;
        };
        // Split a multi-row (soft-wrapped) match into one range per visible row.
        let start = *refined.start();
        let end = *refined.end();
        for line in start.line.0..=end.line.0 {
            let row_start_col = if line == start.line.0 {
                start.column.0
            } else {
                0
            };
            let row_end_col = if line == end.line.0 {
                end.column.0
            } else {
                columns - 1
            };
            let range = Point::new(Line(line), Column(row_start_col))
                ..=Point::new(Line(line), Column(row_end_col));
            if let Some((vp_start, vp_end)) =
                clamp_range_to_viewport(range, display_offset, screen_lines, columns)
            {
                links.push(TerminalLinkRange {
                    start: vp_start,
                    end: vp_end,
                    url: url.clone(),
                });
            }
        }
    }
    links
}

/// Trim trailing punctuation and unbalanced closing brackets from a raw URL match. Handles the
/// common cases: a sentence-ending `.`/`,`, a wrapping `(url)`, while keeping balanced brackets like
/// `wiki_(foo)`. Returns the cleaned prefix (possibly empty).
fn trim_url_trailing(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 0 {
        let ch = chars[keep - 1];
        let strip = match ch {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' | ']' | '}' => {
                let (open, close) = match ch {
                    ')' => ('(', ')'),
                    ']' => ('[', ']'),
                    _ => ('{', '}'),
                };
                let opens = chars[..keep].iter().filter(|&&c| c == open).count();
                let closes = chars[..keep].iter().filter(|&&c| c == close).count();
                closes > opens
            }
            _ => false,
        };
        if strip {
            keep -= 1;
        } else {
            break;
        }
    }
    chars[..keep].iter().collect()
}

/// Trim trailing punctuation from a raw regex match, returning the cleaned URL text and the grid
/// range that still covers it. Returns `None` if nothing usable remains.
fn refine_url_range<T>(
    terminal: &Term<T>,
    regex_match: RangeInclusive<Point>,
) -> Option<(String, RangeInclusive<Point>)> {
    let start = *regex_match.start();
    let end = *regex_match.end();
    let text = terminal.bounds_to_string(start, end);
    let url = trim_url_trailing(&text);
    if url.is_empty() {
        return None;
    }
    // Shrink the grid end by the number of stripped chars. bounds_to_string emits one char per
    // (non-wide-spacer) cell on a line, and the trimmed punctuation set is all single-column ASCII,
    // so the column shift maps 1:1.
    let trimmed_cols = text.chars().count() - url.chars().count();
    let new_end = if trimmed_cols == 0 {
        end
    } else {
        let mut point = end;
        for _ in 0..trimmed_cols {
            point = point.sub(terminal, Boundary::Grid, 1);
        }
        point
    };
    Some((url, start..=new_end))
}

#[derive(Clone)]
struct DaemonEventListener(Arc<ListenerState>);

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                *self.0.title.write() = title;
            }
            Event::ResetTitle => {
                *self.0.title.write() = "shell".to_owned();
            }
            Event::ProgressReport(progress) => self.0.progress.report(progress),
            Event::WorkingDirectory(directory) => {
                self.0.report_working_directory(&directory);
            }
            Event::SemanticPrompt(prompt, cursor_line, history_size) => {
                self.0
                    .shell_integration
                    .lock()
                    .update(prompt, cursor_line, history_size);
            }
            Event::DesktopNotification(notification) => {
                self.0.handle_notification(notification);
            }
            Event::Iterm2Command(payload, terminator) => {
                self.0.handle_iterm2_command(&payload, &terminator);
            }
            Event::KittyClipboard(payload, terminator) => {
                self.0.handle_kitty_clipboard(&payload, &terminator);
            }
            Event::KittyFileTransfer(payload, terminator) => {
                self.0.handle_kitty_file_transfer(&payload, &terminator);
            }
            Event::PtyWrite(text) => self.0.write_to_pty(text.into_bytes()),
            Event::TextAreaSizeRequest(format) => {
                let size = *self.0.size.read();
                self.0.write_to_pty(format(window_size(size)).into_bytes());
            }
            Event::ColorRequest(index, dynamic_color, format) => {
                let color = dynamic_color
                    .or_else(|| self.0.appearance.read().color(index).map(rgb_from_u32));
                if let Some(color) = color {
                    self.0.write_to_pty(format(color).into_bytes());
                }
            }
            Event::ChildExit(status) => {
                *self.0.status.write() = SessionStatus::Exited {
                    code: status.code(),
                };
                self.0.progress.report(None);
            }
            Event::Exit => {
                *self.0.status.write() = SessionStatus::Exited { code: None };
                self.0.progress.report(None);
            }
            Event::ClipboardStore(clipboard, text) => self.0.clipboard_store(clipboard, text),
            Event::ClipboardLoad(clipboard, formatter) => {
                self.0.clipboard_load(clipboard, formatter);
            }
            Event::Bell => {
                self.0.ring_bell();
            }
            Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {}
        }
    }
}

struct TerminalSession {
    id: SessionId,
    project_id: ProjectId,
    initial_directory: PathBuf,
    shell_pid: u32,
    pty_fd: i32,
    runtime_metadata: Mutex<SessionRuntimeMetadata>,
    inspection_system: Mutex<System>,
    terminal: Arc<FairMutex<Term<DaemonEventListener>>>,
    events: Arc<ListenerState>,
    sender: EventLoopSender,
    last_input_sequence: Arc<AtomicU64>,
    mouse_state: Mutex<TerminalMouseState>,
    search_state: Mutex<TerminalSearchState>,
}

/// Per-session terminal search cursor. Tracks the last active match (in grid-absolute coordinates)
/// so `find next`/`find previous` can advance relative to it across requests.
#[derive(Default)]
struct TerminalSearchState {
    /// The query the current cursor position belongs to. Reset the cursor whenever it changes.
    query: String,
    regex: bool,
    /// Grid-absolute range of the current active match, if any. The start keys the match against
    /// the full-buffer enumeration; the end is where forward navigation advances from (and the
    /// start where backward navigation does), so overlapping matches are not re-found.
    active: Option<RangeInclusive<Point>>,
}

enum TerminalSnapshotUpdate {
    Full(Arc<TerminalSnapshot>),
    Delta(TerminalSnapshotDelta),
}

#[derive(Default)]
struct TerminalMouseState {
    last_position: Option<TerminalMousePosition>,
    accumulated_scroll_x: f64,
    accumulated_scroll_y: f64,
}

struct SessionRuntimeMetadata {
    system: System,
    last_foreground_pid: Option<Pid>,
    current_process: ProcessSummary,
    current_directory: PathBuf,
    last_refresh: Option<Instant>,
}

impl TerminalSession {
    fn spawn(
        project_id: ProjectId,
        cwd: PathBuf,
        size: TerminalSize,
        appearance: TerminalAppearance,
    ) -> Result<Self> {
        let id = SessionId::new_v4();
        let last_input_sequence = Arc::new(AtomicU64::new(0));
        let state = Arc::new(ListenerState::new(
            id,
            size,
            appearance,
            last_input_sequence.clone(),
        ));
        let listener = DaemonEventListener(state.clone());
        let config = terminal_config(CursorShape::default());
        let mut terminal = Term::new(config, &GridSize(size), listener.clone());
        terminal.set_kitty_graphics_cell_size(size.cell_width, size.cell_height);
        let terminal = Arc::new(FairMutex::new(terminal));
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
        let shell_name = Path::new(&shell)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shell")
            .to_owned();
        let terminfo = install_bundled_terminfo()?;
        // Install the shell-integration scripts so the child shell reports OSC 133 semantic
        // prompts. Non-fatal: if it fails we simply don't inject, and resize falls back to
        // Alacritty's native reflow.
        let integration_root = match install_bundled_shell_integration() {
            Ok(root) => Some(root),
            Err(error) => {
                eprintln!("failed to install shell integration: {error:#}");
                None
            }
        };
        let launch = build_shell_env(
            &shell_name,
            &shell,
            &terminfo,
            integration_root.as_deref(),
            std::env::var("ZDOTDIR").ok(),
            std::env::var("ENV").ok(),
        );
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell, launch.args)),
            working_directory: Some(cwd.clone()),
            drain_on_exit: true,
            env: launch.env,
            child_signal_mask: tty::SignalMask::current().ok(),
        };
        let pty = tty::new(&options, window_size(size), id.as_u128() as u64)
            .with_context(|| format!("failed to create PTY in {}", cwd.display()))?;
        let shell_pid = pty.child().id();
        let pty_fd = pty.file().as_raw_fd();
        state.publish_terminal(&terminal.lock());
        let publisher = state.clone();
        let event_loop = EventLoop::new(terminal.clone(), listener, pty, true, false)
            .context("failed to create Alacritty event loop")?
            .with_terminal_update(move |terminal| publisher.publish_terminal(terminal))
            .with_terminal_update_interval(TERMINAL_FRAME_INTERVAL);
        let sender = event_loop.channel();
        *state.sender.lock() = Some(sender.clone());
        let _io_thread = event_loop.spawn();

        Ok(Self {
            id,
            project_id,
            initial_directory: cwd.clone(),
            shell_pid,
            pty_fd,
            runtime_metadata: Mutex::new(SessionRuntimeMetadata {
                system: System::new(),
                last_foreground_pid: None,
                current_process: ProcessSummary {
                    pid: shell_pid,
                    name: shell_name,
                },
                current_directory: cwd,
                last_refresh: None,
            }),
            inspection_system: Mutex::new(System::new()),
            terminal,
            events: state,
            sender,
            last_input_sequence,
            mouse_state: Mutex::new(TerminalMouseState::default()),
            search_state: Mutex::new(TerminalSearchState::default()),
        })
    }

    fn summary(&self) -> SessionSummary {
        self.refresh_runtime_metadata();
        let runtime_metadata = self.runtime_metadata.lock();
        let reported_location = self.events.reported_location.read().clone();
        let current_directory = reported_location
            .as_ref()
            .filter(|location| location.local && !location.path.is_empty())
            .map(|location| PathBuf::from(&location.path))
            .unwrap_or_else(|| runtime_metadata.current_directory.clone());
        SessionSummary {
            id: self.id,
            project_id: self.project_id,
            title: self.events.title.read().clone(),
            initial_directory: self.initial_directory.clone(),
            current_directory,
            reported_location,
            shell_pid: self.shell_pid,
            current_process: runtime_metadata.current_process.clone(),
            status: self.events.status.read().clone(),
        }
    }

    fn inspection(&self) -> SessionInspection {
        let processes = descendant_processes(self.shell_pid, &mut self.inspection_system.lock());
        let ports = listening_ports(&processes);
        SessionInspection {
            session_id: self.id,
            processes,
            ports,
        }
    }

    fn refresh_runtime_metadata(&self) {
        let now = Instant::now();
        let mut metadata = self.runtime_metadata.lock();
        if metadata.last_refresh.is_some_and(|last_refresh| {
            now.duration_since(last_refresh) < PROCESS_METADATA_REFRESH_INTERVAL
        }) {
            return;
        }
        metadata.last_refresh = Some(now);

        let foreground_pid = unsafe { libc::tcgetpgrp(self.pty_fd) };
        let foreground_pid = if foreground_pid > 0 {
            Pid::from_u32(foreground_pid as u32)
        } else {
            Pid::from_u32(self.shell_pid)
        };
        let shell_pid = Pid::from_u32(self.shell_pid);
        if metadata.last_foreground_pid.replace(foreground_pid) != Some(foreground_pid) {
            metadata.system = System::new();
        }

        let pids = [foreground_pid, shell_pid];
        let pids = if foreground_pid == shell_pid {
            &pids[..1]
        } else {
            &pids[..]
        };
        let refresh_kind = ProcessRefreshKind::new()
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::Always);
        metadata
            .system
            .refresh_processes_specifics(ProcessesToUpdate::Some(pids), refresh_kind);

        let process_metadata = metadata
            .system
            .process(foreground_pid)
            .or_else(|| metadata.system.process(shell_pid))
            .map(|process| {
                let name = process.name().to_string_lossy().into_owned();
                let cwd = process.cwd().map(Path::to_path_buf);
                (process.pid().as_u32(), name, cwd)
            });
        if let Some((pid, name, cwd)) = process_metadata {
            metadata.current_process = ProcessSummary { pid, name };
            if let Some(cwd) = cwd.filter(|cwd| !cwd.as_os_str().is_empty()) {
                metadata.current_directory = cwd;
            }
        }
    }

    fn input(&self, bytes: Vec<u8>, sequence: u64) -> Result<()> {
        self.last_input_sequence.store(sequence, Ordering::Release);
        // Scroll-on-keystroke: typing while scrolled back into history snaps the viewport to the
        // live bottom, so the user sees what they type. The kernel pins the viewport during output
        // when display_offset != 0, so without this a keystroke's echo would land off-screen.
        {
            let mut terminal = self.terminal.lock();
            if terminal.grid().display_offset() != 0 {
                terminal.scroll_display(Scroll::Bottom);
                self.events.publish_terminal(&terminal);
            }
        }
        self.sender
            .send(Msg::Input(bytes.into()))
            .context("failed to write terminal input")
    }

    fn paste(&self, text: String, sequence: u64) -> Result<()> {
        let bracketed = self
            .terminal
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        self.input(paste_bytes(&text, bracketed), sequence)
    }

    fn paste_clipboard(
        &self,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
    ) -> Result<()> {
        let mode = *self.terminal.lock().mode();
        if mode.contains(TermMode::PASTE_EVENTS) {
            self.last_input_sequence.store(sequence, Ordering::Release);
            self.events.paste_clipboard(selection, contents);
            return Ok(());
        }
        let text = contents
            .iter()
            .find(|content| content.mime_type.starts_with("text/plain"))
            .and_then(|content| String::from_utf8(content.data.clone()).ok())
            .unwrap_or_default();
        self.paste(text, sequence)
    }

    fn mouse(&self, event: TerminalMouseEvent) -> Result<()> {
        let mut mouse_state = self.mouse_state.lock();
        if event.action == TerminalMouseAction::Move
            && mouse_state.last_position == Some(event.position)
        {
            return Ok(());
        }
        mouse_state.last_position = Some(event.position);
        drop(mouse_state);

        let terminal = self.terminal.lock();
        let mode = *terminal.mode();
        let display_offset = terminal.grid().display_offset();
        drop(terminal);

        if let Some(bytes) = mouse_report_bytes(mode, display_offset, event) {
            self.sender
                .send(Msg::Input(bytes.into()))
                .context("failed to write terminal mouse input")?;
        }
        Ok(())
    }

    fn scroll(&self, event: TerminalScrollEvent) -> Result<()> {
        let size = *self.events.size.read();
        let scale = f64::from(TERMINAL_SCROLL_DELTA_SCALE);
        let (mut delta_x, mut delta_y) = match event.delta.unit {
            TerminalScrollUnit::Pixels => (
                f64::from(event.delta.x) / scale,
                f64::from(event.delta.y) / scale,
            ),
            TerminalScrollUnit::Lines => (
                f64::from(event.delta.x) * f64::from(size.cell_width.max(1)) / scale,
                f64::from(event.delta.y) * f64::from(size.cell_height.max(1)) / scale,
            ),
        };

        if event.delta.unit == TerminalScrollUnit::Pixels
            && event.phase == TerminalScrollPhase::Moved
        {
            let magnitude = delta_x.hypot(delta_y);
            if magnitude > 0. && delta_x.abs() / magnitude > 0.9 {
                delta_y = 0.;
            } else {
                delta_x = 0.;
            }
        }

        let mut terminal = self.terminal.lock();
        let initial_display_offset = terminal.grid().display_offset();
        let mode = *terminal.mode();
        let mouse_mode = terminal_mouse_mode(mode);
        let multiplier = if mouse_mode { 1. } else { 3. };
        let mut mouse_state = self.mouse_state.lock();
        if event.phase == TerminalScrollPhase::Started {
            mouse_state.accumulated_scroll_x = 0.;
            mouse_state.accumulated_scroll_y = 0.;
        }
        mouse_state.accumulated_scroll_x += delta_x * multiplier;
        mouse_state.accumulated_scroll_y += delta_y * multiplier;

        let cell_width = f64::from(size.cell_width.max(1));
        let cell_height = f64::from(size.cell_height.max(1));
        let columns = (mouse_state.accumulated_scroll_x / cell_width).abs() as usize;
        let lines = (mouse_state.accumulated_scroll_y / cell_height).abs() as usize;
        let scroll_up = delta_y > 0.;
        let scroll_left = delta_x > 0.;

        if mouse_mode {
            let display_offset = terminal.grid().display_offset();
            let mut bytes = Vec::new();
            let vertical_button = if scroll_up { 64 } else { 65 };
            let horizontal_button = if scroll_left { 66 } else { 67 };
            for _ in 0..lines {
                if let Some(report) = mouse_report_from_code(
                    mode,
                    display_offset,
                    event.position,
                    vertical_button,
                    false,
                    event.modifiers,
                ) {
                    bytes.extend(report);
                }
            }
            for _ in 0..columns {
                if let Some(report) = mouse_report_from_code(
                    mode,
                    display_offset,
                    event.position,
                    horizontal_button,
                    false,
                    event.modifiers,
                ) {
                    bytes.extend(report);
                }
            }
            if !bytes.is_empty() {
                self.sender
                    .send(Msg::Input(bytes.into()))
                    .context("failed to write terminal scroll input")?;
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
            && !event.modifiers.shift
        {
            let mut bytes = Vec::with_capacity(3 * (lines + columns));
            let line_command = if scroll_up { b'A' } else { b'B' };
            let column_command = if scroll_left { b'D' } else { b'C' };
            for _ in 0..lines {
                bytes.extend_from_slice(&[0x1b, b'O', line_command]);
            }
            for _ in 0..columns {
                bytes.extend_from_slice(&[0x1b, b'O', column_command]);
            }
            if !bytes.is_empty() {
                self.sender
                    .send(Msg::Input(bytes.into()))
                    .context("failed to write alternate-screen scroll input")?;
            }
        } else if lines != 0 {
            let lines = if scroll_up {
                lines as i32
            } else {
                -(lines as i32)
            };
            terminal.scroll_display(Scroll::Delta(lines));
        }

        mouse_state.accumulated_scroll_x %= cell_width;
        mouse_state.accumulated_scroll_y %= cell_height;
        let viewport_changed = terminal.grid().display_offset() != initial_display_offset;
        drop(mouse_state);
        if viewport_changed {
            self.events.publish_terminal(&terminal);
        }
        drop(terminal);
        Ok(())
    }

    fn scroll_to(&self, command: TerminalScrollCommand) -> Result<()> {
        let direction = match command {
            TerminalScrollCommand::PrevPrompt => Some(TerminalJumpDirection::Up),
            TerminalScrollCommand::NextPrompt => Some(TerminalJumpDirection::Down),
            _ => None,
        };
        if let Some(direction) = direction {
            return self.jump_to_prompt(direction);
        }
        let mut terminal = self.terminal.lock();
        let initial_display_offset = terminal.grid().display_offset();
        let scroll = match command {
            TerminalScrollCommand::Top => Scroll::Top,
            TerminalScrollCommand::Bottom => Scroll::Bottom,
            TerminalScrollCommand::PageUp => Scroll::PageUp,
            TerminalScrollCommand::PageDown => Scroll::PageDown,
            TerminalScrollCommand::PrevPrompt | TerminalScrollCommand::NextPrompt => unreachable!(),
        };
        terminal.scroll_display(scroll);
        let viewport_changed = terminal.grid().display_offset() != initial_display_offset;
        if viewport_changed {
            self.events.publish_terminal(&terminal);
        }
        drop(terminal);
        Ok(())
    }

    /// Scroll to the previous/next OSC 133 shell prompt. No-op (returns `Ok`) when shell integration
    /// recorded no reachable prompt in that direction, so the keybinding is silent without data.
    fn jump_to_prompt(&self, direction: TerminalJumpDirection) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let display_offset = terminal.grid().display_offset();
        let history_size = terminal.grid().history_size();
        let mut shell = self.events.shell_integration.lock();
        // Refresh the coordinate base from the live buffer before comparing, so output that landed
        // since the last published frame does not skew the viewport-top coordinate.
        shell.advance_scroll_base(history_size);
        let total_scrolled = shell.total_scrolled_lines;
        // Global coordinate of the line currently at the top of the viewport.
        let viewport_top_global = total_scrolled as i64 - display_offset as i64;
        let Some(target_global) = shell.jump_target(viewport_top_global, direction) else {
            return Ok(());
        };
        drop(shell);
        // Convert the target's stable global coordinate back to a current absolute grid Line
        // (relative to the active-screen top, negative in scrollback).
        let target_line = target_global as i64 - total_scrolled as i64;
        let target_point = Point::new(Line(target_line as i32), Column(0));
        let initial_display_offset = display_offset;
        let target_offset = active_match_display_offset(&terminal, target_point);
        if target_offset != initial_display_offset {
            terminal.scroll_display(Scroll::Delta(
                target_offset as i32 - initial_display_offset as i32,
            ));
            self.events.publish_terminal(&terminal);
        }
        drop(terminal);
        Ok(())
    }

    fn search(&self, request: TerminalSearchRequest) -> Result<TerminalSearchResult> {
        let mut terminal = self.terminal.lock();

        // An empty query clears any active search and highlights nothing.
        if request.query.is_empty() {
            *self.search_state.lock() = TerminalSearchState::default();
            drop(terminal);
            return Ok(TerminalSearchResult {
                active: None,
                matches: Vec::new(),
                index: 0,
                total: 0,
                revision: self.events.revision.load(Ordering::Acquire),
            });
        }

        let pattern = if request.regex {
            request.query.clone()
        } else {
            regex_escape(&request.query)
        };
        let mut regex = match RegexSearch::new(&pattern) {
            Ok(regex) => regex,
            // An invalid regex (or a literal that still fails to compile) simply matches nothing.
            Err(_) => {
                drop(terminal);
                return Ok(TerminalSearchResult {
                    active: None,
                    matches: Vec::new(),
                    index: 0,
                    total: 0,
                    revision: self.events.revision.load(Ordering::Acquire),
                });
            }
        };

        let direction = match request.direction {
            TerminalSearchDirection::Forward => Direction::Right,
            TerminalSearchDirection::Backward => Direction::Left,
        };

        // Decide whether we are continuing to navigate the same query or starting over. A changed
        // query/mode, an explicit `fresh` flag, or the very first search all reset the cursor.
        let previous_active = {
            let state = self.search_state.lock();
            let same_query = state.query == request.query && state.regex == request.regex;
            if request.fresh || !same_query {
                None
            } else {
                state.active.clone()
            }
        };

        let display_offset = terminal.grid().display_offset();
        let last_column = terminal.grid().last_column();
        let screen_lines = terminal.grid().screen_lines();

        // Choose the origin the search advances from.
        let origin = match &previous_active {
            Some(active) => {
                // Advance past the previous match's far edge so overlapping matches are not
                // re-found: forward moves one cell past the end, backward one cell before the start.
                match direction {
                    Direction::Right => active.end().add(&*terminal, Boundary::None, 1),
                    Direction::Left => active.start().sub(&*terminal, Boundary::None, 1),
                }
            }
            None => {
                // Start from the edge of the current viewport so the first match found is the one
                // closest to what the user is already looking at.
                let top = Line(-(display_offset as i32));
                let bottom = Line(screen_lines as i32 - 1 - display_offset as i32);
                match direction {
                    Direction::Right => Point::new(top, Column(0)),
                    Direction::Left => Point::new(bottom, last_column),
                }
            }
        };

        let Some(active_match) =
            terminal.search_next(&mut regex, origin, direction, Side::Left, None)
        else {
            // No match anywhere. Preserve the query so a later navigation keypress with the same
            // text does not resurrect a stale cursor.
            let mut state = self.search_state.lock();
            state.query = request.query.clone();
            state.regex = request.regex;
            state.active = None;
            drop(state);
            drop(terminal);
            return Ok(TerminalSearchResult {
                active: None,
                matches: Vec::new(),
                index: 0,
                total: 0,
                revision: self.events.revision.load(Ordering::Acquire),
            });
        };

        let active_start = *active_match.start();

        // Count every match across the whole buffer and find the active match's ordinal. The
        // terminal core caps regex complexity internally, so this stays bounded.
        let (total, index) = count_all_matches(&*terminal, &mut regex, active_start);

        // Remember where we are so the next navigation keypress advances from here.
        {
            let mut state = self.search_state.lock();
            state.query = request.query.clone();
            state.regex = request.regex;
            state.active = Some(active_match.clone());
        }

        // Scroll so the active match is on screen, if it is not already.
        let target_offset = active_match_display_offset(&*terminal, active_start);
        if target_offset != display_offset {
            let delta = target_offset as i32 - display_offset as i32;
            terminal.scroll_display(Scroll::Delta(delta));
            self.events.publish_terminal(&terminal);
        }

        // Collect every match that falls within the (possibly newly scrolled) viewport, in
        // viewport-relative coordinates, for highlighting.
        let final_offset = terminal.grid().display_offset();
        let columns = terminal.grid().columns();
        let matches = collect_viewport_matches(&*terminal, &mut regex, final_offset, screen_lines);
        let active =
            viewport_match_inner(active_match.clone(), final_offset, screen_lines, columns);

        let revision = self.events.revision.load(Ordering::Acquire);
        drop(terminal);
        Ok(TerminalSearchResult {
            active,
            matches,
            index,
            total,
            revision,
        })
    }

    /// Begin an interactive selection at a viewport cell. Converts the viewport point to a
    /// grid-absolute `Point` using the current scroll offset and stores it as the terminal core's
    /// authoritative selection, then republishes so the client sees the projected highlight.
    fn selection_start(
        &self,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
        kind: TerminalSelectionKind,
    ) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let absolute = viewport_point_to_absolute(&*terminal, point);
        let ty = match kind {
            TerminalSelectionKind::Simple => SelectionType::Simple,
            TerminalSelectionKind::Semantic => SelectionType::Semantic,
            TerminalSelectionKind::Lines => SelectionType::Lines,
            TerminalSelectionKind::Block => SelectionType::Block,
        };
        terminal.selection = Some(Selection::new(ty, absolute, selection_side(side)));
        self.events.publish_terminal(&terminal);
        Ok(())
    }

    /// Extend the active selection's head to a viewport cell (drag).
    fn selection_update(
        &self,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    ) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let absolute = viewport_point_to_absolute(&*terminal, point);
        if let Some(selection) = terminal.selection.as_mut() {
            selection.update(absolute, selection_side(side));
        }
        self.events.publish_terminal(&terminal);
        Ok(())
    }

    /// Clear the active selection.
    fn selection_clear(&self) -> Result<()> {
        let mut terminal = self.terminal.lock();
        if terminal.selection.take().is_some() {
            self.events.publish_terminal(&terminal);
        }
        Ok(())
    }

    /// Select the entire buffer including scrollback.
    fn select_all(&self) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let start = Point::new(terminal.topmost_line(), Column(0));
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.include_all();
        let end = Point::new(terminal.bottommost_line(), terminal.grid().last_column());
        selection.update(end, Side::Right);
        terminal.selection = Some(selection);
        self.events.publish_terminal(&terminal);
        Ok(())
    }

    /// Extract the current selection's text (whole scrollback span, soft-wrap unwrapped, trailing
    /// whitespace trimmed). Returns `None` when there is no active or non-empty selection.
    fn selection_text(&self) -> Result<Option<String>> {
        let terminal = self.terminal.lock();
        Ok(terminal.selection_to_string())
    }

    fn focus(&self, focused: bool) -> Result<()> {
        let terminal = self.terminal.lock();
        let report_focus = terminal.mode().contains(TermMode::FOCUS_IN_OUT);
        drop(terminal);
        if report_focus {
            let bytes = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.sender
                .send(Msg::Input(bytes.to_vec().into()))
                .context("failed to write terminal focus input")?;
        }
        Ok(())
    }

    fn resize(&self, size: TerminalSize) -> Result<()> {
        let mut terminal = self.terminal.lock();
        // Match the PTY publisher's terminal -> metadata lock ordering. Taking the size lock
        // first can deadlock when sustained output publishes while a resize is waiting.
        *self.events.size.write() = size;
        // Read the semantic phase and prompt-start line under the same terminal ->
        // shell_integration lock order used by `send_event`, so this can never deadlock. Together
        // they decide whether — and where — the active prompt region is cleared before reflow.
        // A column change reflows the whole buffer, invalidating every stored jump-point line
        // coordinate, so the index is dropped and rebuilt from subsequent prompts.
        let columns_changed = terminal.columns() != size.columns as usize;
        let (phase, prompt_start_line) = {
            let mut shell = self.events.shell_integration.lock();
            if columns_changed || terminal.mode().contains(TermMode::ALT_SCREEN) {
                shell.clear_jump_points();
            }
            (shell.phase, shell.prompt_start_line)
        };
        resize_terminal_with_history_reflow(&mut terminal, size, phase, prompt_start_line);
        terminal.set_kitty_graphics_cell_size(size.cell_width, size.cell_height);
        self.events.publish_terminal(&terminal);
        drop(terminal);
        self.sender
            .send(Msg::Resize(window_size(size)))
            .context("failed to resize PTY")
    }

    fn set_appearance(&self, appearance: TerminalAppearance) {
        *self.events.appearance.write() = appearance;
    }

    fn set_progress_timeouts(&self, timeouts: TerminalProgressTimeouts) {
        self.events.progress.set_timeouts(timeouts);
    }

    fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.events.snapshot()
    }

    fn image_chunk(
        &self,
        key: TerminalImageKey,
        offset: u32,
        length: u32,
    ) -> Result<(u32, u32, u32, Vec<u8>)> {
        let chunk = self.image_chunk_ref(key, offset, length)?;
        Ok((
            chunk.width,
            chunk.height,
            chunk.total_length,
            chunk.bytes().to_vec(),
        ))
    }

    fn image_chunk_ref(
        &self,
        key: TerminalImageKey,
        offset: u32,
        length: u32,
    ) -> Result<PublishedTerminalImageChunk> {
        // Clone the immutable Arc retained with the published snapshot. Reading the live terminal
        // here loses races with animated images and also blocks PTY parsing behind a multi-chunk
        // client transfer.
        let (width, height, pixels) = self
            .events
            .published_images
            .read()
            .get(&key)
            .map(|image| (image.width, image.height, Arc::clone(&image.pixels)))
            .ok_or_else(|| {
                anyhow!(
                    "unknown published terminal image {} generation {}",
                    key.id,
                    key.generation
                )
            })?;
        let total_length = u32::try_from(pixels.len()).context("terminal image is too large")?;
        let start = offset as usize;
        if start > pixels.len() {
            bail!("terminal image offset {offset} exceeds {total_length}");
        }
        let requested = (length as usize).clamp(1, MAX_IMAGE_CHUNK_SIZE);
        let end = start.saturating_add(requested).min(pixels.len());
        Ok(PublishedTerminalImageChunk {
            key,
            width,
            height,
            total_length,
            offset,
            pixels,
            end,
        })
    }

    fn wait_for_snapshot(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalSnapshotUpdate> {
        self.events
            .wait_for_revision(after_revision, timeout)
            .then(|| self.events.snapshot_after(after_revision))
    }

    fn wait_for_progress(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalProgressUpdate> {
        self.events.progress.wait_after(after_revision, timeout)
    }

    fn progress_revision(&self) -> u64 {
        self.events.progress.revision()
    }

    fn wait_for_osc_events(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalOscEventUpdate> {
        self.events.osc_events.wait_after(after_revision, timeout)
    }

    fn osc_event_revision(&self) -> u64 {
        self.events.osc_events.revision()
    }

    fn shell_integration_state(&self) -> TerminalShellIntegrationState {
        self.events.shell_integration.lock().snapshot()
    }

    fn complete_clipboard_read(&self, request_id: u64, contents: Vec<TerminalClipboardContent>) {
        self.events.complete_clipboard_read(request_id, contents);
    }

    fn notification_response(&self, notification_id: &str, activated: bool) {
        self.events
            .notification_response(notification_id, activated);
    }

    fn complete_file_transfer(&self, request_id: u64, destination: Option<PathBuf>) -> Result<()> {
        self.events.complete_file_transfer(request_id, destination)
    }

    fn set_osc_policy(&self, allow_clipboard_read: bool) {
        self.events
            .allow_clipboard_read
            .store(allow_clipboard_read, Ordering::Release);
    }

    fn set_url_detection(&self, detect_urls: bool) {
        let previous = self.events.detect_urls.swap(detect_urls, Ordering::AcqRel);
        if previous != detect_urls {
            // Republish so detected-link highlights appear (or clear) without waiting for the next
            // terminal output.
            self.events.publish_terminal(&self.terminal.lock());
        }
    }

    /// Set the session's default cursor shape. This changes only the *default* (`set_options`
    /// replaces the whole config, reconstructed from the same constants used at creation), so a
    /// program that has issued DECSCUSR keeps its runtime override — matching Ghostty's semantics.
    fn set_default_cursor_shape(&self, shape: TerminalCursorShape) {
        let mut terminal = self.terminal.lock();
        let config = terminal_config(kernel_cursor_shape(shape));
        terminal.set_options(config);
        self.events.publish_terminal(&terminal);
    }

    fn terminate(&self) {
        self.events.progress.report(None);
        let _ = self.sender.send(Msg::Shutdown);
    }
}

fn snapshot_terminal(
    terminal: &Term<DaemonEventListener>,
    session_id: SessionId,
    size: TerminalSize,
    title: String,
    revision: u64,
    last_input_sequence: u64,
) -> TerminalSnapshot {
    let graphics = terminal.kitty_graphics_snapshot();
    let content = terminal.renderable_content();
    let display_offset = content.display_offset;
    let cells = content
        .display_iter
        .filter_map(|indexed| {
            let point = point_to_viewport(display_offset, indexed.point)?;
            let cell = indexed.cell;
            if snapshot_cell_is_empty(cell) {
                return None;
            }
            Some(TerminalCell {
                line: point.line as u16,
                column: point.column.0 as u16,
                character: cell.c,
                zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
                foreground: snapshot_color(cell.fg),
                background: snapshot_color(cell.bg),
                underline_color: cell.underline_color().map(snapshot_color),
                hyperlink: cell.hyperlink().map(|hyperlink| hyperlink.uri().to_owned()),
                flags: cell.flags.bits(),
            })
        })
        .collect();
    let color_overrides = (0..COUNT)
        .filter_map(|index| {
            content.colors[index].map(|color| TerminalColorOverride {
                index: index as u16,
                color: rgba_from_rgb(color),
            })
        })
        .collect();
    let cursor_width = if terminal.grid()[content.cursor.point]
        .flags
        .contains(Flags::WIDE_CHAR)
    {
        2
    } else {
        1
    };
    let cursor_point = point_to_viewport(display_offset, content.cursor.point)
        .filter(|point| point.line < size.rows as usize);
    let selection = project_selection(terminal, display_offset, size);
    TerminalSnapshot {
        session_id,
        size,
        cells,
        color_overrides,
        cursor_line: cursor_point.map_or(0, |point| point.line as u16),
        cursor_column: cursor_point.map_or(0, |point| point.column.0 as u16),
        cursor_shape: cursor_point.map_or(TerminalCursorShape::Hidden, |_| {
            snapshot_cursor_shape(content.cursor.shape)
        }),
        cursor_width,
        // The kernel tracks the blinking bit alongside the shape (set by DECSCUSR or DEC Mode 12);
        // it is only meaningful while the cursor is on-screen. The UI's blink setting decides
        // whether to actually honor this.
        cursor_blinking: cursor_point.is_some() && terminal.cursor_style().blinking,
        title,
        revision,
        last_input_sequence,
        input_modes: terminal_input_modes(*terminal.mode()),
        images: graphics
            .images
            .into_iter()
            .map(|image| TerminalImageDescriptor {
                key: TerminalImageKey {
                    id: image.key.id,
                    generation: image.key.generation,
                },
                width: image.width,
                height: image.height,
            })
            .collect(),
        image_placements: graphics
            .placements
            .into_iter()
            .map(|placement| TerminalImagePlacement {
                image: TerminalImageKey {
                    id: placement.image.id,
                    generation: placement.image.generation,
                },
                placement_id: placement.placement_id,
                line: placement.line,
                column: placement.column,
                source_x: placement.source_x,
                source_y: placement.source_y,
                source_width: placement.source_width,
                source_height: placement.source_height,
                x_offset: placement.x_offset,
                y_offset: placement.y_offset,
                columns: placement.columns,
                rows: placement.rows,
                destination_width: placement.destination_width,
                destination_height: placement.destination_height,
                z: placement.z,
            })
            .collect(),
        selection,
        // Filled in by `ListenerState::detect_urls_into` after this snapshot is built.
        detected_links: Vec::new(),
    }
}

fn snapshot_delta(
    base: &TerminalSnapshot,
    current: &TerminalSnapshot,
) -> Option<TerminalSnapshotDelta> {
    if base.session_id != current.session_id
        || base.size != current.size
        || base.revision >= current.revision
    {
        return None;
    }
    let mut cells = Vec::new();
    let mut cleared = Vec::new();
    let mut base_index = 0;
    let mut current_index = 0;
    while base_index < base.cells.len() || current_index < current.cells.len() {
        let base_cell = base.cells.get(base_index);
        let current_cell = current.cells.get(current_index);
        let base_position = base_cell.map(|cell| (cell.line, cell.column));
        let current_position = current_cell.map(|cell| (cell.line, cell.column));
        match (base_position, current_position) {
            (Some(base_position), Some(current_position)) if base_position == current_position => {
                if base_cell != current_cell {
                    cells.push(current_cell.expect("current cell exists").clone());
                }
                base_index += 1;
                current_index += 1;
            }
            (Some(base_position), Some(current_position)) if base_position < current_position => {
                cleared.push(TerminalCellPosition {
                    line: base_position.0,
                    column: base_position.1,
                });
                base_index += 1;
            }
            (Some(_), Some(_)) => {
                cells.push(current_cell.expect("current cell exists").clone());
                current_index += 1;
            }
            (Some(base_position), None) => {
                cleared.push(TerminalCellPosition {
                    line: base_position.0,
                    column: base_position.1,
                });
                base_index += 1;
            }
            (None, Some(_)) => {
                cells.push(current_cell.expect("current cell exists").clone());
                current_index += 1;
            }
            (None, None) => break,
        }
    }

    if cells.len() + cleared.len() >= current.cells.len().max(1) {
        return None;
    }
    Some(TerminalSnapshotDelta {
        session_id: current.session_id,
        base_revision: base.revision,
        size: current.size,
        cells,
        cleared,
        color_overrides: current.color_overrides.clone(),
        cursor_line: current.cursor_line,
        cursor_column: current.cursor_column,
        cursor_shape: current.cursor_shape,
        cursor_width: current.cursor_width,
        cursor_blinking: current.cursor_blinking,
        title: current.title.clone(),
        revision: current.revision,
        last_input_sequence: current.last_input_sequence,
        input_modes: current.input_modes,
        images: (base.images != current.images).then(|| current.images.clone()),
        image_placements: (base.image_placements != current.image_placements)
            .then(|| current.image_placements.clone()),
        selection: current.selection,
        detected_links: current.detected_links.clone(),
    })
}

fn snapshot_cell_is_empty(cell: &Cell) -> bool {
    (cell.c == ' ' || cell.c == '\t')
        && cell.bg == Color::Named(NamedColor::Background)
        && cell.fg == Color::Named(NamedColor::Foreground)
        && !cell.flags.intersects(
            Flags::INVERSE
                | Flags::ALL_UNDERLINES
                | Flags::STRIKEOUT
                | Flags::WRAPLINE
                | Flags::WIDE_CHAR_SPACER
                | Flags::LEADING_WIDE_CHAR_SPACER,
        )
        && cell
            .zerowidth()
            .is_none_or(|zerowidth| zerowidth.is_empty())
}

/// Resize the terminal, protecting the active shell prompt/input region from a reflow that the
/// shell is about to redraw anyway.
///
/// Alacritty reflows the whole primary screen on a column change, tracking each logical line by
/// its `WRAPLINE` continuation markers. That is correct for completed command output, but the
/// active prompt/input region is a special case: after `SIGWINCH` the shell reprints its prompt
/// from scratch. Any copy of the prompt that resize leaves behind (or pushes into scrollback)
/// becomes a stale duplicate — the "duplicate prompt fragments" bug, most visible with multiline
/// prompts like Powerlevel10k where each resize sediments another copy into history.
///
/// When OSC 133 shell integration tells us the cursor is on a prompt/input region (`phase` is
/// [`TerminalSemanticPhase::Prompt`] or [`TerminalSemanticPhase::Input`]), we clear that region —
/// from the recorded prompt-start line down to the cursor — before resizing, blanking every cell
/// so no stale glyph or `WRAPLINE`/`WIDE_CHAR` continuation survives, and let the shell redraw it.
/// The cursor is left in place (like Ghostty's `clearCells`) so Alacritty's native resize tracks it
/// and the shell's redraw repaints from the same anchor. `prompt_start_line` comes from the cursor
/// position captured when the OSC 133 prompt-start marker arrived (mirroring Ghostty's approach),
/// so command output above the prompt is preserved.
///
/// In every other case (command output, no shell integration, or a row-only resize) we fall back
/// to Alacritty's native, reversible reflow — the same behavior Ghostty uses when it lacks
/// semantic information.
fn resize_terminal_with_history_reflow(
    terminal: &mut Term<DaemonEventListener>,
    size: TerminalSize,
    phase: TerminalSemanticPhase,
    prompt_start_line: Option<i32>,
) {
    let dimensions = GridSize(size);

    // Alternate-screen apps (vim, less, tmux, …) repaint themselves on resize, so neither
    // reflow nor prompt-clearing applies. Hand straight off to the native resize.
    if terminal.mode().contains(TermMode::ALT_SCREEN) {
        terminal.resize(dimensions);
        return;
    }

    let columns_changed = terminal.columns() != dimensions.columns();
    let cursor_on_prompt = matches!(
        phase,
        TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
    );

    // A wrap-reflow only happens on a column change; a row-only resize never reflows, so there is
    // nothing to protect the prompt from.
    if columns_changed && cursor_on_prompt {
        let grid = terminal.grid_mut();
        let old_columns = grid.columns();
        let cursor_line = grid.cursor.point.line.0;

        // Clear from the recorded prompt start (captured when OSC 133 A arrived) down to the cursor.
        // If we somehow have no recorded start, fall back to the cursor row alone rather than
        // guessing a wider span — clearing too little only risks a small fragment, while clearing
        // too much would wipe command output above the prompt.
        //
        // Clamp the start into the visible screen: scrollback above the prompt is settled history
        // and must not be touched, and the cursor row is always the lower bound.
        let start = prompt_start_line
            .unwrap_or(cursor_line)
            .clamp(0, cursor_line.max(0));

        for line in start..=cursor_line.max(0) {
            for column in 0..old_columns {
                grid[Line(line)][Column(column)] = Cell::default();
            }
        }

        terminal.resize(dimensions);
        return;
    }

    // Command output, no shell integration, or a row-only resize: native reflow is correct and
    // reversible.
    terminal.resize(dimensions);
}

fn descendant_processes(root_pid: u32, system: &mut System) -> Vec<ProcessInfo> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_cpu().with_memory(),
    );
    let all_processes = system
        .processes()
        .values()
        .map(|process| ProcessInfo {
            pid: process.pid().as_u32(),
            parent_pid: process.parent().map(|pid| pid.as_u32()),
            name: process.name().to_string_lossy().into_owned(),
            cpu_usage_tenths_percent: Some(cpu_usage_tenths_percent(process.cpu_usage())),
            memory_bytes: Some(process.memory()),
        })
        .collect::<Vec<_>>();
    filter_descendant_processes(root_pid, all_processes)
}

fn cpu_usage_tenths_percent(usage: f32) -> u32 {
    if usage.is_finite() && usage > 0. {
        (usage * 10.).round().min(u32::MAX as f32) as u32
    } else {
        0
    }
}

fn filter_descendant_processes(root_pid: u32, mut processes: Vec<ProcessInfo>) -> Vec<ProcessInfo> {
    processes.sort_by_key(|process| process.pid);
    let mut included = HashSet::from([root_pid]);
    let mut result = Vec::new();
    let mut pending = processes;
    loop {
        let mut changed = false;
        pending.retain(|process| {
            if process.pid == root_pid
                || process
                    .parent_pid
                    .is_some_and(|pid| included.contains(&pid))
            {
                included.insert(process.pid);
                result.push(process.clone());
                changed = true;
                false
            } else {
                true
            }
        });
        if !changed {
            break;
        }
    }
    result
}

fn listening_ports(processes: &[ProcessInfo]) -> Vec<ListeningPort> {
    if processes.is_empty() {
        return Vec::new();
    }
    let pids = processes
        .iter()
        .map(|process| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let lsof = if Path::new("/usr/sbin/lsof").exists() {
        "/usr/sbin/lsof"
    } else {
        "lsof"
    };
    let Ok(output) = Command::new(lsof)
        .args([
            "-nP",
            "-a",
            "-p",
            &pids,
            "-iTCP",
            "-sTCP:LISTEN",
            "-iUDP",
            "-FpcPn",
        ])
        .output()
    else {
        return Vec::new();
    };
    parse_lsof_ports(&String::from_utf8_lossy(&output.stdout))
}

fn parse_lsof_ports(output: &str) -> Vec<ListeningPort> {
    let mut current_pid = None;
    let mut current_protocol = None;
    let mut ports = Vec::new();
    for field in output.lines().filter(|line| line.len() >= 2) {
        let (kind, value) = field.split_at(1);
        match kind {
            "p" => current_pid = value.parse::<u32>().ok(),
            "f" => current_protocol = None,
            "P" => current_protocol = Some(value.to_owned()),
            "n" => {
                let Some(pid) = current_pid else {
                    continue;
                };
                let Some(protocol) = current_protocol.clone() else {
                    continue;
                };
                let endpoint = value.split("->").next().unwrap_or(value);
                let Some((address, port)) = endpoint.rsplit_once(':') else {
                    continue;
                };
                let Ok(port) = port.parse::<u16>() else {
                    continue;
                };
                ports.push(ListeningPort {
                    pid,
                    protocol,
                    address: address.to_owned(),
                    port,
                });
            }
            _ => {}
        }
    }
    ports.sort_by(|left, right| {
        (&left.protocol, &left.address, left.port, left.pid).cmp(&(
            &right.protocol,
            &right.address,
            right.port,
            right.pid,
        ))
    });
    ports.dedup_by(|left, right| left == right);
    ports
}

fn snapshot_color(color: Color) -> eggie_protocol::TerminalColor {
    use eggie_protocol::TerminalColor;

    match color {
        Color::Spec(color) => TerminalColor::Rgb(rgba_from_rgb(color)),
        Color::Indexed(index) => TerminalColor::Indexed(index),
        Color::Named(named) => TerminalColor::Named(named as u16),
    }
}

fn snapshot_cursor_shape(shape: CursorShape) -> TerminalCursorShape {
    match shape {
        CursorShape::Block => TerminalCursorShape::Block,
        CursorShape::Underline => TerminalCursorShape::Underline,
        CursorShape::Beam => TerminalCursorShape::Beam,
        CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
        CursorShape::Hidden => TerminalCursorShape::Hidden,
    }
}

/// Map a protocol cursor shape back to the terminal core's `CursorShape`. `Hidden` has no meaning as
/// a *default* shape (it would make the cursor permanently invisible), so it falls back to `Block`.
fn kernel_cursor_shape(shape: TerminalCursorShape) -> CursorShape {
    match shape {
        TerminalCursorShape::Block => CursorShape::Block,
        TerminalCursorShape::Underline => CursorShape::Underline,
        TerminalCursorShape::Beam => CursorShape::Beam,
        TerminalCursorShape::HollowBlock => CursorShape::HollowBlock,
        TerminalCursorShape::Hidden => CursorShape::Block,
    }
}

/// Build the terminal core `Config` for a session with the given default cursor shape. Kept in one
/// place so session creation and runtime `set_options` stay in sync on every other field.
fn terminal_config(default_cursor_shape: CursorShape) -> Config {
    Config {
        scrolling_history: TERMINAL_SCROLLBACK_LIMIT,
        kitty_keyboard: true,
        // Permission is enforced by ListenerState so it can be changed at runtime without
        // rebuilding the terminal. Reads remain denied by default.
        osc52: Osc52::CopyPaste,
        default_cursor_style: CursorStyle {
            shape: default_cursor_shape,
            blinking: false,
        },
        ..Config::default()
    }
}

/// Escape a literal search string so it can be compiled as a regex that matches it verbatim.
fn regex_escape(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(
            c,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        ) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Count every match in the whole buffer and return `(total, index_of_active)`. `index` is the
/// 0-based ordinal of the match starting at `active_start`; it defaults to 0 if that match is not
/// found during the walk (which should not happen for a match the caller just located).
fn count_all_matches<T>(
    terminal: &Term<T>,
    regex: &mut RegexSearch,
    active_start: Point,
) -> (usize, usize) {
    let start = Point::new(terminal.topmost_line(), Column(0));
    let end = Point::new(terminal.bottommost_line(), terminal.grid().last_column());
    let mut total = 0usize;
    let mut index = 0usize;
    for regex_match in RegexIter::new(start, end, Direction::Right, terminal, regex) {
        if *regex_match.start() == active_start {
            index = total;
        }
        total += 1;
    }
    (total, index)
}

/// The `display_offset` that brings the match starting at `active_start` fully on screen. If the
/// match is already visible the current offset is returned unchanged; otherwise the match line is
/// positioned near the top of the viewport.
fn active_match_display_offset<T>(terminal: &Term<T>, active_start: Point) -> usize {
    let display_offset = terminal.grid().display_offset();
    let screen_lines = terminal.grid().screen_lines() as i32;
    let history_size = terminal.grid().history_size();
    let viewport_line = active_start.line.0 + display_offset as i32;
    if viewport_line >= 0 && viewport_line < screen_lines {
        return display_offset;
    }
    // Place the match line at the viewport top: viewport_line == 0 => offset == -line.
    let target = (-active_start.line.0).clamp(0, history_size as i32);
    target as usize
}

/// Convert a viewport cell position into a grid-absolute point using the current scroll offset.
/// Viewport row `r` maps to absolute line `r - display_offset`.
fn viewport_point_to_absolute<T>(terminal: &Term<T>, point: TerminalCellPosition) -> Point {
    let display_offset = terminal.grid().display_offset() as i32;
    Point::new(
        Line(point.line as i32 - display_offset),
        Column(point.column as usize),
    )
}

/// Map a protocol selection side to the terminal core's `Side`.
fn selection_side(side: TerminalSelectionSide) -> Side {
    match side {
        TerminalSelectionSide::Left => Side::Left,
        TerminalSelectionSide::Right => Side::Right,
    }
}

/// Project the terminal core's authoritative selection into the current viewport. Returns `None`
/// when there is no selection or it is entirely scrolled out of view.
fn project_selection<T>(
    terminal: &Term<T>,
    display_offset: usize,
    size: TerminalSize,
) -> Option<TerminalSelectionRange> {
    let range = terminal.selection.as_ref()?.to_range(terminal)?;
    let is_block = range.is_block;
    let (start, end) = clamp_range_to_viewport(
        range.start..=range.end,
        display_offset,
        size.rows as usize,
        size.columns as usize,
    )?;
    Some(TerminalSelectionRange {
        start,
        end,
        is_block,
    })
}

/// Clamp a grid-absolute inclusive point range into the visible viewport rows, returning the
/// viewport-relative endpoints. Returns `None` if the range is entirely off screen. A clamped start
/// snaps to column 0 of the top row; a clamped end snaps to the last column of the bottom row, so a
/// highlight rectangle covers the visible portion. Shared by search-match and selection projection.
fn clamp_range_to_viewport(
    range: RangeInclusive<Point>,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Option<(TerminalCellPosition, TerminalCellPosition)> {
    if screen_lines == 0 || columns == 0 {
        return None;
    }
    let last_line = (screen_lines - 1) as i32;
    let last_column = (columns - 1) as u16;
    let start = *range.start();
    let end = *range.end();
    let start_line = start.line.0 + display_offset as i32;
    let end_line = end.line.0 + display_offset as i32;
    // Reject a range whose start is below the viewport or whose end is above it (fully off screen).
    if start_line > last_line || end_line < 0 {
        return None;
    }
    let (start_line, start_column) = if start_line < 0 {
        (0u16, 0u16)
    } else {
        (start_line as u16, start.column.0 as u16)
    };
    let (end_line, end_column) = if end_line > last_line {
        (last_line as u16, last_column)
    } else {
        (end_line as u16, end.column.0 as u16)
    };
    Some((
        TerminalCellPosition {
            line: start_line,
            column: start_column.min(last_column),
        },
        TerminalCellPosition {
            line: end_line,
            column: end_column.min(last_column),
        },
    ))
}

/// Convert a grid-absolute match into a viewport-relative highlight, clamping endpoints to the
/// visible rows. Returns `None` if the match start is below the viewport or the whole match is
/// above it. `columns` is the grid width, used to extend a clamped end to the row edge.
fn viewport_match_inner(
    regex_match: RangeInclusive<Point>,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> Option<TerminalSearchMatch> {
    let (start, end) =
        clamp_range_to_viewport(regex_match, display_offset, screen_lines, columns)?;
    Some(TerminalSearchMatch { start, end })
}

/// Collect all matches whose start falls within the current viewport rows, in viewport coordinates.
fn collect_viewport_matches<T>(
    terminal: &Term<T>,
    regex: &mut RegexSearch,
    display_offset: usize,
    screen_lines: usize,
) -> Vec<TerminalSearchMatch> {
    // Viewport rows map to grid lines [-display_offset, -display_offset + screen_lines).
    let top_line = Line(-(display_offset as i32));
    let bottom_line = Line(screen_lines as i32 - 1 - display_offset as i32);
    let start = Point::new(top_line, Column(0));
    let end = Point::new(bottom_line, terminal.grid().last_column());
    let columns = terminal.grid().columns();
    RegexIter::new(start, end, Direction::Right, terminal, regex)
        .filter_map(|regex_match| {
            viewport_match_inner(regex_match, display_offset, screen_lines, columns)
        })
        .collect()
}


fn rgba_from_rgb(color: Rgb) -> u32 {
    u32::from_be_bytes([color.r, color.g, color.b, 0xff])
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

fn terminal_mouse_mode(mode: TermMode) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::VI)
}

fn terminal_input_modes(mode: TermMode) -> TerminalInputModes {
    let mouse_tracking = if !terminal_mouse_mode(mode) {
        TerminalMouseTracking::Disabled
    } else if mode.contains(TermMode::MOUSE_MOTION) {
        TerminalMouseTracking::Motion
    } else if mode.contains(TermMode::MOUSE_DRAG) {
        TerminalMouseTracking::Drag
    } else {
        TerminalMouseTracking::Click
    };
    let mouse_encoding = if mode.contains(TermMode::SGR_MOUSE) {
        TerminalMouseEncoding::Sgr
    } else if mode.contains(TermMode::UTF8_MOUSE) {
        TerminalMouseEncoding::Utf8
    } else {
        TerminalMouseEncoding::Legacy
    };
    TerminalInputModes {
        mouse_tracking,
        mouse_encoding,
        focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
        alternate_screen: mode.contains(TermMode::ALT_SCREEN),
        alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        paste_events: mode.contains(TermMode::PASTE_EVENTS),
        kitty_keyboard_flags: ((mode & TermMode::KITTY_KEYBOARD_PROTOCOL).bits() >> 18) as u8,
    }
}

fn terminal_modifier_code(modifiers: TerminalModifiers) -> u8 {
    u8::from(modifiers.shift) * 4 + u8::from(modifiers.alt) * 8 + u8::from(modifiers.control) * 16
}

fn terminal_button_code(button: TerminalMouseButton) -> u8 {
    match button {
        TerminalMouseButton::Left => 0,
        TerminalMouseButton::Middle => 1,
        TerminalMouseButton::Right => 2,
    }
}

fn mouse_report_bytes(
    mode: TermMode,
    display_offset: usize,
    event: TerminalMouseEvent,
) -> Option<Vec<u8>> {
    if !terminal_mouse_mode(mode) {
        return None;
    }

    let (button, released) = match event.action {
        TerminalMouseAction::Press | TerminalMouseAction::Release => {
            if event.modifiers.shift {
                return None;
            }
            let button = terminal_button_code(event.button?);
            (button, event.action == TerminalMouseAction::Release)
        }
        TerminalMouseAction::Move => {
            if event.modifiers.shift
                && matches!(
                    event.button,
                    Some(TerminalMouseButton::Left | TerminalMouseButton::Right)
                )
            {
                return None;
            }
            if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
                return None;
            }
            let button = match event.button {
                Some(button) => terminal_button_code(button),
                None if mode.contains(TermMode::MOUSE_MOTION) => 3,
                None => return None,
            } + 32;
            (button, false)
        }
    };

    mouse_report_from_code(
        mode,
        display_offset,
        event.position,
        button,
        released,
        event.modifiers,
    )
}

fn mouse_report_from_code(
    mode: TermMode,
    display_offset: usize,
    position: TerminalMousePosition,
    button: u8,
    released: bool,
    modifiers: TerminalModifiers,
) -> Option<Vec<u8>> {
    let line = usize::from(position.row).checked_sub(display_offset)?;
    let column = usize::from(position.column);
    let modifier_code = terminal_modifier_code(modifiers);

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if released { 'm' } else { 'M' };
        let (x, y) = if mode.contains(TermMode::SGR_PIXEL_MOUSE) {
            (
                usize::try_from(position.pixel_x).ok()?.saturating_add(1),
                usize::try_from(position.pixel_y).ok()?.saturating_add(1),
            )
        } else {
            (column + 1, line + 1)
        };
        return Some(
            format!("\x1b[<{};{};{}{}", button + modifier_code, x, y, suffix).into_bytes(),
        );
    }

    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let max_point = if utf8 { 2015 } else { 223 };
    if line >= max_point || column >= max_point {
        return None;
    }

    let button = if released { 3 } else { button } + modifier_code;
    let mut report = vec![b'\x1b', b'[', b'M', 32 + button];
    encode_legacy_mouse_coordinate(&mut report, column, utf8);
    encode_legacy_mouse_coordinate(&mut report, line, utf8);
    Some(report)
}

fn encode_legacy_mouse_coordinate(report: &mut Vec<u8>, coordinate: usize, utf8: bool) {
    let coordinate = coordinate + 33;
    if utf8 && coordinate >= 128 {
        report.push((0xc0 + coordinate / 64) as u8);
        report.push((0x80 + (coordinate & 63)) as u8);
    } else {
        report.push(coordinate as u8);
    }
}

fn rgb_from_u32(color: u32) -> Rgb {
    Rgb {
        r: ((color >> 16) & 0xff) as u8,
        g: ((color >> 8) & 0xff) as u8,
        b: (color & 0xff) as u8,
    }
}

struct DaemonState {
    sessions: RwLock<HashMap<SessionId, Arc<TerminalSession>>>,
    build_id: Arc<str>,
}

impl DaemonState {
    fn handle(&self, request: ClientRequest) -> DaemonResponse {
        match self.try_handle(request) {
            Ok(response) => response,
            Err(error) => DaemonResponse::Error {
                message: format!("{error:#}"),
            },
        }
    }

    fn try_handle(&self, request: ClientRequest) -> Result<DaemonResponse> {
        match request {
            ClientRequest::Handshake { protocol_version } => {
                if protocol_version != PROTOCOL_VERSION {
                    bail!(
                        "protocol mismatch: client {protocol_version}, daemon {PROTOCOL_VERSION}"
                    );
                }
                Ok(DaemonResponse::HandshakeAccepted {
                    protocol_version: PROTOCOL_VERSION,
                    build_id: self.build_id.to_string(),
                })
            }
            ClientRequest::CreateSession {
                project_id,
                cwd,
                size,
                appearance,
            } => {
                let session = Arc::new(TerminalSession::spawn(project_id, cwd, size, appearance)?);
                let summary = session.summary();
                self.sessions.write().insert(session.id, session);
                Ok(DaemonResponse::SessionCreated { session: summary })
            }
            ClientRequest::ListSessions => Ok(DaemonResponse::Sessions {
                sessions: self
                    .sessions
                    .read()
                    .values()
                    .map(|session| session.summary())
                    .collect(),
            }),
            ClientRequest::InspectSession { session_id } => {
                let session = self.session(session_id)?;
                Ok(DaemonResponse::SessionInspection {
                    inspection: session.inspection(),
                })
            }
            ClientRequest::Snapshot { session_id } => {
                let session = self.session(session_id)?;
                Ok(DaemonResponse::Snapshot {
                    snapshot: session.snapshot(),
                })
            }
            ClientRequest::WaitForSnapshot {
                session_id,
                after_revision,
                timeout_ms,
            } => {
                let session = self.session(session_id)?;
                let timeout = Duration::from_millis(u64::from(timeout_ms)).min(MAX_SNAPSHOT_WAIT);
                Ok(match session.wait_for_snapshot(after_revision, timeout) {
                    Some(TerminalSnapshotUpdate::Full(snapshot)) => {
                        DaemonResponse::Snapshot { snapshot }
                    }
                    Some(TerminalSnapshotUpdate::Delta(delta)) => {
                        DaemonResponse::SnapshotDelta { delta }
                    }
                    None => DaemonResponse::SnapshotUnchanged {
                        revision: after_revision,
                    },
                })
            }
            ClientRequest::WaitForProgress {
                session_id,
                after_revision,
                timeout_ms,
            } => {
                let session = self.session(session_id)?;
                let timeout = Duration::from_millis(u64::from(timeout_ms)).min(MAX_SNAPSHOT_WAIT);
                Ok(match session.wait_for_progress(after_revision, timeout) {
                    Some(update) => DaemonResponse::Progress { update },
                    None => DaemonResponse::ProgressUnchanged {
                        session_id,
                        revision: session.progress_revision(),
                    },
                })
            }
            ClientRequest::WaitForOscEvents {
                session_id,
                after_revision,
                timeout_ms,
            } => {
                let session = self.session(session_id)?;
                let timeout = Duration::from_millis(u64::from(timeout_ms)).min(MAX_SNAPSHOT_WAIT);
                Ok(match session.wait_for_osc_events(after_revision, timeout) {
                    Some(update) => DaemonResponse::OscEvents { update },
                    None => DaemonResponse::OscEventsUnchanged {
                        session_id,
                        revision: session.osc_event_revision(),
                    },
                })
            }
            ClientRequest::GetShellIntegrationState { session_id } => {
                let session = self.session(session_id)?;
                Ok(DaemonResponse::ShellIntegrationState {
                    session_id,
                    state: session.shell_integration_state(),
                })
            }
            ClientRequest::CompleteClipboardRead {
                session_id,
                request_id,
                contents,
            } => {
                self.session(session_id)?
                    .complete_clipboard_read(request_id, contents);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::NotificationResponse {
                session_id,
                notification_id,
                activated,
            } => {
                self.session(session_id)?
                    .notification_response(&notification_id, activated);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::CompleteFileTransfer {
                session_id,
                request_id,
                destination,
            } => {
                self.session(session_id)?
                    .complete_file_transfer(request_id, destination)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalImage {
                session_id,
                key,
                offset,
                length,
            } => {
                let (width, height, total_length, bytes) =
                    self.session(session_id)?.image_chunk(key, offset, length)?;
                Ok(DaemonResponse::TerminalImage {
                    key,
                    width,
                    height,
                    total_length,
                    offset,
                    bytes,
                })
            }
            ClientRequest::Input {
                session_id,
                bytes,
                sequence,
            } => {
                self.session(session_id)?.input(bytes, sequence)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::Paste {
                session_id,
                text,
                sequence,
            } => {
                self.session(session_id)?.paste(text, sequence)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::PasteClipboard {
                session_id,
                selection,
                contents,
                sequence,
            } => {
                self.session(session_id)?
                    .paste_clipboard(selection, contents, sequence)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::Mouse { session_id, event } => {
                self.session(session_id)?.mouse(event)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::Scroll { session_id, event } => {
                self.session(session_id)?.scroll(event)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalScrollTo {
                session_id,
                command,
            } => {
                self.session(session_id)?.scroll_to(command)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalSearch {
                session_id,
                request,
            } => {
                let result = self.session(session_id)?.search(request)?;
                Ok(DaemonResponse::SearchResult { session_id, result })
            }
            ClientRequest::TerminalSelectionStart {
                session_id,
                point,
                side,
                kind,
            } => {
                self.session(session_id)?.selection_start(point, side, kind)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalSelectionUpdate {
                session_id,
                point,
                side,
            } => {
                self.session(session_id)?.selection_update(point, side)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalSelectionClear { session_id } => {
                self.session(session_id)?.selection_clear()?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalSelectAll { session_id } => {
                self.session(session_id)?.select_all()?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::TerminalCopySelection { session_id } => {
                let text = self.session(session_id)?.selection_text()?;
                Ok(DaemonResponse::SelectionText { session_id, text })
            }
            ClientRequest::Focus {
                session_id,
                focused,
            } => {
                self.session(session_id)?.focus(focused)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::Resize { session_id, size } => {
                self.session(session_id)?.resize(size)?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::SetAppearance {
                session_id,
                appearance,
            } => {
                self.session(session_id)?.set_appearance(appearance);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::SetProgressTimeouts {
                session_id,
                timeouts,
            } => {
                self.session(session_id)?.set_progress_timeouts(timeouts);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::SetOscPolicy {
                session_id,
                allow_clipboard_read,
            } => {
                self.session(session_id)?
                    .set_osc_policy(allow_clipboard_read);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::SetUrlDetection {
                session_id,
                detect_urls,
            } => {
                self.session(session_id)?.set_url_detection(detect_urls);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::SetCursorStyle { session_id, shape } => {
                self.session(session_id)?.set_default_cursor_shape(shape);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::Terminate { session_id } => {
                let Some(session) = self.sessions.write().remove(&session_id) else {
                    bail!("unknown terminal session {session_id}");
                };
                session.terminate();
                Ok(DaemonResponse::Ok)
            }
        }
    }

    fn session(&self, session_id: SessionId) -> Result<Arc<TerminalSession>> {
        self.sessions
            .read()
            .get(&session_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown terminal session {session_id}"))
    }
}

/// The shell command line and environment Eggie should launch a child session with.
struct ShellLaunch {
    args: Vec<String>,
    env: HashMap<String, String>,
}

/// Build the environment and argument list for a child shell, injecting OSC 133 shell integration
/// for the shells that support it. Pure (no I/O) so it can be unit-tested: the caller passes in the
/// already-installed `integration_root` and the user's current `ZDOTDIR`/`ENV`.
///
/// - zsh: point `ZDOTDIR` at our integration dir, preserving the user's original in
///   `EGGIE_ZDOTDIR_ORIG`. Keeps the existing `-l` login argument.
/// - bash: launch `--posix` with `ENV` pointing at our script (the POSIX-mode injection hook),
///   preserving the user's `ENV` in `EGGIE_BASH_ENV` and forwarding intercepted flags via
///   `EGGIE_BASH_INJECT`/`EGGIE_BASH_RCFILE`. Skipped for Apple's `/bin/bash` (3.2), which disables
///   the `ENV`-based POSIX startup path.
/// - anything else / no integration installed: base environment only; resize falls back to native
///   reflow.
fn build_shell_env(
    shell_name: &str,
    shell_path: &str,
    terminfo: &Path,
    integration_root: Option<&Path>,
    user_zdotdir: Option<String>,
    user_env_var: Option<String>,
) -> ShellLaunch {
    let mut env = HashMap::new();
    env.insert("TERM".to_owned(), "alacritty".to_owned());
    env.insert(
        "TERMINFO".to_owned(),
        terminfo.to_string_lossy().into_owned(),
    );
    env.insert("COLORTERM".to_owned(), "truecolor".to_owned());
    env.insert("TERM_PROGRAM".to_owned(), "Eggie".to_owned());
    env.insert(
        "TERM_PROGRAM_VERSION".to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );

    let mut args = vec!["-l".to_owned()];

    match (shell_name, integration_root) {
        ("zsh", Some(root)) => {
            if let Some(original) = user_zdotdir {
                env.insert("EGGIE_ZDOTDIR_ORIG".to_owned(), original);
            }
            env.insert(
                "ZDOTDIR".to_owned(),
                root.join("zsh").to_string_lossy().into_owned(),
            );
        }
        // Apple's /bin/bash is 3.2 and does not honor the ENV-based POSIX startup path, so
        // integration is impossible there; fall through to no injection.
        ("bash", Some(root)) if shell_path != "/bin/bash" || !cfg!(target_os = "macos") => {
            args = vec!["--posix".to_owned()];
            if let Some(original) = user_env_var {
                env.insert("EGGIE_BASH_ENV".to_owned(), original);
            }
            env.insert(
                "ENV".to_owned(),
                root.join("bash")
                    .join("eggie.bash")
                    .to_string_lossy()
                    .into_owned(),
            );
            // "1" marks an automatic (Eggie) injection; the script also reads any forwarded flags
            // from here. We do not intercept --norc/--rcfile from SHELL today, so this is just "1".
            env.insert("EGGIE_BASH_INJECT".to_owned(), "1".to_owned());
        }
        _ => {}
    }

    ShellLaunch { args, env }
}

fn install_bundled_terminfo() -> Result<PathBuf> {
    match INSTALLED_TERMINFO
        .get_or_init(|| install_bundled_terminfo_inner().map_err(|error| format!("{error:#}")))
    {
        Ok(path) => Ok(path.clone()),
        Err(error) => bail!("failed to install bundled terminfo: {error}"),
    }
}

fn install_bundled_terminfo_inner() -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let database = std::env::temp_dir()
        .join(format!("eggie-{uid}"))
        .join("terminfo-v1");
    let directory = database.join("61");
    let entry = directory.join("alacritty");
    fs::create_dir_all(&directory)?;
    fs::set_permissions(&database, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;

    write_if_changed(&entry, BUNDLED_ALACRITTY_TERMINFO)?;
    Ok(database)
}

/// Write `contents` to `path` (mode 0600) only when it differs from what is already there, via a
/// per-process temporary file and an atomic rename so concurrent sessions never observe a partial
/// file. Shared by the terminfo and shell-integration installers.
fn write_if_changed(path: &Path, contents: &[u8]) -> Result<()> {
    if fs::read(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    let directory = path
        .parent()
        .ok_or_else(|| anyhow!("cannot install {}: no parent directory", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("cannot install {}: invalid file name", path.display()))?;
    let temporary = directory.join(format!("{file_name}.tmp-{}", process::id()));
    fs::write(&temporary, contents)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Install the bundled shell-integration scripts into a per-user runtime directory and return its
/// root. The zsh subdirectory becomes `ZDOTDIR`; the bash script under it becomes `ENV`.
fn install_bundled_shell_integration() -> Result<PathBuf> {
    match INSTALLED_SHELL_INTEGRATION.get_or_init(|| {
        install_bundled_shell_integration_inner().map_err(|error| format!("{error:#}"))
    }) {
        Ok(path) => Ok(path.clone()),
        Err(error) => bail!("failed to install bundled shell integration: {error}"),
    }
}

fn install_bundled_shell_integration_inner() -> Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let root = std::env::temp_dir()
        .join(format!("eggie-{uid}"))
        .join("shell-integration-v1");
    let zsh = root.join("zsh");
    let bash = root.join("bash");
    fs::create_dir_all(&zsh)?;
    fs::create_dir_all(&bash)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&zsh, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&bash, fs::Permissions::from_mode(0o700))?;

    // The dotfile name is required verbatim so zsh's ZDOTDIR mechanism finds it.
    write_if_changed(&zsh.join(".zshenv"), BUNDLED_ZSH_ZSHENV)?;
    write_if_changed(&zsh.join("eggie-integration"), BUNDLED_ZSH_INTEGRATION)?;
    write_if_changed(&bash.join("eggie.bash"), BUNDLED_BASH_INTEGRATION)?;
    Ok(root)
}

pub fn daemon_socket_path() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    std::env::temp_dir()
        .join(format!("eggie-{uid}"))
        .join(format!("daemon-v{PROTOCOL_VERSION}.sock"))
}

pub fn is_daemon_invocation(arguments: &[String]) -> Option<PathBuf> {
    (arguments.get(1).map(String::as_str) == Some(DAEMON_ARGUMENT)).then(|| {
        arguments
            .get(2)
            .map(PathBuf::from)
            .unwrap_or_else(daemon_socket_path)
    })
}

pub fn run_daemon(socket_path: &Path, build_id: &str) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind daemon socket {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    let state = Arc::new(DaemonState {
        sessions: RwLock::new(HashMap::new()),
        build_id: Arc::from(build_id),
    });

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || {
                    if let Err(error) = serve_connection(stream, &state) {
                        eprintln!("Eggie daemon connection error: {error:#}");
                    }
                });
            }
            Err(error) => eprintln!("Eggie daemon accept error: {error}"),
        }
    }
    Ok(())
}

fn serve_connection(stream: UnixStream, state: &DaemonState) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let mut request = Vec::with_capacity(512);
    let mut response = Vec::with_capacity(8 * 1024);
    let mut latency_sensitive = false;
    loop {
        let Some(request) = read_wire_message::<ClientRequest>(&mut stream, &mut request)
            .context("failed to read request")?
        else {
            return Ok(());
        };
        if !latency_sensitive
            && matches!(
                &request,
                ClientRequest::WaitForSnapshot { .. }
                    | ClientRequest::WaitForProgress { .. }
                    | ClientRequest::WaitForOscEvents { .. }
                    | ClientRequest::TerminalImage { .. }
                    | ClientRequest::Input { .. }
                    | ClientRequest::Paste { .. }
                    | ClientRequest::PasteClipboard { .. }
                    | ClientRequest::Mouse { .. }
                    | ClientRequest::Scroll { .. }
                    | ClientRequest::Focus { .. }
                    | ClientRequest::TerminalSelectionStart { .. }
                    | ClientRequest::TerminalSelectionUpdate { .. }
                    | ClientRequest::TerminalSelectionClear { .. }
            )
        {
            configure_latency_sensitive_thread();
            latency_sensitive = true;
        }
        match request {
            ClientRequest::TerminalImage {
                session_id,
                key,
                offset,
                length,
            } => match state
                .session(session_id)
                .and_then(|session| session.image_chunk_ref(key, offset, length))
            {
                Ok(chunk) => write_terminal_image_wire(stream.get_mut(), &chunk)?,
                Err(error) => write_wire_message(
                    stream.get_mut(),
                    &mut response,
                    &DaemonResponse::Error {
                        message: format!("{error:#}"),
                    },
                )?,
            },
            request => write_wire_message(stream.get_mut(), &mut response, &state.handle(request))?,
        }
    }
}

fn read_wire_message<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
) -> Result<Option<T>> {
    let Some(header) = read_wire_header(reader)? else {
        return Ok(None);
    };
    if header & RAW_TERMINAL_IMAGE_WIRE_FLAG != 0 {
        bail!("unexpected raw terminal image wire response");
    }
    read_message_body(reader, buffer, header as usize).map(Some)
}

fn read_wire_header(reader: &mut impl Read) -> Result<Option<u32>> {
    let mut header = [0_u8; 4];
    match reader.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => {}
        Err(error) => return Err(error.into()),
    }
    reader.read_exact(&mut header[1..])?;
    Ok(Some(u32::from_le_bytes(header)))
}

fn read_message_body<T: serde::de::DeserializeOwned>(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
    length: usize,
) -> Result<T> {
    if length > MAX_WIRE_MESSAGE_SIZE {
        bail!("daemon wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {length}");
    }
    buffer.resize(length, 0);
    reader.read_exact(buffer)?;
    let value = rmp_serde::from_slice(buffer).context("invalid daemon wire message")?;
    Ok(value)
}

fn write_wire_message(
    writer: &mut impl Write,
    buffer: &mut Vec<u8>,
    value: &impl serde::Serialize,
) -> Result<()> {
    buffer.clear();
    rmp_serde::encode::write_named(&mut *buffer, value)
        .context("failed to encode daemon wire message")?;
    if buffer.len() > MAX_WIRE_MESSAGE_SIZE {
        bail!(
            "daemon wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {}",
            buffer.len()
        );
    }
    let length = u32::try_from(buffer.len()).context("daemon wire message is too large")?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(buffer)?;
    Ok(())
}

fn write_terminal_image_wire(
    writer: &mut impl Write,
    chunk: &PublishedTerminalImageChunk,
) -> Result<()> {
    let bytes = chunk.bytes();
    let payload_length = RAW_TERMINAL_IMAGE_METADATA_SIZE
        .checked_add(bytes.len())
        .context("terminal image wire message length overflow")?;
    if payload_length > MAX_WIRE_MESSAGE_SIZE {
        bail!(
            "terminal image wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {payload_length}"
        );
    }
    let payload_length =
        u32::try_from(payload_length).context("terminal image chunk is too large")?;
    let mut metadata = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    metadata[0..4].copy_from_slice(&chunk.key.id.to_le_bytes());
    metadata[4..12].copy_from_slice(&chunk.key.generation.to_le_bytes());
    metadata[12..16].copy_from_slice(&chunk.width.to_le_bytes());
    metadata[16..20].copy_from_slice(&chunk.height.to_le_bytes());
    metadata[20..24].copy_from_slice(&chunk.total_length.to_le_bytes());
    metadata[24..28].copy_from_slice(&chunk.offset.to_le_bytes());
    writer.write_all(&(payload_length | RAW_TERMINAL_IMAGE_WIRE_FLAG).to_le_bytes())?;
    writer.write_all(&metadata)?;
    writer.write_all(bytes)?;
    Ok(())
}

fn read_terminal_image_wire_into(
    reader: &mut impl Read,
    payload_length: usize,
    destination: &mut Vec<u8>,
) -> Result<TerminalImageChunkMetadata> {
    if !(RAW_TERMINAL_IMAGE_METADATA_SIZE..=MAX_WIRE_MESSAGE_SIZE).contains(&payload_length) {
        bail!("invalid raw terminal image wire length: {payload_length}");
    }
    let mut metadata = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    reader.read_exact(&mut metadata)?;
    let read_u32 = |offset: usize| {
        u32::from_le_bytes(metadata[offset..offset + 4].try_into().expect("four bytes"))
    };
    let generation = u64::from_le_bytes(metadata[4..12].try_into().expect("eight bytes"));
    let chunk_length = payload_length - RAW_TERMINAL_IMAGE_METADATA_SIZE;
    let chunk_length = u32::try_from(chunk_length).context("terminal image chunk is too large")?;
    let original_length = destination.len();
    destination.resize(original_length + chunk_length as usize, 0);
    if let Err(error) = reader.read_exact(&mut destination[original_length..]) {
        destination.truncate(original_length);
        return Err(error.into());
    }
    Ok(TerminalImageChunkMetadata {
        key: TerminalImageKey {
            id: read_u32(0),
            generation,
        },
        width: read_u32(12),
        height: read_u32(16),
        total_length: read_u32(20),
        offset: read_u32(24),
        chunk_length,
    })
}

fn read_daemon_response(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
) -> Result<Option<DaemonResponse>> {
    let Some(header) = read_wire_header(reader)? else {
        return Ok(None);
    };
    if header & RAW_TERMINAL_IMAGE_WIRE_FLAG == 0 {
        return read_message_body(reader, buffer, header as usize).map(Some);
    }
    let mut bytes = Vec::new();
    let metadata = read_terminal_image_wire_into(
        reader,
        (header & !RAW_TERMINAL_IMAGE_WIRE_FLAG) as usize,
        &mut bytes,
    )?;
    Ok(Some(DaemonResponse::TerminalImage {
        key: metadata.key,
        width: metadata.width,
        height: metadata.height,
        total_length: metadata.total_length,
        offset: metadata.offset,
        bytes,
    }))
}

#[cfg(target_os = "macos")]
fn configure_latency_sensitive_thread() {
    unsafe {
        // QOS_CLASS_USER_INTERACTIVE is stable in Darwin's pthread ABI but is not exported by
        // every libc crate release supported by this workspace.
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_latency_sensitive_thread() {}

#[derive(Clone)]
pub struct DaemonClient {
    socket_path: Arc<PathBuf>,
    build_id: Arc<str>,
}

pub struct DaemonConnection {
    stream: BufReader<UnixStream>,
    request: Vec<u8>,
    response: Vec<u8>,
}

#[derive(Clone)]
pub struct DaemonInputSender {
    sender: mpsc::Sender<QueuedTerminalInput>,
}

#[derive(Debug)]
enum QueuedTerminalInput {
    Input {
        session_id: SessionId,
        bytes: Vec<u8>,
        sequence: u64,
    },
    Paste {
        session_id: SessionId,
        text: String,
        sequence: u64,
    },
    PasteClipboard {
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
    },
    Mouse {
        session_id: SessionId,
        event: TerminalMouseEvent,
    },
    Scroll {
        session_id: SessionId,
        event: TerminalScrollEvent,
    },
    Focus {
        session_id: SessionId,
        focused: bool,
    },
    SelectionStart {
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
        kind: TerminalSelectionKind,
    },
    SelectionUpdate {
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    },
    SelectionClear {
        session_id: SessionId,
    },
}

impl QueuedTerminalInput {
    fn merge(&mut self, next: Self) -> std::result::Result<(), Self> {
        match (self, next) {
            (
                Self::Input {
                    session_id,
                    bytes,
                    sequence,
                },
                Self::Input {
                    session_id: next_session_id,
                    bytes: next_bytes,
                    sequence: next_sequence,
                },
            ) if *session_id == next_session_id => {
                bytes.extend(next_bytes);
                *sequence = next_sequence;
                Ok(())
            }
            (
                Self::Mouse { session_id, event },
                Self::Mouse {
                    session_id: next_session_id,
                    event: next_event,
                },
            ) if *session_id == next_session_id
                && event.action == TerminalMouseAction::Move
                && next_event.action == TerminalMouseAction::Move =>
            {
                *event = next_event;
                Ok(())
            }
            (
                Self::Scroll { session_id, event },
                Self::Scroll {
                    session_id: next_session_id,
                    event: next_event,
                },
            ) if *session_id == next_session_id
                && event.phase == TerminalScrollPhase::Moved
                && next_event.phase == TerminalScrollPhase::Moved
                && event.delta.unit == next_event.delta.unit
                && event.modifiers == next_event.modifiers
                && (event.delta.x == 0
                    || next_event.delta.x == 0
                    || event.delta.x.signum() == next_event.delta.x.signum())
                && (event.delta.y == 0
                    || next_event.delta.y == 0
                    || event.delta.y.signum() == next_event.delta.y.signum()) =>
            {
                event.delta.x = event.delta.x.saturating_add(next_event.delta.x);
                event.delta.y = event.delta.y.saturating_add(next_event.delta.y);
                event.position = next_event.position;
                Ok(())
            }
            (
                Self::Focus {
                    session_id,
                    focused,
                },
                Self::Focus {
                    session_id: next_session_id,
                    focused: next_focused,
                },
            ) if *session_id == next_session_id && *focused == next_focused => Ok(()),
            (
                Self::SelectionUpdate {
                    session_id,
                    point,
                    side,
                },
                Self::SelectionUpdate {
                    session_id: next_session_id,
                    point: next_point,
                    side: next_side,
                },
            ) if *session_id == next_session_id => {
                // A fast drag emits many head updates; only the latest matters.
                *point = next_point;
                *side = next_side;
                Ok(())
            }
            (_, next) => Err(next),
        }
    }

    fn request(self) -> ClientRequest {
        match self {
            Self::Input {
                session_id,
                bytes,
                sequence,
            } => ClientRequest::Input {
                session_id,
                bytes,
                sequence,
            },
            Self::Paste {
                session_id,
                text,
                sequence,
            } => ClientRequest::Paste {
                session_id,
                text,
                sequence,
            },
            Self::PasteClipboard {
                session_id,
                selection,
                contents,
                sequence,
            } => ClientRequest::PasteClipboard {
                session_id,
                selection,
                contents,
                sequence,
            },
            Self::Mouse { session_id, event } => ClientRequest::Mouse { session_id, event },
            Self::Scroll { session_id, event } => ClientRequest::Scroll { session_id, event },
            Self::Focus {
                session_id,
                focused,
            } => ClientRequest::Focus {
                session_id,
                focused,
            },
            Self::SelectionStart {
                session_id,
                point,
                side,
                kind,
            } => ClientRequest::TerminalSelectionStart {
                session_id,
                point,
                side,
                kind,
            },
            Self::SelectionUpdate {
                session_id,
                point,
                side,
            } => ClientRequest::TerminalSelectionUpdate {
                session_id,
                point,
                side,
            },
            Self::SelectionClear { session_id } => {
                ClientRequest::TerminalSelectionClear { session_id }
            }
        }
    }
}

fn receive_input_batch(
    receiver: &mpsc::Receiver<QueuedTerminalInput>,
    first: QueuedTerminalInput,
) -> Vec<QueuedTerminalInput> {
    let mut queued = vec![first];
    for _ in 1..MAX_INPUT_BATCH_MESSAGES {
        let Ok(next) = receiver.try_recv() else {
            break;
        };
        let next = match queued
            .last_mut()
            .expect("input batch is not empty")
            .merge(next)
        {
            Ok(()) => continue,
            Err(next) => next,
        };
        queued.push(next);
    }
    queued
}

impl DaemonConnection {
    pub fn request(&mut self, request: ClientRequest) -> Result<DaemonResponse> {
        write_wire_message(self.stream.get_mut(), &mut self.request, &request)?;
        let Some(response) = read_daemon_response(&mut self.stream, &mut self.response)? else {
            bail!("daemon closed the persistent connection");
        };
        if let DaemonResponse::Error { message } = &response {
            bail!("daemon request failed: {message}");
        }
        Ok(response)
    }

    pub fn append_terminal_image_chunk(
        &mut self,
        session_id: SessionId,
        key: TerminalImageKey,
        offset: u32,
        length: u32,
        destination: &mut Vec<u8>,
    ) -> Result<TerminalImageChunkMetadata> {
        write_wire_message(
            self.stream.get_mut(),
            &mut self.request,
            &ClientRequest::TerminalImage {
                session_id,
                key,
                offset,
                length,
            },
        )?;
        let Some(header) = read_wire_header(&mut self.stream)? else {
            bail!("daemon closed the persistent connection");
        };
        if header & RAW_TERMINAL_IMAGE_WIRE_FLAG != 0 {
            return read_terminal_image_wire_into(
                &mut self.stream,
                (header & !RAW_TERMINAL_IMAGE_WIRE_FLAG) as usize,
                destination,
            );
        }

        let response = read_message_body::<DaemonResponse>(
            &mut self.stream,
            &mut self.response,
            header as usize,
        )?;
        match response {
            DaemonResponse::TerminalImage {
                key,
                width,
                height,
                total_length,
                offset,
                bytes,
            } => {
                let chunk_length = u32::try_from(bytes.len())
                    .context("terminal image fallback chunk is too large")?;
                destination.extend_from_slice(&bytes);
                Ok(TerminalImageChunkMetadata {
                    key,
                    width,
                    height,
                    total_length,
                    offset,
                    chunk_length,
                })
            }
            DaemonResponse::Error { message } => bail!("daemon request failed: {message}"),
            response => bail!("unexpected terminal image response: {response:?}"),
        }
    }
}

impl DaemonInputSender {
    pub fn send_input(&self, session_id: SessionId, bytes: Vec<u8>, sequence: u64) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::Input {
                session_id,
                bytes,
                sequence,
            })
            .context("terminal input worker stopped")
    }

    pub fn send_paste(&self, session_id: SessionId, text: String, sequence: u64) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::Paste {
                session_id,
                text,
                sequence,
            })
            .context("terminal input worker stopped")
    }

    pub fn send_paste_clipboard(
        &self,
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
    ) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::PasteClipboard {
                session_id,
                selection,
                contents,
                sequence,
            })
            .context("terminal input worker stopped")
    }

    pub fn send_mouse(&self, session_id: SessionId, event: TerminalMouseEvent) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::Mouse { session_id, event })
            .context("terminal input worker stopped")
    }

    pub fn send_scroll(&self, session_id: SessionId, event: TerminalScrollEvent) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::Scroll { session_id, event })
            .context("terminal input worker stopped")
    }

    pub fn send_focus(&self, session_id: SessionId, focused: bool) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::Focus {
                session_id,
                focused,
            })
            .context("terminal input worker stopped")
    }

    pub fn send_selection_start(
        &self,
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
        kind: TerminalSelectionKind,
    ) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::SelectionStart {
                session_id,
                point,
                side,
                kind,
            })
            .context("terminal input worker stopped")
    }

    pub fn send_selection_update(
        &self,
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    ) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::SelectionUpdate {
                session_id,
                point,
                side,
            })
            .context("terminal input worker stopped")
    }

    pub fn send_selection_clear(&self, session_id: SessionId) -> Result<()> {
        self.sender
            .send(QueuedTerminalInput::SelectionClear { session_id })
            .context("terminal input worker stopped")
    }
}

fn terminate_daemon_at(socket_path: &Path) {
    if let Ok(stream) = UnixStream::connect(socket_path)
        && let Some(pid) = unix_peer_pid(&stream)
        && pid > 1
        && pid != process::id() as libc::pid_t
    {
        // The socket directory is private to this uid. Resolve the peer from the live connection
        // instead of trusting a PID file, so an obsolete protocol daemon cannot survive rebuilds
        // and a reused process id can never be signalled accidentally.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
}

impl DaemonClient {
    pub fn new(socket_path: PathBuf, build_id: impl Into<Arc<str>>) -> Self {
        Self {
            socket_path: Arc::new(socket_path),
            build_id: build_id.into(),
        }
    }

    pub fn connect_default(build_id: impl Into<Arc<str>>) -> Result<Self> {
        let client = Self::new(daemon_socket_path(), build_id);
        client.terminate_obsolete_daemons();
        client.ensure_running()?;
        Ok(client)
    }

    pub fn ensure_running(&self) -> Result<()> {
        if self.handshake().is_ok() {
            return Ok(());
        }
        self.terminate_stale_daemon();
        let executable = std::env::current_exe().context("cannot locate Eggie executable")?;
        let mut command = Command::new(executable);
        command
            .arg(DAEMON_ARGUMENT)
            .arg(self.socket_path.as_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(if render_metrics_enabled() {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .process_group(0);
        command.spawn().context("failed to start Eggie daemon")?;

        let started = Instant::now();
        while started.elapsed() < CONNECT_TIMEOUT {
            if self.handshake().is_ok() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        bail!("Eggie daemon did not become ready within {CONNECT_TIMEOUT:?}")
    }

    fn terminate_stale_daemon(&self) {
        terminate_daemon_at(self.socket_path.as_path());
    }

    fn terminate_obsolete_daemons(&self) {
        let Some(directory) = self.socket_path.parent() else {
            return;
        };
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == self.socket_path.as_path() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with("daemon-v") && name.ends_with(".sock") {
                terminate_daemon_at(&path);
            }
        }
    }

    pub fn handshake(&self) -> Result<()> {
        match self.request(ClientRequest::Handshake {
            protocol_version: PROTOCOL_VERSION,
        })? {
            DaemonResponse::HandshakeAccepted {
                protocol_version,
                build_id,
            } if protocol_version == PROTOCOL_VERSION && build_id == self.build_id.as_ref() => {
                Ok(())
            }
            response => bail!("unexpected handshake response: {response:?}"),
        }
    }

    pub fn connect(&self) -> Result<DaemonConnection> {
        let stream = UnixStream::connect(self.socket_path.as_path())
            .with_context(|| format!("failed to connect to {}", self.socket_path.display()))?;
        Ok(DaemonConnection {
            stream: BufReader::new(stream),
            request: Vec::with_capacity(512),
            response: Vec::with_capacity(8 * 1024),
        })
    }

    pub fn input_sender(&self) -> Result<DaemonInputSender> {
        let (sender, receiver) = mpsc::channel::<QueuedTerminalInput>();
        let client = self.clone();
        thread::Builder::new()
            .name("eggie-terminal-input".to_owned())
            .spawn(move || {
                configure_latency_sensitive_thread();
                let mut connection = None;
                while let Ok(first) = receiver.recv() {
                    for input in receive_input_batch(&receiver, first) {
                        if connection.is_none() {
                            match client.connect() {
                                Ok(connected) => connection = Some(connected),
                                Err(error) => {
                                    eprintln!("failed to connect terminal input stream: {error:#}");
                                    continue;
                                }
                            }
                        }
                        if let Err(error) = connection
                            .as_mut()
                            .expect("input connection was initialized")
                            .request(input.request())
                        {
                            eprintln!("failed to dispatch terminal input: {error:#}");
                            connection = None;
                        }
                    }
                }
            })
            .context("failed to spawn terminal input worker")?;
        Ok(DaemonInputSender { sender })
    }

    pub fn request(&self, request: ClientRequest) -> Result<DaemonResponse> {
        self.connect()?.request(request)
    }
}

#[cfg(target_os = "macos")]
fn unix_peer_pid(stream: &UnixStream) -> Option<libc::pid_t> {
    let mut pid = 0 as libc::pid_t;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    (result == 0 && length as usize == std::mem::size_of_val(&pid)).then_some(pid)
}

#[cfg(not(target_os = "macos"))]
fn unix_peer_pid(_stream: &UnixStream) -> Option<libc::pid_t> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_socket_is_scoped_by_user_and_protocol() {
        let path = daemon_socket_path();
        assert!(path.to_string_lossy().contains("eggie-"));
        assert!(
            path.to_string_lossy()
                .contains(&format!("daemon-v{PROTOCOL_VERSION}.sock"))
        );
    }

    #[test]
    fn default_client_removes_only_obsolete_protocol_sockets() {
        let directory = std::env::temp_dir().join(format!(
            "eggie-daemon-cleanup-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let current = directory.join(format!("daemon-v{PROTOCOL_VERSION}.sock"));
        let obsolete = directory.join("daemon-v1.sock");
        let unrelated = directory.join("keep-me.sock");
        fs::write(&current, []).unwrap();
        fs::write(&obsolete, []).unwrap();
        fs::write(&unrelated, []).unwrap();

        DaemonClient::new(current.clone(), "test").terminate_obsolete_daemons();

        assert!(current.exists());
        assert!(!obsolete.exists());
        assert!(unrelated.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn raw_terminal_image_wire_appends_pixels_without_messagepack_copying() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let pixels = Arc::new(PixelBuffer::from_vec(
            (0..=255).cycle().take(1024 * 1024).collect::<Vec<_>>(),
        ));
        let chunk = PublishedTerminalImageChunk {
            key: TerminalImageKey {
                id: 42,
                generation: 7,
            },
            width: 512,
            height: 512,
            total_length: pixels.len() as u32,
            offset: 128,
            end: pixels.len(),
            pixels: Arc::clone(&pixels),
        };
        let expected = chunk.bytes().to_vec();
        let writer_thread = thread::spawn(move || write_terminal_image_wire(&mut writer, &chunk));

        let mut reader = BufReader::new(reader);
        let header = read_wire_header(&mut reader).unwrap().unwrap();
        assert_ne!(header & RAW_TERMINAL_IMAGE_WIRE_FLAG, 0);
        let mut destination = vec![1, 2, 3];
        let metadata = read_terminal_image_wire_into(
            &mut reader,
            (header & !RAW_TERMINAL_IMAGE_WIRE_FLAG) as usize,
            &mut destination,
        )
        .unwrap();

        writer_thread.join().unwrap().unwrap();
        assert_eq!(metadata.key.id, 42);
        assert_eq!(metadata.key.generation, 7);
        assert_eq!(metadata.offset, 128);
        assert_eq!(metadata.chunk_length as usize, expected.len());
        assert_eq!(&destination[..3], &[1, 2, 3]);
        assert_eq!(&destination[3..], expected);
    }

    #[test]
    fn bundled_alacritty_terminfo_is_installed_for_child_sessions() {
        let database = install_bundled_terminfo().unwrap();
        let entry = database.join("61/alacritty");
        assert_eq!(fs::read(entry).unwrap(), BUNDLED_ALACRITTY_TERMINFO);
    }

    #[test]
    fn child_sessions_report_the_alacritty_terminal_contract() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 100,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();
        let initial_revision = session.snapshot().revision;
        assert!(
            session
                .events
                .wait_for_revision(initial_revision, Duration::from_secs(5)),
            "shell did not publish its first PTY output"
        );
        session
            .input(
                b"printf 'EGGIE_ENV:%s:%s:%s\\n' \"$TERM\" \"$COLORTERM\" \"$TERM_PROGRAM\"\r"
                    .to_vec(),
                1,
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let lines = session.snapshot().plain_lines();
            if lines
                .iter()
                .any(|line| line.contains("EGGIE_ENV:alacritty:truecolor:Eggie"))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child session did not inherit Eggie's terminal contract: {lines:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        session.terminate();
    }

    #[test]
    fn kitty_graphics_crosses_the_real_pty_snapshot_and_resource_paths() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let size = TerminalSize {
            columns: 80,
            rows: 24,
            cell_width: 8,
            cell_height: 18,
        };
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            size,
            TerminalAppearance::default(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        session
            .input(
                b"printf '\\033_Ga=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;AQIDBA==\\033\\\\'\r".to_vec(),
                1,
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            let snapshot = session.snapshot();
            if snapshot.images.iter().any(|image| image.key.id == 7) {
                break snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "Kitty image did not reach the published terminal snapshot"
            );
            thread::sleep(Duration::from_millis(20));
        };
        let descriptor = snapshot
            .images
            .iter()
            .find(|image| image.key.id == 7)
            .unwrap();
        assert_eq!((descriptor.width, descriptor.height), (1, 1));
        let placement = snapshot
            .image_placements
            .iter()
            .find(|placement| placement.image == descriptor.key)
            .unwrap();
        assert_eq!(
            (placement.destination_width, placement.destination_height),
            (8, 18)
        );

        // Advance the same Kitty image before fetching the generation referenced by `snapshot`.
        // Published frames must retain their immutable pixels instead of racing the live terminal.
        {
            let mut terminal = session.terminal.lock();
            terminal.kitty_graphics_command(b"a=d,d=I,i=7,q=2;");
            terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=7,c=1,r=1,C=1;BQYHCA==");
            session.events.publish_terminal(&terminal);
        }
        let newer_descriptor = loop {
            let current = session.snapshot();
            if let Some(current) = current
                .images
                .iter()
                .find(|image| image.key.id == 7 && image.key != descriptor.key)
            {
                break current.clone();
            }
            assert!(
                Instant::now() < deadline,
                "replacement Kitty image did not reach the published snapshot"
            );
            thread::sleep(Duration::from_millis(20));
        };
        let (width, height, total, pixels) = session.image_chunk(descriptor.key, 0, 4).unwrap();
        assert_eq!((width, height, total), (1, 1, 4));
        assert_eq!(pixels, [1, 2, 3, 4]);
        let (_, _, _, pixels) = session.image_chunk(newer_descriptor.key, 0, 4).unwrap();
        assert_eq!(pixels, [5, 6, 7, 8]);
        session.terminate();
    }

    #[test]
    fn installed_notcurses_exercises_implemented_terminal_compatibility() {
        if !Command::new("sh")
            .args(["-c", "command -v notcurses-info >/dev/null 2>&1"])
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }

        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 180,
                rows: 60,
                cell_width: 8,
                cell_height: 18,
            },
            TerminalAppearance::default(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        session.input(b"notcurses-info\r".to_vec(), 1).unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = session.snapshot();
            let lines = snapshot.plain_lines();
            let identified = lines
                .iter()
                .any(|line| line.contains("notcurses") && line.contains(" on Alacritty "));
            let quadrants = lines.iter().any(|line| line.contains("2x2+"));
            let terminal_capabilities = lines
                .iter()
                .any(|line| line.contains("uline+") && line.contains("rgb+"));
            let graphics_and_input = lines.iter().any(|line| line.contains("kbd+"))
                && lines.iter().any(|line| line.contains("pmouse+"))
                && lines
                    .iter()
                    .any(|line| line.contains("rgba pixel graphics support"));
            let finished = lines.iter().any(|line| line.contains("renders,"));
            if identified && quadrants && terminal_capabilities && graphics_and_input && finished {
                let placement = snapshot
                    .image_placements
                    .first()
                    .expect("notcurses Kitty logo placement must be published");
                assert_eq!(
                    placement.column, 55,
                    "synchronized cursor positioning must precede the Kitty display APC"
                );
                assert!(
                    (6..=9).contains(&placement.line),
                    "the notcurses logo must remain in its top information block: {placement:?}"
                );
                let rgb_backgrounds = snapshot
                    .cells
                    .iter()
                    .filter_map(|cell| match cell.background {
                        eggie_protocol::TerminalColor::Rgb(color) => Some(color),
                        _ => None,
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                assert!(
                    lines.iter().any(
                        |line| line.contains("default fg 0x") && line.contains("default bg 0x")
                    ),
                    "OSC default color queries were not answered: {lines:?}"
                );
                assert!(
                    rgb_backgrounds.len() > 256,
                    "notcurses truecolor gradient collapsed to {} backgrounds",
                    rgb_backgrounds.len()
                );
                let emoji_line = snapshot
                    .cells
                    .iter()
                    .find(|cell| cell.character == '👾')
                    .map(|cell| cell.line)
                    .expect("notcurses emoji coverage row must be present");
                let emoji_cells = snapshot
                    .cells
                    .iter()
                    .filter(|cell| cell.line == emoji_line)
                    .collect::<Vec<_>>();
                for (base, suffix) in [
                    ('👩', vec!['\u{200d}', '🔬']),
                    ('✊', vec!['🏿']),
                    ('🇦', vec!['🇶']),
                    ('🏴', vec!['\u{200d}', '☠', '\u{fe0f}']),
                    ('🤽', vec!['🏼', '\u{200d}', '♀', '\u{fe0f}']),
                ] {
                    let cell = emoji_cells
                        .iter()
                        .find(|cell| cell.character == base && cell.zerowidth == suffix)
                        .unwrap_or_else(|| {
                            panic!("missing grapheme base {base:?}: {emoji_cells:?}")
                        });
                    assert!(Flags::from_bits_retain(cell.flags).contains(Flags::WIDE_CHAR));
                }
                break;
            }
            assert!(
                Instant::now() < deadline,
                "notcurses did not detect Eggie's implemented Kitty compatibility: {lines:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        session.input(vec![0x03], 2).unwrap();
        session.terminate();
    }

    #[test]
    fn paste_respects_alacritty_bracketed_paste_mode() {
        assert_eq!(paste_bytes("one\ntwo", false), b"one\rtwo");
        assert_eq!(
            paste_bytes("one\n\x1btwo", true),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
    }

    #[test]
    fn snapshot_omits_only_visually_empty_default_cells() {
        let mut cell = Cell::default();
        assert!(snapshot_cell_is_empty(&cell));
        cell.flags.insert(Flags::BOLD);
        assert!(snapshot_cell_is_empty(&cell));
        cell.flags.insert(Flags::UNDERLINE);
        assert!(!snapshot_cell_is_empty(&cell));

        let cell = Cell {
            bg: Color::Indexed(1),
            ..Cell::default()
        };
        assert!(!snapshot_cell_is_empty(&cell));
        let mut cell = Cell::default();
        cell.flags.insert(Flags::WRAPLINE);
        assert!(!snapshot_cell_is_empty(&cell));
    }

    #[test]
    fn messagepack_wire_round_trip_preserves_truecolor_cells() {
        let snapshot = Arc::new(TerminalSnapshot {
            session_id: SessionId::nil(),
            size: TerminalSize {
                columns: 1,
                rows: 1,
                ..TerminalSize::default()
            },
            cells: vec![TerminalCell {
                line: 0,
                column: 0,
                character: '▀',
                zerowidth: Vec::new(),
                foreground: eggie_protocol::TerminalColor::Rgb(0x46b4c8ff),
                background: eggie_protocol::TerminalColor::Rgb(0xbee8f5ff),
                underline_color: Some(eggie_protocol::TerminalColor::Rgb(0x112233ff)),
                hyperlink: Some("https://example.com".to_owned()),
                flags: 0,
            }],
            color_overrides: vec![TerminalColorOverride {
                index: 42,
                color: 0xaabbccff,
            }],
            cursor_line: 0,
            cursor_column: 0,
            cursor_shape: TerminalCursorShape::Hidden,
            cursor_width: 1,
            cursor_blinking: false,
            title: "truecolor".to_owned(),
            revision: 7,
            last_input_sequence: 3,
            input_modes: TerminalInputModes::default(),
            images: Vec::new(),
            image_placements: Vec::new(),
            selection: None,
            detected_links: Vec::new(),
        });
        let response = DaemonResponse::Snapshot {
            snapshot: snapshot.clone(),
        };
        let mut encoded = Vec::new();
        let mut scratch = Vec::new();
        write_wire_message(&mut encoded, &mut scratch, &response).unwrap();
        let decoded = read_wire_message::<DaemonResponse>(
            &mut std::io::Cursor::new(encoded),
            &mut Vec::new(),
        )
        .unwrap()
        .expect("wire frame is present");
        assert_eq!(decoded, response);
    }

    #[test]
    fn snapshot_delta_keeps_color_only_cell_changes() {
        let cell = |column, foreground, background| TerminalCell {
            line: 0,
            column,
            character: '▀',
            zerowidth: Vec::new(),
            foreground: eggie_protocol::TerminalColor::Rgb(foreground),
            background: eggie_protocol::TerminalColor::Rgb(background),
            underline_color: None,
            hyperlink: None,
            flags: 0,
        };
        let mut base = TerminalSnapshot {
            session_id: SessionId::nil(),
            size: TerminalSize {
                columns: 2,
                rows: 1,
                ..TerminalSize::default()
            },
            cells: vec![
                cell(0, 0x111111ff, 0x222222ff),
                cell(1, 0x333333ff, 0x444444ff),
            ],
            color_overrides: Vec::new(),
            cursor_line: 0,
            cursor_column: 0,
            cursor_shape: TerminalCursorShape::Hidden,
            cursor_width: 1,
            cursor_blinking: false,
            title: String::new(),
            revision: 10,
            last_input_sequence: 0,
            input_modes: TerminalInputModes::default(),
            images: Vec::new(),
            image_placements: Vec::new(),
            selection: None,
            detected_links: Vec::new(),
        };
        let image = TerminalImageKey {
            id: 7,
            generation: 9,
        };
        base.images.push(TerminalImageDescriptor {
            key: image,
            width: 1,
            height: 1,
        });
        base.image_placements.push(TerminalImagePlacement {
            image,
            placement_id: 3,
            line: 0,
            column: 0,
            source_x: 0,
            source_y: 0,
            source_width: 1,
            source_height: 1,
            x_offset: 0,
            y_offset: 0,
            columns: 1,
            rows: 1,
            destination_width: 1,
            destination_height: 1,
            z: 0,
        });
        let mut current = base.clone();
        current.revision = 11;
        current.cells[1] = cell(1, 0xaabbccff, 0x123456ff);

        let delta = snapshot_delta(&base, &current).expect("one of two cells changed");
        assert_eq!(delta.cells, vec![current.cells[1].clone()]);
        assert!(delta.cleared.is_empty());
        assert!(delta.images.is_none());
        assert!(delta.image_placements.is_none());
        assert_eq!(base.apply_delta(&delta), Some(current));
    }

    #[test]
    fn snapshot_wait_wakes_on_revision_without_polling_delay() {
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            TerminalSize::default(),
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        ));
        let worker_state = state.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            worker_state.signal_revision_for_test();
        });
        let started = Instant::now();
        assert!(state.wait_for_revision(0, Duration::from_secs(1)));
        assert!(started.elapsed() < Duration::from_millis(250));
        worker.join().unwrap();

        let revision = state.revision.load(Ordering::Acquire);
        let started = Instant::now();
        assert!(!state.wait_for_revision(revision, Duration::from_millis(20)));
        assert!(started.elapsed() >= Duration::from_millis(15));
    }

    #[test]
    fn progress_tracker_coalesces_reports_and_expires_completed_work() {
        use alacritty_terminal::vte::ansi::{ProgressReport, ProgressState};

        let session_id = SessionId::new_v4();
        let tracker = ProgressTracker::new(session_id);
        tracker.set_timeouts(TerminalProgressTimeouts {
            completed_ms: 100,
            stale_ms: 500,
        });
        tracker.report(Some(ProgressReport {
            state: ProgressState::Normal,
            percent: Some(1),
        }));
        let first = tracker
            .wait_after(0, Duration::from_millis(50))
            .expect("first report publishes immediately");
        assert_eq!(first.progress.unwrap().percent, Some(1));

        tracker.report(Some(ProgressReport {
            state: ProgressState::Normal,
            percent: Some(2),
        }));
        tracker.report(Some(ProgressReport {
            state: ProgressState::Normal,
            percent: Some(100),
        }));
        assert!(
            tracker
                .wait_after(first.revision, Duration::from_millis(2))
                .is_none()
        );
        let completed = tracker
            .wait_after(first.revision, Duration::from_millis(50))
            .expect("latest report publishes on the next progress frame");
        assert_eq!(completed.progress.unwrap().percent, Some(100));

        let cleared = tracker
            .wait_after(completed.revision, Duration::from_millis(250))
            .expect("completed progress clears after its timeout");
        assert_eq!(cleared.progress, None);
    }

    #[test]
    fn pty_osc_progress_reaches_daemon_and_ris_clears_it() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize::default(),
            TerminalAppearance::default(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));

        session
            .input(b"printf '\\033]9;4;1;42\\a'\r".to_vec(), 1)
            .unwrap();
        let progress = session
            .wait_for_progress(0, Duration::from_secs(2))
            .expect("OSC 9;4 report reaches daemon state");
        assert_eq!(
            progress
                .progress
                .map(|progress| (progress.state, progress.percent)),
            Some((TerminalProgressState::Normal, Some(42)))
        );

        session.input(b"printf '\\033c'\r".to_vec(), 2).unwrap();
        let cleared = session
            .wait_for_progress(progress.revision, Duration::from_secs(2))
            .expect("RIS clears daemon progress state");
        assert_eq!(cleared.progress, None);
        session.terminate();
    }

    #[test]
    fn terminal_input_to_snapshot_is_event_driven() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        let mut snapshot = session.snapshot();
        let mut revision = snapshot.revision;
        let mut samples = Vec::new();
        let mut serialization_samples = Vec::new();
        let mut response_bytes = 0;
        for sequence in 1..=32 {
            let started = Instant::now();
            session.input(b"x".to_vec(), sequence).unwrap();
            let update = session
                .wait_for_snapshot(revision, Duration::from_secs(1))
                .expect("terminal input did not produce a snapshot");
            snapshot = match update {
                TerminalSnapshotUpdate::Full(snapshot) => snapshot,
                TerminalSnapshotUpdate::Delta(delta) => Arc::new(
                    snapshot
                        .apply_delta(&delta)
                        .expect("input snapshot delta applies to its requested base"),
                ),
            };
            samples.push(started.elapsed());
            revision = snapshot.revision;
            let serialization_started = Instant::now();
            response_bytes = encode_line(&DaemonResponse::Snapshot {
                snapshot: snapshot.clone(),
            })
            .unwrap()
            .len();
            serialization_samples.push(serialization_started.elapsed());
        }
        samples.sort_unstable();
        serialization_samples.sort_unstable();
        let p50 = samples[(samples.len() - 1) * 50 / 100];
        let p95 = samples[(samples.len() - 1) * 95 / 100];
        let serialization_p50 = serialization_samples[(serialization_samples.len() - 1) * 50 / 100];
        let serialization_p95 = serialization_samples[(serialization_samples.len() - 1) * 95 / 100];
        eprintln!(
            "daemon input→snapshot latency: p50={:.2}ms p95={:.2}ms; snapshot JSON: {} bytes p50={:.2}ms p95={:.2}ms",
            p50.as_secs_f64() * 1_000.,
            p95.as_secs_f64() * 1_000.,
            response_bytes,
            serialization_p50.as_secs_f64() * 1_000.,
            serialization_p95.as_secs_f64() * 1_000.,
        );
        assert!(p95 < Duration::from_millis(250));
        session.input(vec![0x03], 33).unwrap();
        session.terminate();
    }

    #[test]
    #[ignore = "performance benchmark requiring the locally installed flappy-tui binary"]
    fn sustained_full_screen_snapshot_transport_benchmark() {
        let flappy = Path::new("/Users/bytedance/.cargo/bin/flappy-tui");
        if !flappy.exists() {
            eprintln!("skipping benchmark because {} is missing", flappy.display());
            return;
        }
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = Arc::new(
            TerminalSession::spawn(
                ProjectId::new_v4(),
                std::env::current_dir().unwrap(),
                TerminalSize {
                    columns: 229,
                    rows: 74,
                    cell_width: 8,
                    cell_height: 18,
                },
                TerminalAppearance::default(),
            )
            .unwrap(),
        );
        let session_id = session.id;
        let state = Arc::new(DaemonState {
            sessions: RwLock::new(HashMap::from([(session_id, session.clone())])),
            build_id: Arc::from("benchmark"),
        });
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let server_state = state.clone();
        let server = thread::spawn(move || serve_connection(server_stream, &server_state));
        let mut connection = DaemonConnection {
            stream: BufReader::new(client_stream),
            request: Vec::with_capacity(512),
            response: Vec::with_capacity(1024 * 1024),
        };

        thread::sleep(Duration::from_millis(800));
        session
            .input(format!("{}\r", flappy.display()).into_bytes(), 1)
            .unwrap();
        thread::sleep(Duration::from_millis(500));
        session.input(b" ".to_vec(), 2).unwrap();
        thread::sleep(Duration::from_millis(700));

        let mut snapshot = session.snapshot();
        let mut revision = snapshot.revision;
        let mut samples = Vec::with_capacity(60);
        let mut payload_cells = 0;
        let mut payload_bytes = 0;
        for _ in 0..60 {
            let started = Instant::now();
            let response = connection
                .request(ClientRequest::WaitForSnapshot {
                    session_id,
                    after_revision: revision,
                    timeout_ms: 1000,
                })
                .unwrap();
            snapshot = match response {
                DaemonResponse::Snapshot { snapshot } => snapshot,
                DaemonResponse::SnapshotDelta { delta } => Arc::new(
                    snapshot
                        .apply_delta(&delta)
                        .expect("benchmark delta applies to its requested base"),
                ),
                response => panic!(
                    "unexpected benchmark response: {response:?}; screen={:?}",
                    session.snapshot().plain_lines()
                ),
            };
            samples.push(started.elapsed());
            revision = snapshot.revision;
            payload_cells = snapshot.cells.len();
            payload_bytes = connection.response.len();
        }
        samples.sort_unstable();
        let p50 = samples[(samples.len() - 1) * 50 / 100];
        let p95 = samples[(samples.len() - 1) * 95 / 100];
        eprintln!(
            "229x74 flappy snapshot transport: cells={payload_cells} wire={:.1}KiB p50={:.2}ms p95={:.2}ms ({:.1} fps)",
            payload_bytes as f64 / 1024.,
            p50.as_secs_f64() * 1_000.,
            p95.as_secs_f64() * 1_000.,
            1. / p50.as_secs_f64(),
        );

        session.terminate();
        drop(connection);
        server.join().unwrap().unwrap();
        assert!(p95 < Duration::from_millis(50));
    }

    #[test]
    fn adjacent_input_messages_are_coalesced_without_losing_order() {
        let session_id = SessionId::new_v4();
        let mut first = QueuedTerminalInput::Input {
            session_id,
            bytes: b"a".to_vec(),
            sequence: 1,
        };
        first
            .merge(QueuedTerminalInput::Input {
                session_id,
                bytes: b"b".to_vec(),
                sequence: 2,
            })
            .unwrap();
        let ClientRequest::Input {
            bytes, sequence, ..
        } = first.request()
        else {
            panic!("coalesced input changed request kind")
        };
        assert_eq!(bytes, b"ab");
        assert_eq!(sequence, 2);
    }

    #[test]
    fn continuous_input_is_dispatched_in_latency_bounded_batches() {
        let session_id = SessionId::new_v4();
        let (sender, receiver) = mpsc::channel();
        for sequence in 1..=(MAX_INPUT_BATCH_MESSAGES as u64 * 3) {
            sender
                .send(QueuedTerminalInput::Input {
                    session_id,
                    bytes: vec![b'x'],
                    sequence,
                })
                .unwrap();
        }
        let first = receiver.recv().unwrap();
        let batch = receive_input_batch(&receiver, first);
        assert_eq!(batch.len(), 1, "adjacent key input should still coalesce");
        let ClientRequest::Input {
            bytes, sequence, ..
        } = batch.into_iter().next().unwrap().request()
        else {
            panic!("input batch changed request kind")
        };
        assert_eq!(bytes.len(), MAX_INPUT_BATCH_MESSAGES);
        assert_eq!(sequence, MAX_INPUT_BATCH_MESSAGES as u64);
        assert_eq!(receiver.try_iter().count(), MAX_INPUT_BATCH_MESSAGES * 2);
    }

    #[test]
    fn unresolved_terminal_colors_remain_semantic() {
        assert_eq!(
            snapshot_color(Color::Named(
                alacritty_terminal::vte::ansi::NamedColor::Foreground
            )),
            eggie_protocol::TerminalColor::Named(
                alacritty_terminal::vte::ansi::NamedColor::Foreground as u16
            )
        );
    }

    #[test]
    fn every_alacritty_cursor_shape_is_preserved_in_the_snapshot_protocol() {
        assert_eq!(
            snapshot_cursor_shape(CursorShape::Block),
            TerminalCursorShape::Block
        );
        assert_eq!(
            snapshot_cursor_shape(CursorShape::Underline),
            TerminalCursorShape::Underline
        );
        assert_eq!(
            snapshot_cursor_shape(CursorShape::Beam),
            TerminalCursorShape::Beam
        );
        assert_eq!(
            snapshot_cursor_shape(CursorShape::HollowBlock),
            TerminalCursorShape::HollowBlock
        );
        assert_eq!(
            snapshot_cursor_shape(CursorShape::Hidden),
            TerminalCursorShape::Hidden
        );
    }

    #[test]
    fn default_cursor_shape_config_is_applied_and_overridable_by_the_program() {
        use alacritty_terminal::event::VoidListener;
        use alacritty_terminal::vte::ansi::Processor;

        let size = TerminalSize {
            columns: 20,
            rows: 5,
            ..TerminalSize::default()
        };
        // Build a terminal with a Beam default shape (as set_default_cursor_shape would).
        let mut term = Term::new(
            terminal_config(kernel_cursor_shape(TerminalCursorShape::Beam)),
            &GridSize(size),
            VoidListener,
        );
        assert_eq!(term.cursor_style().shape, CursorShape::Beam);

        // A program issuing DECSCUSR (CSI 2 SP q = steady block) overrides the configured default.
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"\x1b[2 q");
        assert_eq!(term.cursor_style().shape, CursorShape::Block);

        // Reapplying the default via set_options must not clobber the program's runtime override.
        term.set_options(terminal_config(kernel_cursor_shape(
            TerminalCursorShape::Underline,
        )));
        assert_eq!(
            term.cursor_style().shape,
            CursorShape::Block,
            "program's DECSCUSR override should survive a default-shape config change"
        );

        // DECSCUSR 0 resets to the (new) configured default.
        processor.advance(&mut term, b"\x1b[0 q");
        assert_eq!(term.cursor_style().shape, CursorShape::Underline);
    }

    #[test]
    fn hidden_is_not_usable_as_a_default_cursor_shape() {
        // Hidden has no meaning as a *default* (it would make the cursor permanently invisible), so
        // the protocol->kernel mapping falls back to Block.
        assert_eq!(
            kernel_cursor_shape(TerminalCursorShape::Hidden),
            CursorShape::Block
        );
    }

    #[test]
    fn process_filter_keeps_only_the_terminal_process_tree() {
        let processes = vec![
            ProcessInfo {
                pid: 10,
                parent_pid: Some(1),
                name: "shell".to_owned(),
                cpu_usage_tenths_percent: None,
                memory_bytes: None,
            },
            ProcessInfo {
                pid: 11,
                parent_pid: Some(10),
                name: "node".to_owned(),
                cpu_usage_tenths_percent: None,
                memory_bytes: None,
            },
            ProcessInfo {
                pid: 12,
                parent_pid: Some(11),
                name: "worker".to_owned(),
                cpu_usage_tenths_percent: None,
                memory_bytes: None,
            },
            ProcessInfo {
                pid: 20,
                parent_pid: Some(1),
                name: "unrelated".to_owned(),
                cpu_usage_tenths_percent: None,
                memory_bytes: None,
            },
        ];

        assert_eq!(
            filter_descendant_processes(10, processes)
                .into_iter()
                .map(|process| process.pid)
                .collect::<Vec<_>>(),
            [10, 11, 12]
        );
    }

    #[test]
    fn cpu_usage_is_stored_at_one_decimal_percent_precision() {
        assert_eq!(cpu_usage_tenths_percent(12.34), 123);
        assert_eq!(cpu_usage_tenths_percent(12.36), 124);
        assert_eq!(cpu_usage_tenths_percent(f32::NAN), 0);
        assert_eq!(cpu_usage_tenths_percent(-1.), 0);
    }

    #[test]
    fn lsof_machine_output_is_parsed_into_listening_ports() {
        let output = "p42\ncpython\nf5u\nPTCP\nn127.0.0.1:3000\nf6u\nPUDP\nn*:5353\n";

        assert_eq!(
            parse_lsof_ports(output),
            vec![
                ListeningPort {
                    pid: 42,
                    protocol: "TCP".to_owned(),
                    address: "127.0.0.1".to_owned(),
                    port: 3000,
                },
                ListeningPort {
                    pid: 42,
                    protocol: "UDP".to_owned(),
                    address: "*".to_owned(),
                    port: 5353,
                },
            ]
        );
    }

    #[test]
    fn open_tcp_listener_is_reported_for_its_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let pid = std::process::id();
        let ports = listening_ports(&[ProcessInfo {
            pid,
            parent_pid: None,
            name: "test".to_owned(),
            cpu_usage_tenths_percent: None,
            memory_bytes: None,
        }]);

        assert!(
            ports
                .iter()
                .any(|entry| entry.pid == pid && entry.port == port && entry.protocol == "TCP")
        );
    }

    #[test]
    fn primary_screen_resize_reflows_completed_output_without_losing_columns() {
        let initial_size = TerminalSize {
            columns: 12,
            rows: 4,
            ..TerminalSize::default()
        };
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            initial_size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        ));
        let listener = DaemonEventListener(state);
        let mut terminal = Term::new(Config::default(), &GridSize(initial_size), listener);
        for (column, character) in "ABCDEFGHIJKL".chars().enumerate() {
            terminal.grid_mut()[Line(0)][Column(column)].c = character;
        }
        terminal.grid_mut()[Line(0)][Column(11)]
            .flags
            .insert(Flags::WRAPLINE);
        for (column, character) in "MNOP".chars().enumerate() {
            terminal.grid_mut()[Line(1)][Column(column)].c = character;
        }
        // The cursor is on the active shell line below this completed wrapped output.
        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(3), Column(0));

        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 8,
                ..initial_size
            },
            TerminalSemanticPhase::Output,
            None,
        );

        assert_eq!(terminal.grid()[Line(0)][Column(0)].c, 'A');
        assert_eq!(terminal.grid()[Line(0)][Column(7)].c, 'H');
        assert_eq!(terminal.grid()[Line(1)][Column(0)].c, 'I');
        assert_eq!(terminal.grid()[Line(1)][Column(7)].c, 'P');
        assert!(
            terminal.grid()[Line(0)][Column(7)]
                .flags
                .contains(Flags::WRAPLINE)
        );

        resize_terminal_with_history_reflow(&mut terminal, initial_size, TerminalSemanticPhase::Output, None);

        assert_eq!(terminal.grid()[Line(0)][Column(0)].c, 'A');
        assert_eq!(terminal.grid()[Line(0)][Column(11)].c, 'L');
        assert_eq!(terminal.grid()[Line(1)][Column(0)].c, 'M');
    }

    #[test]
    fn primary_screen_resize_clears_active_wrapped_input_for_shell_redraw() {
        let initial_size = TerminalSize {
            columns: 12,
            rows: 3,
            ..TerminalSize::default()
        };
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            initial_size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        ));
        let listener = DaemonEventListener(state);
        let mut terminal = Term::new(Config::default(), &GridSize(initial_size), listener);
        for (column, character) in "ABCDEFGHIJKL".chars().enumerate() {
            terminal.grid_mut()[Line(0)][Column(column)].c = character;
        }
        terminal.grid_mut()[Line(0)][Column(11)]
            .flags
            .insert(Flags::WRAPLINE);
        for (column, character) in "MNOP".chars().enumerate() {
            terminal.grid_mut()[Line(1)][Column(column)].c = character;
        }
        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(1), Column(4));

        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 8,
                ..initial_size
            },
            TerminalSemanticPhase::Input,
            Some(0),
        );

        // The active prompt/input line is on a prompt phase, so it is cleared entirely: none of
        // its glyphs survive and no stale WRAPLINE continuation is left for the shell's redraw to
        // stack a duplicate on top of.
        let has_prompt_glyph = (-(terminal.grid().history_size() as i32)
            ..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .any(|(line, column)| {
                let cell = &terminal.grid()[Line(line)][Column(column)];
                "ABCDEFGHIJKLMNOP".contains(cell.c)
            });
        assert!(
            !has_prompt_glyph,
            "the active prompt/input line must be cleared for the shell to redraw"
        );
        let has_wrapline = (-(terminal.grid().history_size() as i32)
            ..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .any(|(line, column)| {
                terminal.grid()[Line(line)][Column(column)]
                    .flags
                    .contains(Flags::WRAPLINE)
            });
        assert!(
            !has_wrapline,
            "no stale WRAPLINE continuation may survive on the cleared prompt line"
        );
    }

    /// Build a bare primary-screen terminal with a two-row wrapped active line ("ABCDEFGH" on
    /// row 0 continuing into "IJKL" on row 1), the cursor parked on the continuation row. Returns
    /// the terminal ready for a resize.
    fn terminal_with_wrapped_active_line(size: TerminalSize) -> Term<DaemonEventListener> {
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        ));
        let listener = DaemonEventListener(state);
        let mut terminal = Term::new(Config::default(), &GridSize(size), listener);
        for (column, character) in "ABCDEFGH".chars().enumerate() {
            terminal.grid_mut()[Line(0)][Column(column)].c = character;
        }
        terminal.grid_mut()[Line(0)][Column(size.columns as usize - 1)]
            .flags
            .insert(Flags::WRAPLINE);
        for (column, character) in "IJKL".chars().enumerate() {
            terminal.grid_mut()[Line(1)][Column(column)].c = character;
        }
        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(1), Column(4));
        terminal
    }

    fn grid_has_glyph(terminal: &Term<DaemonEventListener>, glyphs: &str) -> bool {
        (-(terminal.grid().history_size() as i32)..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .any(|(line, column)| glyphs.contains(terminal.grid()[Line(line)][Column(column)].c))
    }

    fn grid_has_wrapline(terminal: &Term<DaemonEventListener>) -> bool {
        (-(terminal.grid().history_size() as i32)..terminal.grid().screen_lines() as i32)
            .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
            .any(|(line, column)| {
                terminal.grid()[Line(line)][Column(column)]
                    .flags
                    .contains(Flags::WRAPLINE)
            })
    }

    #[test]
    fn prompt_phase_shrink_clears_active_line_and_removes_wrapline() {
        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);
        // Put a completed-output row above the active line to make sure it is NOT cleared.
        terminal.grid_mut()[Line(3)][Column(0)].c = '#';

        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 4,
                ..initial_size
            },
            TerminalSemanticPhase::Prompt,
            Some(0),
        );

        assert!(
            !grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "the wrapped prompt line must be fully cleared on shrink"
        );
        assert!(
            !grid_has_wrapline(&terminal),
            "no WRAPLINE may survive on the cleared prompt region"
        );
    }

    #[test]
    fn prompt_phase_grow_clears_active_line_no_orphan_rows() {
        // Regression test for the duplicate-fragment bug: on WIDEN, the old code stripped the
        // WRAPLINE marker but left the continuation cells in place, so alacritty's grow_columns
        // failed to merge them and left an orphan row that stacked under the shell's redraw.
        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);

        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 16,
                ..initial_size
            },
            TerminalSemanticPhase::Input,
            Some(0),
        );

        assert!(
            !grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "no orphan continuation row may survive a widen on a prompt line"
        );
        assert!(
            !grid_has_wrapline(&terminal),
            "no stale WRAPLINE may survive a widen on a prompt line"
        );
    }

    #[test]
    fn output_phase_reflows_natively_without_clearing() {
        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);

        // Output phase: the wrapped content is completed command output and must reflow, not be
        // cleared. Shrink then grow and confirm the glyphs survive the round trip.
        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 4,
                ..initial_size
            },
            TerminalSemanticPhase::Output,
            None,
        );
        assert!(
            grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "output content must be preserved (reflowed), not cleared"
        );
        resize_terminal_with_history_reflow(&mut terminal, initial_size, TerminalSemanticPhase::Output, None);
        assert!(
            grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "output content must survive the reflow round trip"
        );
    }

    #[test]
    fn none_phase_uses_native_reflow_without_clearing() {
        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);

        // No shell integration: phase stays None and we must fall back to native reflow, never
        // clearing content.
        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 4,
                ..initial_size
            },
            TerminalSemanticPhase::None,
            None,
        );
        assert!(
            grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "without shell integration, content must reflow, not be cleared"
        );
    }

    #[test]
    fn alt_screen_resize_skips_prompt_clearing() {
        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);
        // Enter the alternate screen; the wrapped active line above is on the primary screen, but
        // the alt-screen guard must short-circuit before any clearing regardless of phase.
        let mut parser: alacritty_terminal::vte::ansi::Processor =
            alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut terminal, b"\x1b[?1049h");
        assert!(terminal.mode().contains(TermMode::ALT_SCREEN));

        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 4,
                ..initial_size
            },
            TerminalSemanticPhase::Prompt,
            Some(0),
        );
        // The alt screen was blank, so there is nothing to assert on its contents; the test simply
        // verifies the guard path runs without touching the primary grid's clear logic (no panic,
        // resize applied).
        assert_eq!(terminal.columns(), 4);
    }

    #[test]
    fn row_only_resize_does_not_clear_prompt() {
        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);

        // Only rows change (columns stay 8): no wrap-reflow happens, so the prompt line must be
        // left untouched even on a prompt phase.
        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                rows: 6,
                ..initial_size
            },
            TerminalSemanticPhase::Prompt,
            Some(0),
        );
        assert!(
            grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "a row-only resize must not clear the prompt line"
        );
    }

    #[test]
    fn resize_reads_tracker_phase_to_gate_clearing() {
        // Drive the tracker the way the OSC 133 event path does, then confirm the phase it exposes
        // routes the resize into the prompt-clearing branch.
        let mut tracker = ShellIntegrationTracker::default();
        tracker.update(
            SemanticPrompt {
                action: SemanticPromptAction::PromptStart,
                options: String::new(),
            },
            0,
            0,
        );
        tracker.update(
            SemanticPrompt {
                action: SemanticPromptAction::InputStart,
                options: String::new(),
            },
            1,
            0,
        );
        assert_eq!(tracker.phase, TerminalSemanticPhase::Input);
        // The prompt start was recorded from the first (PromptStart) marker, not moved down to the
        // input line.
        assert_eq!(tracker.prompt_start_line, Some(0));

        let initial_size = TerminalSize {
            columns: 8,
            rows: 4,
            ..TerminalSize::default()
        };
        let mut terminal = terminal_with_wrapped_active_line(initial_size);
        resize_terminal_with_history_reflow(
            &mut terminal,
            TerminalSize {
                columns: 4,
                ..initial_size
            },
            tracker.phase,
            tracker.prompt_start_line,
        );
        assert!(
            !grid_has_glyph(&terminal, "ABCDEFGHIJKL"),
            "the tracker's Input phase must route the resize into the prompt-clearing branch"
        );
    }

    fn prompt_start(tracker: &mut ShellIntegrationTracker, cursor_line: i32, history_size: usize) {
        tracker.update(
            SemanticPrompt {
                action: SemanticPromptAction::PromptStart,
                options: String::new(),
            },
            cursor_line,
            history_size,
        );
    }

    fn output_start(tracker: &mut ShellIntegrationTracker, history_size: usize) {
        tracker.update(
            SemanticPrompt {
                action: SemanticPromptAction::OutputStart,
                options: String::new(),
            },
            0,
            history_size,
        );
    }

    #[test]
    fn prompt_jump_points_dedupe_repeated_marks() {
        let mut tracker = ShellIntegrationTracker::default();
        // A prompt re-emitting PromptStart while already on the prompt (zle redraw) must not add a
        // second jump point.
        prompt_start(&mut tracker, 0, 0);
        prompt_start(&mut tracker, 0, 0);
        prompt_start(&mut tracker, 0, 0);
        assert_eq!(tracker.prompt_jump_points.len(), 1);
        assert_eq!(tracker.prompt_jump_points.front(), Some(&0));
    }

    #[test]
    fn prompt_jump_points_use_global_line_from_total_scrolled() {
        let mut tracker = ShellIntegrationTracker::default();
        // First prompt at screen line 0, nothing scrolled yet -> global 0.
        prompt_start(&mut tracker, 0, 0);
        output_start(&mut tracker, 0);
        // 5 lines of output scrolled off (history_size captured with the next marker); next prompt
        // at screen line 3 -> global 5 + 3 = 8. This proves the capture uses the marker's own
        // history_size, without relying on a separate observe_scroll call.
        prompt_start(&mut tracker, 3, 5);
        assert_eq!(
            tracker.prompt_jump_points.iter().copied().collect::<Vec<_>>(),
            vec![0, 8]
        );
    }

    #[test]
    fn prompt_capture_uses_marker_history_not_stale_observe_scroll() {
        // Regression: a burst like `cat ~/.zshrc` scrolls many lines in one parser batch, while the
        // throttled observe_scroll lags behind. The new prompt marker must still record an accurate
        // global coordinate from its own (fresh) history_size, so a later Up jump lands on the
        // previous prompt rather than in the middle of the output.
        let mut tracker = ShellIntegrationTracker::default();
        prompt_start(&mut tracker, 0, 0); // prompt #0 at global 0
        output_start(&mut tracker, 0);
        // 50 lines of output scrolled off. observe_scroll has NOT run yet (still stale at 0), but
        // the next prompt marker carries the true history_size = 50.
        prompt_start(&mut tracker, 2, 50); // prompt #1 at global 50 + 2 = 52
        assert_eq!(
            tracker.prompt_jump_points.iter().copied().collect::<Vec<_>>(),
            vec![0, 52],
            "the second prompt must be recorded at its true post-scroll coordinate"
        );
        // Viewport sitting at the live bottom: top line global == total_scrolled (50).
        assert_eq!(tracker.total_scrolled_lines, 50);
        // Jumping Up from the bottom selects prompt #0 (global 0), never a mid-output line.
        assert_eq!(tracker.jump_target(50, TerminalJumpDirection::Up), Some(0));
    }

    #[test]
    fn observe_scroll_tracks_history_below_saturation() {
        let mut tracker = ShellIntegrationTracker::default();
        prompt_start(&mut tracker, 0, 0); // global 0
        output_start(&mut tracker, 0);
        // Below saturation, total_scrolled == history_size and nothing is evicted yet (the whole
        // buffer still fits in scrollback), so the point survives.
        tracker.observe_scroll(500, TERMINAL_SCROLLBACK_LIMIT);
        assert_eq!(tracker.total_scrolled_lines, 500);
        assert_eq!(tracker.prompt_jump_points.front(), Some(&0));
    }

    #[test]
    fn observe_scroll_prunes_points_that_fall_out_of_a_shrunk_buffer() {
        let mut tracker = ShellIntegrationTracker::default();
        // Simulate a saturated buffer: many lines scrolled, points spread across the window.
        tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
        tracker.prompt_jump_points.extend([5, 9_000]);
        // The active-screen top is at global == total_scrolled; a point at global 5 sits
        // `10000 - 5` lines up, still inside the scrollback window (oldest live == 0), so it stays.
        tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
        assert_eq!(
            tracker.prompt_jump_points.iter().copied().collect::<Vec<_>>(),
            vec![5, 9_000]
        );
    }

    #[test]
    fn command_history_cap_bounds_the_jump_index() {
        let mut tracker = ShellIntegrationTracker::default();
        // Push more distinct prompts than the cap; the oldest are dropped, newest kept.
        for i in 0..(COMMAND_HISTORY + 5) {
            prompt_start(&mut tracker, 0, i);
            output_start(&mut tracker, i);
        }
        assert!(tracker.prompt_jump_points.len() <= COMMAND_HISTORY);
    }

    #[test]
    fn observe_scroll_advances_after_saturation() {
        let mut tracker = ShellIntegrationTracker::default();
        tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
        assert_eq!(tracker.total_scrolled_lines, TERMINAL_SCROLLBACK_LIMIT as u64);
        // history_size stays pinned at the limit, but observe_scroll must not stall: the delta is 0
        // here, so total stays; a later call with the same size keeps it monotonic.
        tracker.observe_scroll(TERMINAL_SCROLLBACK_LIMIT, TERMINAL_SCROLLBACK_LIMIT);
        assert_eq!(tracker.total_scrolled_lines, TERMINAL_SCROLLBACK_LIMIT as u64);
    }

    #[test]
    fn clear_jump_points_resets_index_and_base() {
        let mut tracker = ShellIntegrationTracker::default();
        prompt_start(&mut tracker, 2, 0);
        tracker.observe_scroll(10, TERMINAL_SCROLLBACK_LIMIT);
        tracker.clear_jump_points();
        assert!(tracker.prompt_jump_points.is_empty());
        assert_eq!(tracker.total_scrolled_lines, 0);
        assert_eq!(tracker.last_history_size, 0);
    }

    #[test]
    fn jump_target_selects_strictly_nearer_prompt_in_each_direction() {
        let mut tracker = ShellIntegrationTracker::default();
        // Prompts at global lines 0, 10, 20.
        tracker.prompt_jump_points.extend([0, 10, 20]);
        // Viewport top at global 15: Up -> nearest below 15 is 10; Down -> nearest above 15 is 20.
        assert_eq!(tracker.jump_target(15, TerminalJumpDirection::Up), Some(10));
        assert_eq!(tracker.jump_target(15, TerminalJumpDirection::Down), Some(20));
        // At the oldest prompt (0): Up has nothing strictly earlier.
        assert_eq!(tracker.jump_target(0, TerminalJumpDirection::Up), None);
        // At the newest prompt (20): Down has nothing strictly later.
        assert_eq!(tracker.jump_target(20, TerminalJumpDirection::Down), None);
        // Exactly on a prompt line (10): Up -> 0, Down -> 20 (strict inequality).
        assert_eq!(tracker.jump_target(10, TerminalJumpDirection::Up), Some(0));
        assert_eq!(tracker.jump_target(10, TerminalJumpDirection::Down), Some(20));
    }

    #[test]
    fn zsh_env_sets_zdotdir_and_preserves_original() {
        let terminfo = PathBuf::from("/tmp/eggie-terminfo");
        let root = PathBuf::from("/tmp/eggie-integration");
        let launch = build_shell_env(
            "zsh",
            "/bin/zsh",
            &terminfo,
            Some(&root),
            Some("/home/user/.zsh".to_owned()),
            None,
        );
        assert_eq!(
            launch.env.get("ZDOTDIR").map(String::as_str),
            Some("/tmp/eggie-integration/zsh")
        );
        assert_eq!(
            launch.env.get("EGGIE_ZDOTDIR_ORIG").map(String::as_str),
            Some("/home/user/.zsh")
        );
        // Base environment still present, and the login arg is preserved for zsh.
        assert_eq!(launch.env.get("TERM").map(String::as_str), Some("alacritty"));
        assert!(launch.env.contains_key("TERMINFO"));
        assert_eq!(launch.args, vec!["-l".to_owned()]);
    }

    #[test]
    fn zsh_env_without_user_zdotdir_sets_no_marker() {
        let terminfo = PathBuf::from("/tmp/eggie-terminfo");
        let root = PathBuf::from("/tmp/eggie-integration");
        let launch = build_shell_env("zsh", "/bin/zsh", &terminfo, Some(&root), None, None);
        assert!(launch.env.contains_key("ZDOTDIR"));
        assert!(!launch.env.contains_key("EGGIE_ZDOTDIR_ORIG"));
    }

    #[test]
    fn bash_env_sets_env_var_and_inject() {
        let terminfo = PathBuf::from("/tmp/eggie-terminfo");
        let root = PathBuf::from("/tmp/eggie-integration");
        // A non-Apple bash path (e.g. Homebrew) gets full integration.
        let launch = build_shell_env(
            "bash",
            "/opt/homebrew/bin/bash",
            &terminfo,
            Some(&root),
            None,
            Some("/home/user/env.sh".to_owned()),
        );
        assert_eq!(
            launch.env.get("ENV").map(String::as_str),
            Some("/tmp/eggie-integration/bash/eggie.bash")
        );
        assert_eq!(
            launch.env.get("EGGIE_BASH_ENV").map(String::as_str),
            Some("/home/user/env.sh")
        );
        assert_eq!(
            launch.env.get("EGGIE_BASH_INJECT").map(String::as_str),
            Some("1")
        );
        assert_eq!(launch.args, vec!["--posix".to_owned()]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn apple_bin_bash_skips_integration() {
        let terminfo = PathBuf::from("/tmp/eggie-terminfo");
        let root = PathBuf::from("/tmp/eggie-integration");
        let launch = build_shell_env("bash", "/bin/bash", &terminfo, Some(&root), None, None);
        // Apple's /bin/bash cannot use the ENV-based POSIX startup path, so no injection.
        assert!(!launch.env.contains_key("ENV"));
        assert!(!launch.env.contains_key("EGGIE_BASH_INJECT"));
        assert!(launch.env.contains_key("TERM"));
    }

    #[test]
    fn non_integrated_shell_has_no_injection() {
        let terminfo = PathBuf::from("/tmp/eggie-terminfo");
        let root = PathBuf::from("/tmp/eggie-integration");
        let launch = build_shell_env("fish", "/usr/bin/fish", &terminfo, Some(&root), None, None);
        assert!(!launch.env.contains_key("ZDOTDIR"));
        assert!(!launch.env.contains_key("ENV"));
        assert!(!launch.env.contains_key("EGGIE_BASH_INJECT"));
        // Base environment is still populated.
        assert_eq!(launch.env.get("TERM").map(String::as_str), Some("alacritty"));
    }

    #[test]
    fn no_integration_root_skips_injection() {
        let terminfo = PathBuf::from("/tmp/eggie-terminfo");
        // Installation failed -> integration_root is None -> zsh gets no ZDOTDIR override.
        let launch = build_shell_env(
            "zsh",
            "/bin/zsh",
            &terminfo,
            None,
            Some("/home/user/.zsh".to_owned()),
            None,
        );
        assert!(!launch.env.contains_key("ZDOTDIR"));
        assert!(!launch.env.contains_key("EGGIE_ZDOTDIR_ORIG"));
        assert_eq!(launch.args, vec!["-l".to_owned()]);
    }

    #[test]
    fn repeated_column_reflow_keeps_images_attached_to_content_and_blank_cells() {
        let initial_size = TerminalSize {
            columns: 12,
            rows: 6,
            cell_width: 8,
            cell_height: 18,
        };
        let state = Arc::new(ListenerState::new(
            SessionId::new_v4(),
            initial_size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        ));
        let listener = DaemonEventListener(state);
        let mut terminal = Term::new(Config::default(), &GridSize(initial_size), listener);
        terminal.set_kitty_graphics_cell_size(initial_size.cell_width, initial_size.cell_height);

        for (column, character) in "ABCDEFGHIJKL".chars().enumerate() {
            terminal.grid_mut()[Line(0)][Column(column)].c = character;
        }
        terminal.grid_mut()[Line(0)][Column(11)]
            .flags
            .insert(Flags::WRAPLINE);
        for (column, character) in "MNOP@RSTUVWX".chars().enumerate() {
            terminal.grid_mut()[Line(1)][Column(column)].c = character;
        }
        terminal.grid_mut()[Line(2)][Column(0)].c = '#';

        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(1), Column(4));
        terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=7,c=2,r=2,C=1;AQIDBA==");
        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(2), Column(10));
        terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=8,c=1,r=1,C=1;AQIDBA==");
        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(5), Column(0));

        let compact_size = TerminalSize {
            columns: 8,
            ..initial_size
        };

        for _ in 0..3 {
            resize_terminal_with_history_reflow(&mut terminal, compact_size, TerminalSemanticPhase::Output, None);
            let marker = (-(terminal.grid().history_size() as i32)
                ..terminal.grid().screen_lines() as i32)
                .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
                .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '@')
                .expect("the marked text cell survives reflow");
            let line_start = (-(terminal.grid().history_size() as i32)
                ..terminal.grid().screen_lines() as i32)
                .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
                .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '#')
                .expect("the blank-anchor line survives reflow");
            let snapshot = terminal.kitty_graphics_snapshot();
            let content = snapshot
                .placements
                .iter()
                .find(|placement| placement.image.id == 7)
                .unwrap();
            let blank = snapshot
                .placements
                .iter()
                .find(|placement| placement.image.id == 8)
                .unwrap();
            assert_eq!((content.line, content.column), (marker.0, marker.1 as u32));
            assert_eq!((blank.line, blank.column), (line_start.0 + 1, 2));

            resize_terminal_with_history_reflow(&mut terminal, initial_size, TerminalSemanticPhase::Output, None);
            let marker = (-(terminal.grid().history_size() as i32)
                ..terminal.grid().screen_lines() as i32)
                .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
                .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '@')
                .expect("the marked text cell unwraps");
            let line_start = (-(terminal.grid().history_size() as i32)
                ..terminal.grid().screen_lines() as i32)
                .flat_map(|line| (0..terminal.grid().columns()).map(move |column| (line, column)))
                .find(|(line, column)| terminal.grid()[Line(*line)][Column(*column)].c == '#')
                .expect("the blank-anchor line unwraps");
            let snapshot = terminal.kitty_graphics_snapshot();
            let content = snapshot
                .placements
                .iter()
                .find(|placement| placement.image.id == 7)
                .unwrap();
            let blank = snapshot
                .placements
                .iter()
                .find(|placement| placement.image.id == 8)
                .unwrap();
            assert_eq!((content.line, content.column), (marker.0, marker.1 as u32));
            assert_eq!((blank.line, blank.column), (line_start.0, 10));
        }
    }

    #[test]
    fn scrollback_round_trip_keeps_image_attached_to_its_text_row() {
        let size = TerminalSize {
            columns: 20,
            rows: 5,
            cell_width: 8,
            cell_height: 18,
        };
        let session_id = SessionId::new_v4();
        let state = Arc::new(ListenerState::new(
            session_id,
            size,
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        ));
        let listener = DaemonEventListener(state);
        let mut terminal = Term::new(Config::default(), &GridSize(size), listener);
        terminal.set_kitty_graphics_cell_size(size.cell_width, size.cell_height);
        terminal.grid_mut()[Line(1)][Column(8)].c = 'X';
        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(1), Column(8));
        terminal.kitty_graphics_command(b"a=T,f=32,s=1,v=1,i=7,c=1,r=2,C=1;AQIDBA==");

        terminal.grid_mut().cursor.point =
            alacritty_terminal::index::Point::new(Line(4), Column(0));
        let mut parser: alacritty_terminal::vte::ansi::Processor =
            alacritty_terminal::vte::ansi::Processor::new();
        parser.advance(&mut terminal, b"\n\n\n\n\n\n\n\n");

        let bottom = snapshot_terminal(&terminal, session_id, size, String::new(), 1, 0);
        assert!(
            bottom.image_placements.is_empty(),
            "an image in scrollback must not stay pinned to the top of the live viewport"
        );

        terminal.scroll_display(Scroll::Delta(8));
        let history = snapshot_terminal(&terminal, session_id, size, String::new(), 2, 0);
        let marker_line = history
            .cells
            .iter()
            .find(|cell| cell.character == 'X')
            .map(|cell| i32::from(cell.line))
            .expect("the anchor text must be visible in scrollback");
        assert_eq!(history.image_placements.len(), 1);
        assert_eq!(history.image_placements[0].line, marker_line);

        terminal.scroll_display(Scroll::Bottom);
        assert!(
            snapshot_terminal(&terminal, session_id, size, String::new(), 3, 0)
                .image_placements
                .is_empty()
        );
        terminal.scroll_display(Scroll::Delta(8));
        let history_again = snapshot_terminal(&terminal, session_id, size, String::new(), 4, 0);
        assert_eq!(history_again.image_placements[0].line, marker_line);

        terminal.scroll_display(Scroll::Bottom);
        let compact_size = TerminalSize { columns: 6, ..size };
        resize_terminal_with_history_reflow(&mut terminal, compact_size, TerminalSemanticPhase::Output, None);
        assert!(
            snapshot_terminal(&terminal, session_id, compact_size, String::new(), 5, 0,)
                .image_placements
                .is_empty(),
            "resizing must not pull a historical image into the live viewport",
        );
        terminal.scroll_display(Scroll::Top);
        let compact_history =
            snapshot_terminal(&terminal, session_id, compact_size, String::new(), 6, 0);
        let marker = compact_history
            .cells
            .iter()
            .find(|cell| cell.character == 'X')
            .expect("the reflowed anchor cell must remain in scrollback");
        assert_eq!(compact_history.image_placements.len(), 1);
        assert_eq!(
            compact_history.image_placements[0].line,
            i32::from(marker.line)
        );
        assert_eq!(
            compact_history.image_placements[0].column,
            u32::from(marker.column)
        );

        resize_terminal_with_history_reflow(&mut terminal, size, TerminalSemanticPhase::Output, None);
        terminal.scroll_display(Scroll::Top);
        let restored_history = snapshot_terminal(&terminal, session_id, size, String::new(), 7, 0);
        let marker = restored_history
            .cells
            .iter()
            .find(|cell| cell.character == 'X')
            .expect("the unwrapped anchor cell must remain in scrollback");
        assert_eq!(restored_history.image_placements.len(), 1);
        assert_eq!(
            restored_history.image_placements[0].line,
            i32::from(marker.line)
        );
        assert_eq!(
            restored_history.image_placements[0].column,
            u32::from(marker.column)
        );
    }

    #[test]
    fn resizing_a_session_updates_its_snapshot_grid_and_revision() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize::default(),
            TerminalAppearance::default(),
        )
        .unwrap();
        let previous_revision = session.snapshot().revision;
        let size = TerminalSize {
            columns: 72,
            rows: 18,
            cell_width: 9,
            cell_height: 19,
        };

        session.resize(size).unwrap();

        let snapshot = session.snapshot();
        assert_eq!(snapshot.size, size);
        assert!(snapshot.revision > previous_revision);
        session.terminate();
    }

    #[test]
    fn pty_output_is_parsed_into_an_alacritty_snapshot() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let cwd = std::env::current_dir().unwrap();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            cwd,
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input(
                b"printf '\\033]4;1;#123456\\007\\033[1;3;4;9;31;58;2;17;34;51mEGGIE_PTY_OK\\033[0m\\n'\r"
                    .to_vec(),
                1,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            let snapshot = session.snapshot();
            let dynamic_color_ready = snapshot.color_overrides.contains(&TerminalColorOverride {
                index: 1,
                color: 0x123456ff,
            });
            let styled_output_ready = snapshot.cells.iter().any(|cell| {
                cell.character == 'E'
                    && Flags::from_bits_retain(cell.flags)
                        .contains(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE | Flags::STRIKEOUT)
            });
            if dynamic_color_ready && styled_output_ready {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "terminal output did not arrive");
            thread::sleep(Duration::from_millis(20));
        };

        assert!(snapshot.revision > 0);
        assert!(snapshot.color_overrides.contains(&TerminalColorOverride {
            index: 1,
            color: 0x123456ff,
        }));
        let styled_cell = snapshot
            .cells
            .iter()
            .find(|cell| {
                cell.character == 'E'
                    && Flags::from_bits_retain(cell.flags)
                        .contains(Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE | Flags::STRIKEOUT)
            })
            .expect("styled terminal cell was not captured");
        assert_eq!(
            styled_cell.foreground,
            eggie_protocol::TerminalColor::Named(1)
        );
        assert_eq!(
            styled_cell.underline_color,
            Some(eggie_protocol::TerminalColor::Rgb(0x112233ff))
        );
        let flags = Flags::from_bits_retain(styled_cell.flags);
        assert!(flags.contains(Flags::BOLD));
        assert!(flags.contains(Flags::ITALIC));
        assert!(flags.contains(Flags::UNDERLINE));
        assert!(flags.contains(Flags::STRIKEOUT));
        session.terminate();
    }

    #[test]
    fn terminal_search_finds_counts_and_navigates_matches() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        // Print three lines that each contain the needle so the search has multiple matches.
        session
            .input(
                b"printf 'needle one\\nneedle two\\nneedle three\\n'\r".to_vec(),
                1,
            )
            .unwrap();

        // Wait until all three needle lines have been parsed into the snapshot.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            let needle_rows = snapshot
                .cells
                .iter()
                .filter(|cell| cell.character == 'n' && cell.column == 0)
                .count();
            if needle_rows >= 3 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "needle output did not arrive in the snapshot"
            );
            thread::sleep(Duration::from_millis(20));
        }

        // A fresh forward search should find a match and count every occurrence. The exact count
        // is not asserted because the shell echoes the command line (which also contains the
        // needle); instead we verify the invariants that must hold regardless of echo.
        let first = session
            .search(TerminalSearchRequest {
                query: "needle".to_owned(),
                regex: false,
                direction: TerminalSearchDirection::Forward,
                fresh: true,
            })
            .unwrap();
        assert!(
            first.total >= 3,
            "expected at least three matches for 'needle', got {}",
            first.total
        );
        assert_eq!(first.index, 0, "fresh search should start at the first match");
        let first_active = first.active.expect("active match should be present");
        assert!(
            first.matches.iter().any(|m| *m == first_active),
            "the active match should be among the visible highlights"
        );
        let total = first.total;

        // Advancing forward moves to the next match without changing the total.
        let second = session
            .search(TerminalSearchRequest {
                query: "needle".to_owned(),
                regex: false,
                direction: TerminalSearchDirection::Forward,
                fresh: false,
            })
            .unwrap();
        assert_eq!(second.total, total);
        assert_eq!(second.index, 1, "forward navigation should advance the index");

        // A query with no matches reports nothing.
        let missing = session
            .search(TerminalSearchRequest {
                query: "this-string-does-not-exist".to_owned(),
                regex: false,
                direction: TerminalSearchDirection::Forward,
                fresh: true,
            })
            .unwrap();
        assert_eq!(missing.total, 0);
        assert!(missing.active.is_none());

        // A regex search matches the same needles as the literal search.
        let regex_result = session
            .search(TerminalSearchRequest {
                query: "n..dle".to_owned(),
                regex: true,
                direction: TerminalSearchDirection::Forward,
                fresh: true,
            })
            .unwrap();
        assert_eq!(
            regex_result.total, total,
            "regex 'n..dle' should match the same cells as literal 'needle'"
        );

        session.terminate();
    }

    #[test]
    fn select_all_and_selection_text_span_the_whole_scrollback() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        // Print more lines than the viewport so the earliest ones scroll into history.
        session
            .input(
                b"printf 'FIRSTLINE\\n'; for i in $(seq 1 60); do echo filler $i; done; printf 'LASTLINE\\n'\r".to_vec(),
                1,
            )
            .unwrap();

        // Wait until LASTLINE has been parsed into the snapshot.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            let has_last = snapshot
                .plain_lines()
                .iter()
                .any(|line| line.contains("LASTLINE"));
            if has_last {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "LASTLINE did not arrive in the snapshot"
            );
            thread::sleep(Duration::from_millis(20));
        }

        // FIRSTLINE is now in scrollback (not visible). Select-all must reach it and LASTLINE.
        session.select_all().unwrap();
        let text = session
            .selection_text()
            .unwrap()
            .expect("select all should produce text");
        assert!(
            text.contains("FIRSTLINE"),
            "select all should include the scrollback top; got:\n{text}"
        );
        assert!(
            text.contains("LASTLINE"),
            "select all should include the buffer bottom; got:\n{text}"
        );

        // Clearing drops the selection text entirely.
        session.selection_clear().unwrap();
        assert!(session.selection_text().unwrap().is_none());

        session.terminate();
    }

    #[test]
    fn scroll_to_commands_move_the_viewport_across_scrollback() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input(
                b"printf 'FIRSTLINE\\n'; for i in $(seq 1 60); do echo filler $i; done; printf 'LASTLINE\\n'\r".to_vec(),
                1,
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            if snapshot
                .plain_lines()
                .iter()
                .any(|line| line.contains("LASTLINE"))
            {
                break;
            }
            assert!(Instant::now() < deadline, "LASTLINE did not arrive");
            thread::sleep(Duration::from_millis(20));
        }

        // FIRSTLINE is in scrollback and not visible at the live bottom.
        assert!(
            !session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("FIRSTLINE")),
            "FIRSTLINE should start off-screen in scrollback"
        );

        // Jump to the top: the earliest scrollback line becomes visible.
        session.scroll_to(TerminalScrollCommand::Top).unwrap();
        assert!(
            session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("FIRSTLINE")),
            "scroll-to-top should reveal the oldest scrollback line"
        );

        // Jump back to the bottom: the live tail is visible again.
        session.scroll_to(TerminalScrollCommand::Bottom).unwrap();
        assert!(
            session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("LASTLINE")),
            "scroll-to-bottom should return to the live viewport"
        );
        assert!(
            !session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("FIRSTLINE")),
            "scroll-to-bottom should leave scrollback again"
        );

        session.terminate();
    }

    #[test]
    fn keystroke_snaps_the_viewport_back_to_the_live_bottom() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input(
                b"printf 'FIRSTLINE\\n'; for i in $(seq 1 60); do echo filler $i; done; printf 'LASTLINE\\n'\r".to_vec(),
                1,
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("LASTLINE"))
            {
                break;
            }
            assert!(Instant::now() < deadline, "LASTLINE did not arrive");
            thread::sleep(Duration::from_millis(20));
        }

        // Scroll up into history so the live bottom is off-screen.
        session.scroll_to(TerminalScrollCommand::Top).unwrap();
        assert!(
            session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("FIRSTLINE")),
            "precondition: scrolled into scrollback"
        );

        // Typing must snap the viewport back to the bottom, even before the echo arrives.
        session.input(b"x".to_vec(), 2).unwrap();
        assert!(
            session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("LASTLINE")),
            "a keystroke should scroll the viewport back to the live bottom"
        );

        session.terminate();
    }

    #[test]
    fn interactive_selection_projects_into_the_visible_viewport() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input(b"printf 'ALPHA BRAVO CHARLIE\\n'\r".to_vec(), 1)
            .unwrap();

        // Wait until the printed line has arrived, then wait for the terminal to go quiescent (the
        // trailing shell prompt can scroll the grid). Take the target row and its expected text from
        // the SAME settled snapshot so the assertion tests the viewport→absolute mapping, not timing.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            let present = snapshot.plain_lines().iter().any(|line| {
                line.contains("ALPHA BRAVO CHARLIE") && !line.contains("printf")
            });
            if present {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "printed line did not arrive in the snapshot"
            );
            thread::sleep(Duration::from_millis(20));
        }
        // Settle: require the revision to hold steady across a short window.
        let mut last_revision = session.snapshot().revision;
        loop {
            thread::sleep(Duration::from_millis(120));
            let revision = session.snapshot().revision;
            if revision == last_revision {
                break;
            }
            last_revision = revision;
            assert!(
                Instant::now() < deadline,
                "terminal did not go quiescent for the selection test"
            );
        }

        let settled = session.snapshot();
        let lines = settled.plain_lines();
        let target_line = lines
            .iter()
            .position(|line| line.contains("ALPHA BRAVO CHARLIE") && !line.contains("printf"))
            .expect("printed line should be present in the settled snapshot")
            as u16;
        let expected: String = lines[target_line as usize].chars().take(5).collect();

        // Select the first five columns of that row.
        session
            .selection_start(
                TerminalCellPosition {
                    line: target_line,
                    column: 0,
                },
                TerminalSelectionSide::Left,
                TerminalSelectionKind::Simple,
            )
            .unwrap();
        session
            .selection_update(
                TerminalCellPosition {
                    line: target_line,
                    column: 4,
                },
                TerminalSelectionSide::Right,
            )
            .unwrap();

        let text = session
            .selection_text()
            .unwrap()
            .expect("interactive selection should produce text");
        assert_eq!(text, expected);
        assert_eq!(expected, "ALPHA");

        // The selection projects into the visible viewport on the same row.
        let projected = session
            .snapshot()
            .selection
            .expect("visible selection should project into the snapshot");
        assert_eq!(projected.start.line, target_line);
        assert_eq!(projected.start.column, 0);
        assert_eq!(projected.end.line, target_line);

        session.terminate();
    }

    #[test]
    fn detects_bare_url_and_trims_trailing_punctuation() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input(b"printf 'see https://example.com. now\\n'\r".to_vec(), 1)
            .unwrap();

        // Wait until a detected link appears for the printed URL (ignoring the echoed command line).
        let deadline = Instant::now() + Duration::from_secs(5);
        let link = loop {
            let snapshot = session.snapshot();
            let link = snapshot
                .detected_links
                .iter()
                .find(|link| link.url == "https://example.com")
                .cloned();
            if let Some(link) = link {
                break link;
            }
            assert!(
                Instant::now() < deadline,
                "detected link did not arrive; links = {:?}",
                session.snapshot().detected_links
            );
            thread::sleep(Duration::from_millis(20));
        };

        // The trailing period must not be part of the URL, so the range ends before it.
        let snapshot = session.snapshot();
        let line = snapshot
            .plain_lines()
            .into_iter()
            .find(|line| line.contains("see https://example.com. now"))
            .expect("printed line should be present");
        let dot_column = line.find("https://example.com.").unwrap() + "https://example.com".len();
        assert!(
            (link.end.column as usize) < dot_column,
            "range should stop before the trailing period at column {dot_column}, got end {}",
            link.end.column
        );

        session.terminate();
    }

    #[test]
    fn refine_url_range_strips_trailing_and_unbalanced_punctuation() {
        // A pure-logic check of the trimming rules independent of the grid: build a tiny term,
        // print candidates, and assert the cleaned text. Uses the same trimming that runs on real
        // matches, exercised through short strings.
        for (raw, expected) in [
            ("https://example.com.", "https://example.com"),
            ("http://a.com,", "http://a.com"),
            ("https://a.com/x)", "https://a.com/x"),
            ("https://a.com/wiki_(foo)", "https://a.com/wiki_(foo)"),
        ] {
            let cleaned = trim_url_trailing(raw);
            assert_eq!(cleaned, expected, "trimming {raw}");
        }
    }

    #[test]
    fn explicit_osc8_hyperlink_stays_out_of_detected_links() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        // OSC 8 hyperlink whose visible text is NOT itself a URL, so a match in detected_links could
        // only come from the auto-detector (which must ignore explicit links).
        session
            .input(
                b"printf '\\033]8;;https://osc8.example\\033\\\\clickme\\033]8;;\\033\\\\\\n'\r"
                    .to_vec(),
                1,
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            // Find the cell that carries the explicit OSC 8 link (its visible text is "clickme").
            let osc8_cell = snapshot
                .cells
                .iter()
                .find(|cell| cell.hyperlink.as_deref() == Some("https://osc8.example"));
            if let Some(cell) = osc8_cell {
                // That cell must not be covered by any auto-detected link range: explicit OSC 8
                // links are carried on `hyperlink`, never duplicated into `detected_links`. (The
                // echoed command line may separately contain the literal URL text as a real bare
                // URL; that is a different cell and legitimately detectable.)
                let covered = snapshot.detected_links.iter().any(|link| {
                    link.start.line == cell.line
                        && cell.column >= link.start.column
                        && cell.column <= link.end.column
                });
                assert!(
                    !covered,
                    "the OSC 8 link cell must not be covered by a detected bare URL"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "OSC 8 hyperlink cell did not arrive"
            );
            thread::sleep(Duration::from_millis(20));
        }

        session.terminate();
    }

    #[test]
    fn terminal_search_overlapping_regex_navigation_advances_without_repeat() {
        // Regression for overlapping matches: navigation must advance past a match's END, so a
        // pattern like "aa" over "aaaa" does not re-find the same overlapping match, and the
        // reported index moves forward instead of sticking.
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input(b"printf 'zzzz aaaa zzzz\\n'\r".to_vec(), 1)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            let has_aaaa = snapshot
                .cells
                .iter()
                .any(|cell| cell.character == 'a' && cell.column > 0);
            if has_aaaa {
                break;
            }
            assert!(Instant::now() < deadline, "aaaa output did not arrive");
            thread::sleep(Duration::from_millis(20));
        }

        let first = session
            .search(TerminalSearchRequest {
                query: "aa".to_owned(),
                regex: true,
                direction: TerminalSearchDirection::Forward,
                fresh: true,
            })
            .unwrap();
        let first_active = first.active.expect("first overlapping match should be found");
        assert_eq!(first.index, 0, "fresh search starts at the first match");

        // Advancing forward must land on a different, later match (no overlap re-find).
        let second = session
            .search(TerminalSearchRequest {
                query: "aa".to_owned(),
                regex: true,
                direction: TerminalSearchDirection::Forward,
                fresh: false,
            })
            .unwrap();
        let second_active = second.active.expect("a second match should exist");
        assert_ne!(
            (first_active.start.line, first_active.start.column),
            (second_active.start.line, second_active.start.column),
            "forward navigation must not re-find the same overlapping match"
        );
        assert_eq!(
            second.total, first.total,
            "the total match count is stable across navigation"
        );
        assert!(
            second.index < second.total,
            "the active index stays within range"
        );

        session.terminate();
    }

    #[test]
    fn non_bmp_utf8_input_round_trips_through_the_pty_and_alacritty() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();

        session
            .input("printf 'EGGIE🙂OK\\n'\r".as_bytes().to_vec(), 1)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = session.snapshot();
            if snapshot.cells.iter().any(|cell| cell.character == '🙂') {
                let emoji = snapshot
                    .cells
                    .iter()
                    .find(|cell| cell.character == '🙂')
                    .unwrap();
                assert!(Flags::from_bits_retain(emoji.flags).contains(Flags::WIDE_CHAR));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "emoji input did not round-trip through the terminal"
            );
            thread::sleep(Duration::from_millis(20));
        }

        session.terminate();
    }

    #[test]
    fn session_summary_tracks_foreground_process_and_working_directory() {
        let _pty_guard = PTY_TEST_LOCK.lock();
        let cwd = std::env::current_dir().unwrap();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            cwd,
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();
        let expected_directory = fs::canonicalize(std::env::temp_dir()).unwrap();
        session
            .input(
                format!(
                    "printf 'EGGIE_METADATA_READY\\n'; cd '{}'\r",
                    expected_directory.display()
                )
                .into_bytes(),
                1,
            )
            .unwrap();

        let shell_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if session
                .snapshot()
                .plain_lines()
                .iter()
                .any(|line| line.contains("EGGIE_METADATA_READY"))
            {
                break;
            }
            assert!(
                Instant::now() < shell_deadline,
                "shell did not execute metadata test setup"
            );
            thread::sleep(Duration::from_millis(20));
        }

        let directory_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let summary = session.summary();
            if summary.current_directory == expected_directory {
                break;
            }
            assert!(
                Instant::now() < directory_deadline,
                "terminal working directory did not update: {}; process={} pid={} shell_pid={}",
                summary.current_directory.display(),
                summary.current_process.name,
                summary.current_process.pid,
                summary.shell_pid,
            );
            thread::sleep(Duration::from_millis(20));
        }

        session.input(b"sleep 5\r".to_vec(), 2).unwrap();
        let process_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let summary = session.summary();
            if summary.current_process.name == "sleep" {
                assert_ne!(summary.current_process.pid, 0);
                break;
            }
            assert!(
                Instant::now() < process_deadline,
                "foreground process did not update: {}",
                summary.current_process.name
            );
            thread::sleep(Duration::from_millis(20));
        }

        session.input(vec![0x03], 3).unwrap();
        session.terminate();
    }

    #[test]
    fn terminal_input_modes_follow_alacritty_mode_precedence() {
        let mode = TermMode::MOUSE_DRAG
            | TermMode::SGR_MOUSE
            | TermMode::FOCUS_IN_OUT
            | TermMode::ALT_SCREEN
            | TermMode::ALTERNATE_SCROLL;
        assert_eq!(
            terminal_input_modes(mode),
            TerminalInputModes {
                mouse_tracking: TerminalMouseTracking::Drag,
                mouse_encoding: TerminalMouseEncoding::Sgr,
                focus_reporting: true,
                alternate_screen: true,
                alternate_scroll: true,
                paste_events: false,
                kitty_keyboard_flags: 0,
            }
        );
        assert_eq!(
            terminal_input_modes(mode | TermMode::MOUSE_MOTION).mouse_tracking,
            TerminalMouseTracking::Motion
        );
        assert_eq!(
            terminal_input_modes(mode | TermMode::VI).mouse_tracking,
            TerminalMouseTracking::Disabled
        );
    }

    #[test]
    fn legacy_mouse_reports_press_release_modifiers_and_coordinate_limits() {
        let position = TerminalMousePosition {
            column: 4,
            row: 2,
            pixel_x: 0,
            pixel_y: 0,
        };
        let press = TerminalMouseEvent {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::Left),
            position: TerminalMousePosition {
                column: 0,
                row: 0,
                pixel_x: 0,
                pixel_y: 0,
            },
            modifiers: TerminalModifiers::default(),
        };
        assert_eq!(
            mouse_report_bytes(TermMode::MOUSE_REPORT_CLICK, 0, press),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );

        let release = TerminalMouseEvent {
            action: TerminalMouseAction::Release,
            button: Some(TerminalMouseButton::Right),
            position,
            modifiers: TerminalModifiers {
                control: true,
                ..TerminalModifiers::default()
            },
        };
        assert_eq!(
            mouse_report_bytes(TermMode::MOUSE_REPORT_CLICK, 0, release),
            Some(vec![0x1b, b'[', b'M', 51, 37, 35])
        );

        assert!(
            mouse_report_from_code(
                TermMode::MOUSE_REPORT_CLICK,
                0,
                TerminalMousePosition {
                    column: 222,
                    row: 222,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                0,
                false,
                TerminalModifiers::default(),
            )
            .is_some()
        );
        assert!(
            mouse_report_from_code(
                TermMode::MOUSE_REPORT_CLICK,
                0,
                TerminalMousePosition {
                    column: 223,
                    row: 0,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                0,
                false,
                TerminalModifiers::default(),
            )
            .is_none()
        );
    }

    #[test]
    fn utf8_and_sgr_mouse_reports_preserve_extended_coordinates() {
        let utf8_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::UTF8_MOUSE;
        assert_eq!(
            mouse_report_from_code(
                utf8_mode,
                0,
                TerminalMousePosition {
                    column: 95,
                    row: 0,
                    pixel_x: 0,
                    pixel_y: 0
                },
                0,
                false,
                TerminalModifiers::default(),
            ),
            Some(vec![0x1b, b'[', b'M', 32, 0xc2, 0x80, 33])
        );
        assert!(
            mouse_report_from_code(
                utf8_mode,
                0,
                TerminalMousePosition {
                    column: 2014,
                    row: 2014,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                0,
                false,
                TerminalModifiers::default(),
            )
            .is_some()
        );
        assert!(
            mouse_report_from_code(
                utf8_mode,
                0,
                TerminalMousePosition {
                    column: 2015,
                    row: 0,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                0,
                false,
                TerminalModifiers::default(),
            )
            .is_none()
        );

        let sgr_mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let release = TerminalMouseEvent {
            action: TerminalMouseAction::Release,
            button: Some(TerminalMouseButton::Right),
            position: TerminalMousePosition {
                column: 4,
                row: 2,
                pixel_x: 0,
                pixel_y: 0,
            },
            modifiers: TerminalModifiers {
                alt: true,
                ..TerminalModifiers::default()
            },
        };
        assert_eq!(
            mouse_report_bytes(sgr_mode, 0, release),
            Some(b"\x1b[<10;5;3m".to_vec())
        );
        assert!(mouse_report_bytes(sgr_mode, 3, release).is_none());
    }

    #[test]
    fn sgr_pixel_mouse_reports_viewport_pixels_instead_of_cells() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE | TermMode::SGR_PIXEL_MOUSE;
        let press = TerminalMouseEvent {
            action: TerminalMouseAction::Press,
            button: Some(TerminalMouseButton::Left),
            position: TerminalMousePosition {
                column: 4,
                row: 2,
                pixel_x: 35,
                pixel_y: 47,
            },
            modifiers: TerminalModifiers::default(),
        };
        assert_eq!(
            mouse_report_bytes(mode, 0, press),
            Some(b"\x1b[<0;36;48M".to_vec())
        );
    }

    #[test]
    fn mouse_motion_respects_drag_any_motion_shift_and_vi_modes() {
        let motion = |button, modifiers| TerminalMouseEvent {
            action: TerminalMouseAction::Move,
            button,
            position: TerminalMousePosition {
                column: 1,
                row: 1,
                pixel_x: 0,
                pixel_y: 0,
            },
            modifiers,
        };
        let drag_mode = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        assert!(
            mouse_report_bytes(drag_mode, 0, motion(None, TerminalModifiers::default())).is_none()
        );
        assert_eq!(
            mouse_report_bytes(
                drag_mode,
                0,
                motion(
                    Some(TerminalMouseButton::Middle),
                    TerminalModifiers::default()
                ),
            ),
            Some(b"\x1b[<33;2;2M".to_vec())
        );
        assert!(
            mouse_report_bytes(
                drag_mode,
                0,
                motion(
                    Some(TerminalMouseButton::Left),
                    TerminalModifiers {
                        shift: true,
                        ..TerminalModifiers::default()
                    },
                ),
            )
            .is_none()
        );
        assert_eq!(
            mouse_report_bytes(
                TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE,
                0,
                motion(
                    None,
                    TerminalModifiers {
                        control: true,
                        ..TerminalModifiers::default()
                    },
                ),
            ),
            Some(b"\x1b[<51;2;2M".to_vec())
        );
        assert!(
            mouse_report_bytes(
                TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE | TermMode::VI,
                0,
                motion(None, TerminalModifiers::default()),
            )
            .is_none()
        );
    }

    #[test]
    fn input_queue_coalesces_motion_and_scroll_without_crossing_event_barriers() {
        let session_id = SessionId::new_v4();
        let mut motion = QueuedTerminalInput::Mouse {
            session_id,
            event: TerminalMouseEvent {
                action: TerminalMouseAction::Move,
                button: None,
                position: TerminalMousePosition {
                    column: 1,
                    row: 1,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers::default(),
            },
        };
        assert!(
            motion
                .merge(QueuedTerminalInput::Mouse {
                    session_id,
                    event: TerminalMouseEvent {
                        action: TerminalMouseAction::Move,
                        button: None,
                        position: TerminalMousePosition {
                            column: 3,
                            row: 2,
                            pixel_x: 0,
                            pixel_y: 0
                        },
                        modifiers: TerminalModifiers::default(),
                    },
                })
                .is_ok()
        );
        let ClientRequest::Mouse { event, .. } = motion.request() else {
            panic!("coalesced motion changed request kind");
        };
        assert_eq!(
            event.position,
            TerminalMousePosition {
                column: 3,
                row: 2,
                pixel_x: 0,
                pixel_y: 0
            }
        );

        let scroll_event = |y| TerminalScrollEvent {
            delta: eggie_protocol::TerminalScrollDelta {
                x: 0,
                y,
                unit: TerminalScrollUnit::Pixels,
            },
            phase: TerminalScrollPhase::Moved,
            position: TerminalMousePosition {
                column: 0,
                row: 0,
                pixel_x: 0,
                pixel_y: 0,
            },
            modifiers: TerminalModifiers::default(),
        };
        let mut scroll = QueuedTerminalInput::Scroll {
            session_id,
            event: scroll_event(100),
        };
        assert!(
            scroll
                .merge(QueuedTerminalInput::Scroll {
                    session_id,
                    event: scroll_event(250),
                })
                .is_ok()
        );
        let ClientRequest::Scroll { event, .. } = scroll.request() else {
            panic!("coalesced scroll changed request kind");
        };
        assert_eq!(event.delta.y, 350);
    }

    #[test]
    fn mouse_and_focus_reports_round_trip_through_the_real_pty() {
        if !Command::new("sh")
            .args(["-c", "command -v python3 >/dev/null 2>&1"])
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }

        let _pty_guard = PTY_TEST_LOCK.lock();
        let session = TerminalSession::spawn(
            ProjectId::new_v4(),
            std::env::current_dir().unwrap(),
            TerminalSize {
                columns: 80,
                rows: 24,
                ..TerminalSize::default()
            },
            TerminalAppearance::default(),
        )
        .unwrap();
        let script = "import os,sys,termios,tty; old=termios.tcgetattr(0); tty.setraw(0); sys.stdout.write('\\x1b[?1002h\\x1b[?1006h\\x1b[?1004h'); sys.stdout.flush(); data=b''; exec(\"while len(data)<12:\\n data+=os.read(0,12-len(data))\"); sys.stdout.write('\\x1b[?1002l\\x1b[?1006l\\x1b[?1004l\\x1b[?1049h\\x1b[?1007h'); sys.stdout.flush(); scroll=b''; exec(\"while len(scroll)<9:\\n scroll+=os.read(0,9-len(scroll))\"); sys.stdout.write('\\x1b[?1049l'); sys.stdout.flush(); termios.tcsetattr(0,termios.TCSADRAIN,old); print('\\r\\nEGGIE_POINTER:'+data.hex()+':'+scroll.hex())";
        session
            .input(
                format!("python3 -c \"{}\"\r", script.replace('"', "\\\"")).into_bytes(),
                1,
            )
            .unwrap();

        let mode_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let modes = session.snapshot().input_modes;
            if modes.mouse_tracking == TerminalMouseTracking::Drag
                && modes.mouse_encoding == TerminalMouseEncoding::Sgr
                && modes.focus_reporting
            {
                break;
            }
            assert!(
                Instant::now() < mode_deadline,
                "child application did not enable pointer protocols: {modes:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }

        session
            .mouse(TerminalMouseEvent {
                action: TerminalMouseAction::Press,
                button: Some(TerminalMouseButton::Left),
                position: TerminalMousePosition {
                    column: 4,
                    row: 2,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers::default(),
            })
            .unwrap();
        session.focus(false).unwrap();

        let alternate_scroll_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let modes = session.snapshot().input_modes;
            if modes.alternate_screen && modes.alternate_scroll && !modes.captures_mouse() {
                break;
            }
            assert!(
                Instant::now() < alternate_scroll_deadline,
                "child application did not enable alternate scroll: {modes:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        session
            .scroll(TerminalScrollEvent {
                delta: eggie_protocol::TerminalScrollDelta {
                    x: 0,
                    y: TERMINAL_SCROLL_DELTA_SCALE,
                    unit: TerminalScrollUnit::Lines,
                },
                phase: TerminalScrollPhase::Moved,
                position: TerminalMousePosition {
                    column: 4,
                    row: 2,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers::default(),
            })
            .unwrap();

        let output_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let lines = session.snapshot().plain_lines();
            if lines.iter().any(|line| {
                line.contains("EGGIE_POINTER:1b5b3c303b353b334d1b5b4f:1b4f411b4f411b4f41")
            }) {
                break;
            }
            assert!(
                Instant::now() < output_deadline,
                "mouse/focus reports did not round-trip through PTY: {lines:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }

        session
            .input(
                b"for i in {1..40}; do echo EGGIE_SCROLL_$i; done\r".to_vec(),
                2,
            )
            .unwrap();
        let history_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let lines = session.snapshot().plain_lines();
            if lines.iter().any(|line| line.contains("EGGIE_SCROLL_40")) {
                break;
            }
            assert!(
                Instant::now() < history_deadline,
                "scrollback fixture did not reach the terminal: {lines:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(session.terminal.lock().grid().display_offset(), 0);
        let bottom_snapshot = session.snapshot();
        session
            .scroll(TerminalScrollEvent {
                delta: eggie_protocol::TerminalScrollDelta {
                    x: 0,
                    y: TERMINAL_SCROLL_DELTA_SCALE,
                    unit: TerminalScrollUnit::Lines,
                },
                phase: TerminalScrollPhase::Moved,
                position: TerminalMousePosition {
                    column: 4,
                    row: 2,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers::default(),
            })
            .unwrap();
        assert_eq!(session.terminal.lock().grid().display_offset(), 3);
        let history_snapshot = session.snapshot();
        assert!(history_snapshot.revision > bottom_snapshot.revision);
        assert_ne!(
            history_snapshot.plain_lines(),
            bottom_snapshot.plain_lines()
        );
        assert!(!history_snapshot.cells.is_empty());
        assert!(
            history_snapshot
                .cells
                .iter()
                .all(|cell| cell.line < history_snapshot.size.rows)
        );
        assert_eq!(history_snapshot.cursor_shape, TerminalCursorShape::Hidden);

        session
            .scroll(TerminalScrollEvent {
                delta: eggie_protocol::TerminalScrollDelta {
                    x: 0,
                    y: -TERMINAL_SCROLL_DELTA_SCALE,
                    unit: TerminalScrollUnit::Lines,
                },
                phase: TerminalScrollPhase::Moved,
                position: TerminalMousePosition {
                    column: 4,
                    row: 2,
                    pixel_x: 0,
                    pixel_y: 0,
                },
                modifiers: TerminalModifiers::default(),
            })
            .unwrap();
        assert_eq!(session.terminal.lock().grid().display_offset(), 0);
        let returned_snapshot = session.snapshot();
        assert!(returned_snapshot.revision > history_snapshot.revision);
        assert_ne!(
            returned_snapshot.plain_lines(),
            history_snapshot.plain_lines()
        );
        assert!(
            returned_snapshot
                .plain_lines()
                .iter()
                .any(|line| line.contains("EGGIE_SCROLL_40")),
            "scrolling back to offset zero must restore the latest output"
        );
        session.terminate();
    }

    fn osc_listener_state() -> ListenerState {
        ListenerState::new(
            SessionId::new_v4(),
            TerminalSize::default(),
            TerminalAppearance::default(),
            Arc::new(AtomicU64::new(0)),
        )
    }

    #[test]
    fn bell_is_forwarded_once_and_throttles_a_burst() {
        let state = osc_listener_state();
        // First bell is forwarded and publishes an OSC event.
        assert!(state.ring_bell());
        let update = state
            .osc_events
            .wait_after(0, Duration::ZERO)
            .expect("first bell publishes an OSC event");
        assert_eq!(update.events.len(), 1);
        assert_eq!(update.events[0].payload, TerminalOscEventPayload::Bell);

        // A second bell inside the throttle window is dropped, so the revision does not advance.
        let revision_after_first = state.osc_events.revision();
        assert!(!state.ring_bell());
        assert_eq!(state.osc_events.revision(), revision_after_first);
    }

    #[test]
    fn osc_reported_locations_distinguish_local_and_remote_hosts() {
        let local = parse_reported_location("file:///Users/test/My%20Project").unwrap();
        assert_eq!(local.path, "/Users/test/My Project");
        assert!(local.local);
        assert_eq!(local.user, None);

        let remote =
            parse_reported_location("file://alice@example.invalid/home/alice/repo").unwrap();
        assert_eq!(remote.user.as_deref(), Some("alice"));
        assert_eq!(remote.host.as_deref(), Some("example.invalid"));
        assert_eq!(remote.path, "/home/alice/repo");
        assert!(!remote.local);

        let state = osc_listener_state();
        state.report_working_directory("file:///tmp/working");
        state.update_remote_host("bob@remote.invalid");
        state.report_working_directory("/home/bob/repo");
        assert_eq!(
            *state.reported_location.read(),
            Some(TerminalReportedLocation {
                user: Some("bob".to_owned()),
                host: Some("remote.invalid".to_owned()),
                path: "/home/bob/repo".to_owned(),
                local: false,
            })
        );
    }

    #[test]
    fn osc_133_tracks_prompt_command_output_and_aborted_input() {
        let mut tracker = ShellIntegrationTracker::default();
        tracker.update(SemanticPrompt {
            action: SemanticPromptAction::PromptStart,
            options: String::new(),
        }, 0, 0);
        tracker.update(SemanticPrompt {
            action: SemanticPromptAction::InputStart,
            options: "cmdline=echo hello".to_owned(),
        }, 0, 0);
        tracker.update(SemanticPrompt {
            action: SemanticPromptAction::CommandFinished,
            options: "130".to_owned(),
        }, 0, 0);
        assert_eq!(tracker.snapshot().phase, TerminalSemanticPhase::None);
        assert!(tracker.snapshot().history.is_empty());

        tracker.update(SemanticPrompt {
            action: SemanticPromptAction::InputStart,
            options: "cmdline=printf ok".to_owned(),
        }, 0, 0);
        tracker.update(SemanticPrompt {
            action: SemanticPromptAction::OutputStart,
            options: String::new(),
        }, 0, 0);
        tracker.update(SemanticPrompt {
            action: SemanticPromptAction::CommandFinished,
            options: "7".to_owned(),
        }, 0, 0);
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.phase, TerminalSemanticPhase::None);
        assert_eq!(snapshot.history.len(), 1);
        assert_eq!(
            snapshot.history[0].command_line.as_deref(),
            Some("printf ok")
        );
        assert_eq!(snapshot.history[0].exit_code, Some(7));
    }

    #[test]
    fn kitty_notification_multipart_is_bounded_sanitized_and_published_once() {
        let state = osc_listener_state();
        state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
            code: 99,
            payload: "i=build\u{1b}:p=title:d=0;Build".to_owned(),
            terminator: "\x1b\\".to_owned(),
        });
        assert_eq!(state.osc_events.revision(), 0);
        state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
            code: 99,
            payload: "i=build\u{1b}:p=body;Finished".to_owned(),
            terminator: "\x1b\\".to_owned(),
        });

        let update = state
            .osc_events
            .wait_after(0, Duration::ZERO)
            .expect("completed notification publishes");
        assert_eq!(update.events.len(), 1);
        let TerminalOscEventPayload::Notification { notification } = &update.events[0].payload
        else {
            panic!("unexpected OSC event: {:?}", update.events[0].payload);
        };
        assert_eq!(notification.id, "build");
        assert_eq!(notification.title, "Build");
        assert_eq!(notification.body, "Finished");
        assert!(state.live_notifications.lock().contains("build"));

        state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
            code: 99,
            payload: "p=close;".to_owned(),
            terminator: "\x1b\\".to_owned(),
        });
        assert_eq!(
            state.osc_events.revision(),
            1,
            "close without an id is a no-op"
        );
        state.handle_notification(alacritty_terminal::vte::ansi::DesktopNotification {
            code: 99,
            payload: "i=build:p=close;".to_owned(),
            terminator: "\x1b\\".to_owned(),
        });
        assert!(!state.live_notifications.lock().contains("build"));
        assert_eq!(state.osc_events.revision(), 2);
    }

    #[test]
    fn kitty_rich_clipboard_commits_multipart_data_and_aliases_once() {
        let state = osc_listener_state();
        let text_mime = BASE64.encode("text/plain;charset=utf-8");
        let alias_mime = BASE64.encode("text/plain");
        state.handle_kitty_clipboard("type=write:id=clip", "\x1b\\");
        state.handle_kitty_clipboard(
            &format!("type=walias:id=clip:mime={text_mime};{alias_mime}"),
            "\x1b\\",
        );
        state.handle_kitty_clipboard(
            &format!(
                "type=wdata:id=clip:mime={text_mime};{}",
                BASE64.encode("hello")
            ),
            "\x1b\\",
        );
        state.handle_kitty_clipboard("type=wdata:id=clip", "\x1b\\");

        let update = state
            .osc_events
            .wait_after(0, Duration::ZERO)
            .expect("clipboard write publishes");
        let TerminalOscEventPayload::ClipboardWrite { contents, .. } = &update.events[0].payload
        else {
            panic!("unexpected OSC event: {:?}", update.events[0].payload);
        };
        assert_eq!(contents.len(), 2);
        assert!(contents.iter().all(|content| content.data == b"hello"));
        assert!(
            contents
                .iter()
                .any(|content| content.mime_type == "text/plain")
        );
        assert!(
            contents
                .iter()
                .any(|content| content.mime_type == "text/plain;charset=utf-8")
        );
    }

    #[test]
    fn iterm2_variables_and_direct_copy_use_terminal_state() {
        let state = osc_listener_state();
        *state.title.write() = "build tab".to_owned();
        state.report_working_directory("file://alice@remote.invalid/work/repo");
        state.handle_iterm2_command(
            &format!("SetUserVar=branch={}", BASE64.encode("feature/osc")),
            "\x1b\\",
        );

        assert_eq!(
            state.iterm2_variable("session.name").as_deref(),
            Some("build tab")
        );
        assert_eq!(
            state.iterm2_variable("session.path").as_deref(),
            Some("/work/repo")
        );
        assert_eq!(
            state.iterm2_variable("session.hostname").as_deref(),
            Some("remote.invalid")
        );
        assert_eq!(
            state.iterm2_variable("user.branch").as_deref(),
            Some("feature/osc")
        );
        assert_eq!(state.iterm2_variable("unknown"), None);

        state.handle_iterm2_command(&format!("Copy=:{}", BASE64.encode("copy me")), "\x1b\\");
        let update = state
            .osc_events
            .wait_after(0, Duration::ZERO)
            .expect("iTerm2 direct copy publishes a clipboard event");
        let TerminalOscEventPayload::ClipboardWrite { contents, .. } = &update.events[0].payload
        else {
            panic!("unexpected OSC event: {:?}", update.events[0].payload);
        };
        assert_eq!(contents[0].data, b"copy me");
    }

    #[test]
    fn kitty_file_transfer_preserves_safe_hierarchy_and_streams_zlib() {
        use flate2::{Compression, write::ZlibEncoder};

        let state = osc_listener_state();
        state.handle_kitty_file_transfer("ac=send;id=transfer-1", "\x1b\\");
        let update = state
            .osc_events
            .wait_after(0, Duration::ZERO)
            .expect("incoming transfer asks for authorization");
        let TerminalOscEventPayload::FileTransfer { offer } = &update.events[0].payload else {
            panic!("unexpected OSC event: {:?}", update.events[0].payload);
        };
        let destination = std::env::temp_dir().join(format!(
            "eggie-osc-file-transfer-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&destination).unwrap();
        state
            .complete_file_transfer(offer.request_id, Some(destination.clone()))
            .unwrap();

        let contents = b"streamed zlib file contents";
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(contents).unwrap();
        let compressed = encoder.finish().unwrap();
        let encoded_name = BASE64.encode("project/src/main.txt");
        state.handle_kitty_file_transfer(
            &format!(
                "ac=file;id=transfer-1;fid=file-1;ft=regular;n={encoded_name};zip=zlib;sz={}",
                contents.len()
            ),
            "\x1b\\",
        );
        state.handle_kitty_file_transfer(
            &format!(
                "ac=end_data;id=transfer-1;fid=file-1;d={}",
                BASE64.encode(compressed)
            ),
            "\x1b\\",
        );
        state.handle_kitty_file_transfer("ac=finish;id=transfer-1", "\x1b\\");

        assert_eq!(
            fs::read(destination.join("project/src/main.txt")).unwrap(),
            contents
        );
        assert_eq!(
            safe_kitty_transfer_path(Some(&BASE64.encode("../../escape")), "fallback"),
            PathBuf::from("fallback")
        );
        fs::remove_dir_all(destination).unwrap();
    }
}

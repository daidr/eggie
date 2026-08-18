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
use eggie_domain::{ProjectId, SessionId, WindowId};
#[cfg(test)]
use eggie_protocol::encode_line;
use eggie_protocol::{
    ClientRequest, DaemonResponse, PROTOCOL_VERSION, ProcessSummary,
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
    fs::{self, File, OpenOptions},
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

mod process_monitor;
use process_monitor::{descendant_processes, listening_ports};
#[cfg(test)]
use process_monitor::{cpu_usage_tenths_percent, filter_descendant_processes, parse_lsof_ports};
// Re-exported for the test module's `use super::*`; the daemon body no longer names these directly
// since the process-monitor helpers moved out.
#[cfg(test)]
use eggie_protocol::{ListeningPort, ProcessInfo};

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
/// Upper bound for a shared-memory image segment. Because shm pixels never cross the socket, this
/// must track the decoder's image limit (`kitty_graphics::MAX_IMAGE_BYTES`, 400 MiB =
/// 10000×10000×4), NOT the 64 MiB wire-frame limit — an image up to 4096×4096 is exactly 64 MiB, so
/// bounding shm by the wire limit would blank every larger image the terminal can legitimately
/// decode.
const MAX_SHM_IMAGE_BYTES: usize = 400 * 1024 * 1024;
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
/// A terminal-image frame whose payload is an shm segment *reference* (metadata + segment name)
/// rather than the pixels themselves. Mutually exclusive with [`RAW_TERMINAL_IMAGE_WIRE_FLAG`]:
/// payload length never exceeds `MAX_WIRE_MESSAGE_SIZE` (2^26), so bits 30 and 31 are always free.
const SHM_TERMINAL_IMAGE_WIRE_FLAG: u32 = 1 << 30;
/// Every flag bit in a wire header. Clearing these bits yields the payload length; testing them
/// classifies the frame. Kept as one mask so adding a flag can never leave a stale bit that
/// corrupts an extracted length.
const WIRE_HEADER_FLAG_MASK: u32 = RAW_TERMINAL_IMAGE_WIRE_FLAG | SHM_TERMINAL_IMAGE_WIRE_FLAG;
const RAW_TERMINAL_IMAGE_METADATA_SIZE: usize = 28;

/// How a peer should interpret a wire frame, decoded from its 4-byte length-prefix header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WireFrameKind {
    /// No flag bits set — a length-prefixed MessagePack message.
    Message,
    /// [`RAW_TERMINAL_IMAGE_WIRE_FLAG`] set — raw image pixels follow inline.
    RawImage,
    /// [`SHM_TERMINAL_IMAGE_WIRE_FLAG`] set — an shm segment reference follows (no inline pixels).
    ShmImage,
}

/// Split a wire header into its frame kind and payload length. The length is the header with all
/// flag bits cleared, so it stays correct no matter which (mutually exclusive) flag is set.
fn classify_wire_header(header: u32) -> (WireFrameKind, usize) {
    let length = (header & !WIRE_HEADER_FLAG_MASK) as usize;
    let kind = if header & RAW_TERMINAL_IMAGE_WIRE_FLAG != 0 {
        WireFrameKind::RawImage
    } else if header & SHM_TERMINAL_IMAGE_WIRE_FLAG != 0 {
        WireFrameKind::ShmImage
    } else {
        WireFrameKind::Message
    };
    (kind, length)
}

/// Pack the 28-byte image-chunk metadata header shared by the inline and shm wire framings. The
/// layout (all little-endian) is: id[0..4], generation[4..12], width[12..16], height[16..20],
/// total_length[20..24], offset[24..28]. `#[inline]` keeps this a zero-cost stack fill on the
/// image-publish hot path.
#[inline]
fn pack_image_metadata(
    key: TerminalImageKey,
    width: u32,
    height: u32,
    total_length: u32,
    offset: u32,
) -> [u8; RAW_TERMINAL_IMAGE_METADATA_SIZE] {
    let mut header = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    header[0..4].copy_from_slice(&key.id.to_le_bytes());
    header[4..12].copy_from_slice(&key.generation.to_le_bytes());
    header[12..16].copy_from_slice(&width.to_le_bytes());
    header[16..20].copy_from_slice(&height.to_le_bytes());
    header[20..24].copy_from_slice(&total_length.to_le_bytes());
    header[24..28].copy_from_slice(&offset.to_le_bytes());
    header
}

/// Unpack the 28-byte header written by [`pack_image_metadata`]. Returns `(key, width, height,
/// total_length, offset)`; `chunk_length` is framing-specific and stays with the caller.
#[inline]
fn unpack_image_metadata(
    header: &[u8; RAW_TERMINAL_IMAGE_METADATA_SIZE],
) -> (TerminalImageKey, u32, u32, u32, u32) {
    let read_u32 =
        |offset: usize| u32::from_le_bytes(header[offset..offset + 4].try_into().expect("four bytes"));
    let key = TerminalImageKey {
        id: read_u32(0),
        generation: u64::from_le_bytes(header[4..12].try_into().expect("eight bytes")),
    };
    (key, read_u32(12), read_u32(16), read_u32(20), read_u32(24))
}

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
                    self.begin_command(now, command_line_from_options(&prompt.options));
                }
                // Entering input directly (no preceding A) still means the cursor is on a prompt;
                // record the start if we don't have one yet.
                if self.prompt_start_line.is_none() {
                    self.prompt_start_line = Some(cursor_line);
                }
                self.phase = TerminalSemanticPhase::Input;
            }
            SemanticPromptAction::OutputStart => {
                let command_line = command_line_from_options(&prompt.options);
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

    /// The whole image's pixels, ignoring the chunk window. The shm transport hands the entire
    /// image to the consumer in one segment, so it does not use the `offset`/`end` slice.
    #[cfg(unix)]
    fn all_bytes(&self) -> &[u8] {
        &self.pixels[..]
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

/// A whole terminal image fetched over the wire, either inline pixels or a reference to a POSIX
/// shm segment the caller can map directly. Returned by [`DaemonConnection::fetch_terminal_image`]
/// so a zero-copy consumer (the UI) can `mmap` the segment instead of copying it into a `Vec`.
#[derive(Debug)]
pub enum TerminalImageFrame {
    /// Pixels delivered inline on the socket; already owned by the caller.
    Inline {
        metadata: TerminalImageChunkMetadata,
        pixels: Vec<u8>,
    },
    /// A reference to a daemon-created shm segment. The caller owns the unlink: it must
    /// `shm_open` + `shm_unlink` + `mmap` the name (see [`ShmImageSegment`] for the lifecycle).
    Shm {
        metadata: TerminalImageChunkMetadata,
        segment_name: Vec<u8>,
    },
    /// The image is larger than one inline wire chunk, so the whole-image fetch only saw its first
    /// chunk. The caller must fall back to [`DaemonConnection::append_terminal_image_chunk`] to walk
    /// the remaining chunks. A typed variant (rather than a stringly-matched error) makes the
    /// fallback contract explicit and compiler-checked.
    InlineTooLarge {
        metadata: TerminalImageChunkMetadata,
    },
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

/// Resolve a command line from an OSC 133 prompt's options, accepting three encodings in
/// priority order: `cmdline_b64` (base64, robust for CJK/newlines/control bytes and what the
/// zsh integration emits), `cmdline_url` (percent-encoded), then raw `cmdline`.
fn command_line_from_options(options: &str) -> Option<String> {
    option_value(options, "cmdline_b64")
        .and_then(|value| decode_base64_text(&value))
        .or_else(|| option_value(options, "cmdline_url").map(percent_decode))
        .or_else(|| option_value(options, "cmdline"))
        .filter(|line| !line.is_empty())
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
    /// Owning GUI window, or `None` when the session is detached (its window closed but the shell
    /// keeps running, so it survives as claimable). Mutated at runtime by `ClaimSession`/`DetachWindow`.
    window_id: Mutex<Option<WindowId>>,
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
    /// Live terminal-core config knobs that are rebuilt together on every `set_options`. Kept as the
    /// single source of truth so a runtime change to one field (cursor shape, scrollback depth) does
    /// not clobber the other when `terminal_config` reconstructs the whole `Config`.
    config_state: Mutex<TerminalConfigState>,
}

/// The subset of the terminal-core `Config` that Eggie mutates at runtime. `set_default_cursor_shape`
/// and `set_scrollback_limit` each update one field and rebuild via [`terminal_config`] from both, so
/// neither reset the other to the built-in default.
struct TerminalConfigState {
    cursor_shape: CursorShape,
    scrollback_limit: usize,
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

/// Everything the client picks per-session at creation time. Grouped into a struct so the growing
/// set of spawn-time knobs (scrollback depth, shell override, …) does not force every call site to
/// thread positional arguments.
struct SessionSpawnConfig {
    project_id: ProjectId,
    /// Window that will own the freshly spawned session.
    window_id: WindowId,
    cwd: PathBuf,
    size: TerminalSize,
    appearance: TerminalAppearance,
    /// Scrollback history depth in lines (`0` disables scrollback).
    scrollback_limit: usize,
    /// Shell executable override. `None`/empty falls back to `$SHELL` (then `/bin/zsh`).
    shell_program: Option<String>,
    /// Custom shell arguments. `None` uses Eggie's default launch args (`-l`, plus `--posix` for bash
    /// integration).
    shell_args: Option<Vec<String>>,
    /// Comma-separated `EGGIE_SHELL_FEATURES` tokens (e.g. `"path"`), injected verbatim into the
    /// shell environment. Empty = no features.
    shell_features: String,
}

impl TerminalSession {
    fn spawn(config: SessionSpawnConfig) -> Result<Self> {
        let SessionSpawnConfig {
            project_id,
            window_id,
            cwd,
            size,
            appearance,
            scrollback_limit,
            shell_program,
            shell_args,
            shell_features,
        } = config;
        let id = SessionId::new_v4();
        let last_input_sequence = Arc::new(AtomicU64::new(0));
        let state = Arc::new(ListenerState::new(
            id,
            size,
            appearance,
            last_input_sequence.clone(),
        ));
        let listener = DaemonEventListener(state.clone());
        let config = terminal_config(CursorShape::default(), scrollback_limit);
        let mut terminal = Term::new(config, &GridSize(size), listener.clone());
        terminal.set_kitty_graphics_cell_size(size.cell_width, size.cell_height);
        let terminal = Arc::new(FairMutex::new(terminal));
        // Resolve the shell: an explicit non-empty override wins, otherwise fall back to `$SHELL`
        // (then `/bin/zsh`). The basename still drives shell-integration matching either way.
        let shell = shell_program
            .filter(|program| !program.trim().is_empty())
            .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned()));
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
        // The daemon process *is* the eggie binary (launched with `--eggie-daemon`), so its own
        // executable directory is where `eggie` lives. Injected as EGGIE_BIN_DIR so the `path`
        // shell feature can add it to PATH. Non-fatal: if current_exe fails we simply don't inject.
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let launch = build_shell_env(
            &shell_name,
            &shell,
            &terminfo,
            integration_root.as_deref(),
            std::env::var("ZDOTDIR").ok(),
            std::env::var("ENV").ok(),
            shell_args,
            bin_dir.as_deref(),
            &shell_features,
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
            window_id: Mutex::new(Some(window_id)),
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
            config_state: Mutex::new(TerminalConfigState {
                cursor_shape: CursorShape::default(),
                scrollback_limit,
            }),
        })
    }

    /// Convenience constructor for tests: spawn with the built-in scrollback default and the
    /// environment's shell, so the many test call sites don't thread spawn-time config knobs.
    #[cfg(test)]
    fn spawn_default(
        project_id: ProjectId,
        cwd: PathBuf,
        size: TerminalSize,
        appearance: TerminalAppearance,
    ) -> Result<Self> {
        Self::spawn(SessionSpawnConfig {
            project_id,
            window_id: WindowId::new_v4(),
            cwd,
            size,
            appearance,
            scrollback_limit: TERMINAL_SCROLLBACK_LIMIT,
            shell_program: None,
            shell_args: None,
            shell_features: "path".to_owned(),
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
            window_id: *self.window_id.lock(),
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

    /// Scroll the viewport to an absolute scrollback offset (0 = live bottom, `history_size` = the
    /// oldest line). Drives the scrollbar thumb drag. The offset is clamped to the current history
    /// size, and a new frame is published only when the viewport actually moved (matching
    /// [`Self::scroll_to`]).
    fn scroll_to_offset(&self, offset: u32) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let history = terminal.grid().history_size();
        let target = (offset as usize).min(history);
        let initial = terminal.grid().display_offset();
        // `Scroll::Delta` is positive-up (toward older lines / higher offset), so the delta to reach
        // an absolute offset is simply `target - initial`.
        let delta = target as i32 - initial as i32;
        if delta != 0 {
            terminal.scroll_display(Scroll::Delta(delta));
            if terminal.grid().display_offset() != initial {
                self.events.publish_terminal(&terminal);
            }
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

    /// Copy a chunk of a published image into an owned `Vec`. The production transfer path uses the
    /// zero-copy [`image_chunk_ref`](Self::image_chunk_ref) instead; this allocating variant exists
    /// only for tests that want to assert on the pixel bytes directly.
    #[cfg(test)]
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
    /// replaces the whole config, reconstructed from the session's live [`TerminalConfigState`]), so a
    /// program that has issued DECSCUSR keeps its runtime override — matching Ghostty's semantics.
    /// Rebuilding from `config_state` also preserves any custom scrollback depth.
    fn set_default_cursor_shape(&self, shape: TerminalCursorShape) {
        let mut terminal = self.terminal.lock();
        let mut state = self.config_state.lock();
        state.cursor_shape = kernel_cursor_shape(shape);
        let config = terminal_config(state.cursor_shape, state.scrollback_limit);
        drop(state);
        terminal.set_options(config);
        self.events.publish_terminal(&terminal);
    }

    /// Set the session's scrollback history depth at runtime. Rebuilds the whole config from the
    /// live [`TerminalConfigState`] (preserving the current default cursor shape); the terminal core
    /// shrinks the buffer if the new depth is smaller than the current history.
    fn set_scrollback_limit(&self, limit: usize) {
        let mut terminal = self.terminal.lock();
        let mut state = self.config_state.lock();
        if state.scrollback_limit == limit {
            return;
        }
        state.scrollback_limit = limit;
        let config = terminal_config(state.cursor_shape, state.scrollback_limit);
        drop(state);
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
        display_offset: display_offset as u32,
        history_size: terminal.grid().history_size() as u32,
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
        display_offset: current.display_offset,
        history_size: current.history_size,
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

/// Build the terminal core `Config` for a session with the given default cursor shape and scrollback
/// depth. Kept in one place so session creation and runtime `set_options` stay in sync on every other
/// field. Both runtime knobs are passed explicitly so a change to one never resets the other.
fn terminal_config(default_cursor_shape: CursorShape, scrollback_limit: usize) -> Config {
    Config {
        scrolling_history: scrollback_limit,
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
                window_id,
                cwd,
                size,
                appearance,
                scrollback_limit,
                shell_program,
                shell_args,
                shell_features,
            } => {
                let session = Arc::new(TerminalSession::spawn(SessionSpawnConfig {
                    project_id,
                    window_id,
                    cwd,
                    size,
                    appearance,
                    scrollback_limit,
                    shell_program: (!shell_program.is_empty()).then_some(shell_program),
                    shell_args: (!shell_args.is_empty()).then_some(shell_args),
                    shell_features,
                })?);
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
            ClientRequest::TerminalImage { .. } => {
                // `serve_connection` intercepts `TerminalImage` before dispatching to `handle`,
                // serving it through the zero-copy `image_chunk_ref` + `send_terminal_image_chunk`
                // path. This arm is therefore never reached in production; keep it explicit so a
                // future caller that bypasses `serve_connection` fails loudly instead of silently
                // taking the allocating `image_chunk` path.
                unreachable!("TerminalImage is served by serve_connection's zero-copy path")
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
            ClientRequest::TerminalScrollToOffset { session_id, offset } => {
                self.session(session_id)?.scroll_to_offset(offset)?;
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
            ClientRequest::SetScrollbackLimit { session_id, limit } => {
                self.session(session_id)?.set_scrollback_limit(limit);
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::Terminate { session_id } => {
                let Some(session) = self.sessions.write().remove(&session_id) else {
                    bail!("unknown terminal session {session_id}");
                };
                session.terminate();
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::ClaimSession {
                session_id,
                window_id,
            } => {
                let session = self.session(session_id)?;
                *session.window_id.lock() = Some(window_id);
                Ok(DaemonResponse::SessionClaimed {
                    session: session.summary(),
                })
            }
            ClientRequest::DetachWindow { window_id } => {
                // Collect the matching ids under the read lock, then clear ownership one session at a
                // time. This avoids holding the sessions lock across the per-session `window_id` locks.
                let detached: Vec<Arc<TerminalSession>> = self
                    .sessions
                    .read()
                    .values()
                    .filter(|session| *session.window_id.lock() == Some(window_id))
                    .cloned()
                    .collect();
                for session in detached {
                    *session.window_id.lock() = None;
                }
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
///
/// `custom_args`, when `Some`, replaces Eggie's default `-l` login argument with the user's argv.
/// bash's `--posix` is mandatory for the ENV-based integration hook, so it is force-prepended even
/// over custom args (integration wins; the user's args are still honored after it). When
/// `custom_args` is `None` the behavior is byte-identical to before this knob existed.
// Every argument is an independent shell-launch input assembled into one env map; grouping them
// into a struct would just move the same fields elsewhere without improving clarity.
#[allow(clippy::too_many_arguments)]
fn build_shell_env(
    shell_name: &str,
    shell_path: &str,
    terminfo: &Path,
    integration_root: Option<&Path>,
    user_zdotdir: Option<String>,
    user_env_var: Option<String>,
    custom_args: Option<Vec<String>>,
    bin_dir: Option<&Path>,
    shell_features: &str,
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
    // Shell-feature plumbing, injected unconditionally for every shell (even those without an
    // integration script): EGGIE_BIN_DIR points at the eggie binary's directory, and
    // EGGIE_SHELL_FEATURES is the comma-separated token list the integration scripts read. This
    // way, once a shell's script learns to read them, the variables are already in place.
    if let Some(dir) = bin_dir {
        env.insert(
            "EGGIE_BIN_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        );
    }
    env.insert(
        "EGGIE_SHELL_FEATURES".to_owned(),
        shell_features.to_owned(),
    );

    let custom = custom_args.is_some();
    let mut args = custom_args.unwrap_or_else(|| vec!["-l".to_owned()]);

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
            // `--posix` is required for the ENV-based POSIX startup hook. Without custom args this is
            // the sole argument (preserving the original default); with custom args we force it to
            // the front so integration still works while keeping the user's args.
            if !custom {
                args = vec!["--posix".to_owned()];
            } else if !args.iter().any(|arg| arg == "--posix") {
                args.insert(0, "--posix".to_owned());
            }
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
                Ok(chunk) => send_terminal_image_chunk(stream.get_mut(), &chunk)?,
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
    let (kind, length) = classify_wire_header(header);
    if kind != WireFrameKind::Message {
        bail!("unexpected raw terminal image wire response");
    }
    read_message_body(reader, buffer, length).map(Some)
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

/// Send one terminal-image chunk response. On unix, the first chunk of an image (`offset == 0`) is
/// delivered zero-inline-copy via a shared-memory segment: the whole image is copied into an shm
/// object and only its name travels the socket. Non-zero offsets (a client re-requesting a tail
/// after a partial read) and non-unix platforms fall back to the inline wire frame. Because a shm
/// frame carries the entire image (`chunk_length == total_length`), the client's chunk loop
/// completes in one iteration and never asks for a non-zero offset in practice.
fn send_terminal_image_chunk(
    writer: &mut impl Write,
    chunk: &PublishedTerminalImageChunk,
) -> Result<()> {
    #[cfg(unix)]
    {
        if chunk.offset == 0 {
            let pixels = chunk.all_bytes();
            let mut segment = create_image_shm_segment(pixels)?;
            let metadata = TerminalImageChunkMetadata {
                key: chunk.key,
                width: chunk.width,
                height: chunk.height,
                total_length: chunk.total_length,
                offset: 0,
                chunk_length: chunk.total_length,
            };
            write_terminal_image_shm_wire(writer, &metadata, segment.name_bytes())?;
            // The name is on the wire; the consumer now owns the unlink.
            segment.disarm();
            return Ok(());
        }
    }
    write_terminal_image_wire(writer, chunk)
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
    let metadata = pack_image_metadata(
        chunk.key,
        chunk.width,
        chunk.height,
        chunk.total_length,
        chunk.offset,
    );
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
    let (key, width, height, total_length, offset) = unpack_image_metadata(&metadata);
    let chunk_length = payload_length - RAW_TERMINAL_IMAGE_METADATA_SIZE;
    let chunk_length = u32::try_from(chunk_length).context("terminal image chunk is too large")?;
    let original_length = destination.len();
    destination.resize(original_length + chunk_length as usize, 0);
    if let Err(error) = reader.read_exact(&mut destination[original_length..]) {
        destination.truncate(original_length);
        return Err(error.into());
    }
    Ok(TerminalImageChunkMetadata {
        key,
        width,
        height,
        total_length,
        offset,
        chunk_length,
    })
}

/// A daemon-created POSIX shared-memory segment holding image pixels for a single wire response.
///
/// Lifecycle (mirrors the consumer half in `kitty_graphics::read_shared_memory`, reversed): the
/// daemon creates the segment, copies pixels in, and closes its fd *without* unlinking. The
/// consumer (UI) opens the segment by name, unlinks it immediately, maps/copies the pixels, and
/// munmaps — the kernel keeps the object alive until both sides munmap. So on the success path the
/// daemon hands ownership of the unlink to the consumer via [`disarm`](Self::disarm). If the name
/// is never sent (an error before hand-off), `Drop` unlinks so the segment cannot leak. A consumer
/// crash after the name is sent but before it opens leaks one bounded segment until reboot — the
/// accepted cost of avoiding a synchronous ack.
///
/// A1 checkpoint: the daemon sends shm frames, but the UI still copies the mapped pixels into a
/// `Vec` before uploading — so the socket no longer carries pixels, but the total copy count is
/// unchanged until the UI's zero-copy `mmap`→Metal path (A2) lands.
#[cfg(unix)]
struct ShmImageSegment {
    name: std::ffi::CString,
    armed: bool,
}

#[cfg(unix)]
impl ShmImageSegment {
    fn name_bytes(&self) -> &[u8] {
        self.name.as_bytes()
    }

    /// Transfer unlink responsibility to the consumer. Call only after the name is on the wire.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(unix)]
impl Drop for ShmImageSegment {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                libc::shm_unlink(self.name.as_ptr());
            }
        }
    }
}

/// Create a uniquely-named POSIX shm segment, copy `pixels` in, and return an armed guard. The fd
/// is closed before returning; the segment stays linked until the consumer opens+unlinks it (or the
/// guard's `Drop` unlinks it on an error path). The name is unique per daemon process and segment.
#[cfg(unix)]
fn create_image_shm_segment(pixels: &[u8]) -> Result<ShmImageSegment> {
    use std::os::fd::FromRawFd;

    static SHM_SEGMENT_SERIAL: AtomicU64 = AtomicU64::new(0);
    let serial = SHM_SEGMENT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let name = std::ffi::CString::new(format!("/eggie-img-{}-{serial}", process::id()))
        .context("shm segment name contained a nul byte")?;
    let flags = libc::O_CREAT | libc::O_RDWR | libc::O_EXCL;
    let mut fd = unsafe { libc::shm_open(name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        // EEXIST means a prior daemon instance crashed (Drop never ran) and the OS reused its pid,
        // so this exact name is an orphaned, linked segment. It belongs to our pid namespace and no
        // live instance references it, so reclaim it and retry once rather than tearing down the
        // connection.
        if error.raw_os_error() == Some(libc::EEXIST) {
            unsafe {
                libc::shm_unlink(name.as_ptr());
            }
            fd = unsafe { libc::shm_open(name.as_ptr(), flags, 0o600) };
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("shm_open failed to create image segment");
        }
    }
    // Arm the guard as soon as the object exists so every later error path unlinks it.
    let guard = ShmImageSegment { name, armed: true };
    // RAII close of the fd; the segment outlives the fd once created.
    let file = unsafe { File::from_raw_fd(fd) };
    let len = pixels.len();
    if unsafe { libc::ftruncate(file.as_raw_fd(), len as libc::off_t) } != 0 {
        return Err(std::io::Error::last_os_error())
            .context("ftruncate failed to size image segment");
    }
    if len > 0 {
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error())
                .context("mmap failed to write image segment");
        }
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapping.cast::<u8>(), len);
            libc::munmap(mapping, len);
        }
    }
    Ok(guard)
}

/// Serialize an shm image frame: the 28-byte metadata header (identical layout to the inline raw
/// frame) followed by a length-prefixed segment name. The pixels themselves never touch the socket.
fn write_terminal_image_shm_wire(
    writer: &mut impl Write,
    metadata: &TerminalImageChunkMetadata,
    segment_name: &[u8],
) -> Result<()> {
    let payload_length = RAW_TERMINAL_IMAGE_METADATA_SIZE
        .checked_add(4)
        .and_then(|size| size.checked_add(segment_name.len()))
        .context("shm terminal image wire length overflow")?;
    if payload_length > MAX_WIRE_MESSAGE_SIZE {
        bail!("shm terminal image wire message exceeds {MAX_WIRE_MESSAGE_SIZE} bytes: {payload_length}");
    }
    let payload_length =
        u32::try_from(payload_length).context("shm terminal image name is too large")?;
    let name_length =
        u32::try_from(segment_name.len()).context("shm terminal image name is too large")?;
    let header = pack_image_metadata(
        metadata.key,
        metadata.width,
        metadata.height,
        metadata.total_length,
        metadata.offset,
    );
    writer.write_all(&(payload_length | SHM_TERMINAL_IMAGE_WIRE_FLAG).to_le_bytes())?;
    writer.write_all(&header)?;
    writer.write_all(&name_length.to_le_bytes())?;
    writer.write_all(segment_name)?;
    Ok(())
}

/// Read an shm image frame's header + segment name from the wire, without opening the segment.
/// Returns the frame metadata (with `chunk_length` left as `total_length`, since a shm frame always
/// carries the whole image) and the raw segment name. The consumer decides whether to copy the
/// segment into a `Vec` (daemon-side/fallback) or map it directly (the UI's zero-copy path).
fn read_terminal_image_shm_wire_header(
    reader: &mut impl Read,
    payload_length: usize,
) -> Result<(TerminalImageChunkMetadata, Vec<u8>)> {
    let minimum = RAW_TERMINAL_IMAGE_METADATA_SIZE + 4;
    if !(minimum..=MAX_WIRE_MESSAGE_SIZE).contains(&payload_length) {
        bail!("invalid shm terminal image wire length: {payload_length}");
    }
    let mut metadata = [0_u8; RAW_TERMINAL_IMAGE_METADATA_SIZE];
    reader.read_exact(&mut metadata)?;
    let (key, width, height, total_length, offset) = unpack_image_metadata(&metadata);
    let mut name_length = [0_u8; 4];
    reader.read_exact(&mut name_length)?;
    let name_length = u32::from_le_bytes(name_length) as usize;
    if minimum + name_length != payload_length {
        bail!("shm terminal image wire name length mismatch");
    }
    let mut name = vec![0_u8; name_length];
    reader.read_exact(&mut name)?;
    let metadata = TerminalImageChunkMetadata {
        key,
        width,
        height,
        total_length,
        offset,
        // A shm frame always carries the whole image, so the chunk spans the full length.
        chunk_length: total_length,
    };
    Ok((metadata, name))
}

/// Read an shm image frame, open the referenced segment, and copy its pixels into `destination`.
/// This is the functional (copy-into-Vec) decode used by the daemon-side read helpers and the
/// `DaemonConnection` fallback; the UI's zero-copy `mmap` path (A2) maps the segment directly.
fn read_terminal_image_shm_wire_into(
    reader: &mut impl Read,
    payload_length: usize,
    destination: &mut Vec<u8>,
) -> Result<TerminalImageChunkMetadata> {
    let (metadata, name) = read_terminal_image_shm_wire_header(reader, payload_length)?;
    let copied = read_image_shm_segment(&name, metadata.total_length as usize, destination)?;
    if copied != metadata.total_length as usize {
        bail!("shm terminal image segment shorter than declared length");
    }
    Ok(metadata)
}

/// Open the named POSIX shm segment read-only, unlink it (so it is reclaimed once munmapped), map
/// it, append exactly `total_length` bytes to `destination`, and munmap. `total_length` comes from
/// the wire metadata rather than the segment's `metadata().len()`, because POSIX shm objects are
/// rounded up to a page boundary (so the object is often larger than the pixel payload). Mirrors
/// `kitty_graphics::read_shared_memory`; returns the number of bytes appended.
#[cfg(unix)]
fn read_image_shm_segment(
    name: &[u8],
    total_length: usize,
    destination: &mut Vec<u8>,
) -> Result<usize> {
    use std::os::fd::FromRawFd;

    let name = std::ffi::CString::new(name).context("shm segment name contained a nul byte")?;
    let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("shm_open failed to read image segment");
    }
    // Unlink immediately: the kernel keeps the object alive until we munmap, and this guarantees the
    // segment is reclaimed even if the rest of this function fails.
    unsafe {
        libc::shm_unlink(name.as_ptr());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if total_length > MAX_SHM_IMAGE_BYTES {
        bail!("image segment exceeds {MAX_SHM_IMAGE_BYTES} bytes: {total_length}");
    }
    if total_length == 0 {
        return Ok(0);
    }
    let object_len = file
        .metadata()
        .context("failed to stat image segment")?
        .len();
    if (object_len as usize) < total_length {
        bail!("image segment holds {object_len} bytes, expected at least {total_length}");
    }
    let map_len = object_len as usize;
    let mapping = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            map_len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if mapping == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error()).context("mmap failed to read image segment");
    }
    let original_length = destination.len();
    destination.resize(original_length + total_length, 0);
    unsafe {
        std::ptr::copy_nonoverlapping(
            mapping.cast::<u8>(),
            destination[original_length..].as_mut_ptr(),
            total_length,
        );
        libc::munmap(mapping, map_len);
    }
    Ok(total_length)
}

#[cfg(not(unix))]
fn read_image_shm_segment(
    _name: &[u8],
    _total_length: usize,
    _destination: &mut Vec<u8>,
) -> Result<usize> {
    bail!("shared-memory image transport is only supported on unix")
}

fn read_daemon_response(
    reader: &mut impl Read,
    buffer: &mut Vec<u8>,
) -> Result<Option<DaemonResponse>> {
    let Some(header) = read_wire_header(reader)? else {
        return Ok(None);
    };
    let (kind, length) = classify_wire_header(header);
    let mut bytes = Vec::new();
    let metadata = match kind {
        WireFrameKind::Message => return read_message_body(reader, buffer, length).map(Some),
        WireFrameKind::RawImage => read_terminal_image_wire_into(reader, length, &mut bytes)?,
        WireFrameKind::ShmImage => read_terminal_image_shm_wire_into(reader, length, &mut bytes)?,
    };
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
        let (kind, length) = classify_wire_header(header);
        match kind {
            WireFrameKind::RawImage => {
                return read_terminal_image_wire_into(&mut self.stream, length, destination);
            }
            WireFrameKind::ShmImage => {
                return read_terminal_image_shm_wire_into(&mut self.stream, length, destination);
            }
            WireFrameKind::Message => {}
        }

        let response = read_message_body::<DaemonResponse>(
            &mut self.stream,
            &mut self.response,
            length,
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

    /// Fetch a whole terminal image, returning an shm segment reference when the daemon delivers
    /// one (so the caller can map it zero-copy) or inline pixels otherwise. Unlike
    /// [`append_terminal_image_chunk`](Self::append_terminal_image_chunk), the shm case does not
    /// open or copy the segment — it hands the caller the name and the responsibility to unlink it.
    pub fn fetch_terminal_image(
        &mut self,
        session_id: SessionId,
        key: TerminalImageKey,
        length: u32,
    ) -> Result<TerminalImageFrame> {
        write_wire_message(
            self.stream.get_mut(),
            &mut self.request,
            &ClientRequest::TerminalImage {
                session_id,
                key,
                offset: 0,
                length,
            },
        )?;
        let Some(header) = read_wire_header(&mut self.stream)? else {
            bail!("daemon closed the persistent connection");
        };
        let (kind, payload_length) = classify_wire_header(header);
        match kind {
            WireFrameKind::ShmImage => {
                let (metadata, segment_name) =
                    read_terminal_image_shm_wire_header(&mut self.stream, payload_length)?;
                Ok(TerminalImageFrame::Shm {
                    metadata,
                    segment_name,
                })
            }
            WireFrameKind::RawImage => {
                let mut pixels = Vec::new();
                let metadata = read_terminal_image_wire_into(
                    &mut self.stream,
                    payload_length,
                    &mut pixels,
                )?;
                if metadata.chunk_length != metadata.total_length {
                    // Only the first chunk arrived; the caller must walk the rest with
                    // append_terminal_image_chunk. Signal that with a typed variant, not an error.
                    return Ok(TerminalImageFrame::InlineTooLarge { metadata });
                }
                Ok(TerminalImageFrame::Inline { metadata, pixels })
            }
            WireFrameKind::Message => {
                let response = read_message_body::<DaemonResponse>(
                    &mut self.stream,
                    &mut self.response,
                    payload_length,
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
                        Ok(TerminalImageFrame::Inline {
                            metadata: TerminalImageChunkMetadata {
                                key,
                                width,
                                height,
                                total_length,
                                offset,
                                chunk_length,
                            },
                            pixels: bytes,
                        })
                    }
                    DaemonResponse::Error { message } => {
                        bail!("daemon request failed: {message}")
                    }
                    response => bail!("unexpected terminal image response: {response:?}"),
                }
            }
        }
    }
}

impl DaemonInputSender {
    /// Hand a queued input to the worker thread, mapping a closed channel to a uniform error. All
    /// the typed `send_*` methods funnel through here so the "worker stopped" context lives once.
    fn enqueue(&self, input: QueuedTerminalInput) -> Result<()> {
        self.sender
            .send(input)
            .context("terminal input worker stopped")
    }

    pub fn send_input(&self, session_id: SessionId, bytes: Vec<u8>, sequence: u64) -> Result<()> {
        self.enqueue(QueuedTerminalInput::Input {
            session_id,
            bytes,
            sequence,
        })
    }

    pub fn send_paste(&self, session_id: SessionId, text: String, sequence: u64) -> Result<()> {
        self.enqueue(QueuedTerminalInput::Paste {
            session_id,
            text,
            sequence,
        })
    }

    pub fn send_paste_clipboard(
        &self,
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
    ) -> Result<()> {
        self.enqueue(QueuedTerminalInput::PasteClipboard {
            session_id,
            selection,
            contents,
            sequence,
        })
    }

    pub fn send_mouse(&self, session_id: SessionId, event: TerminalMouseEvent) -> Result<()> {
        self.enqueue(QueuedTerminalInput::Mouse { session_id, event })
    }

    pub fn send_scroll(&self, session_id: SessionId, event: TerminalScrollEvent) -> Result<()> {
        self.enqueue(QueuedTerminalInput::Scroll { session_id, event })
    }

    pub fn send_focus(&self, session_id: SessionId, focused: bool) -> Result<()> {
        self.enqueue(QueuedTerminalInput::Focus {
            session_id,
            focused,
        })
    }

    pub fn send_selection_start(
        &self,
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
        kind: TerminalSelectionKind,
    ) -> Result<()> {
        self.enqueue(QueuedTerminalInput::SelectionStart {
            session_id,
            point,
            side,
            kind,
        })
    }

    pub fn send_selection_update(
        &self,
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    ) -> Result<()> {
        self.enqueue(QueuedTerminalInput::SelectionUpdate {
            session_id,
            point,
            side,
        })
    }

    pub fn send_selection_clear(&self, session_id: SessionId) -> Result<()> {
        self.enqueue(QueuedTerminalInput::SelectionClear { session_id })
    }
}

/// Terminate the daemon listening on `socket_path` (SIGTERM via the socket
/// peer PID) and remove the socket file. Used by the app on shutdown paths
/// and by the standalone updater when an update changes the daemon protocol.
pub fn terminate_daemon_at(socket_path: &Path) {
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

/// Whether a daemon's handshake response is acceptable to this client.
///
/// The protocol version must always match. In debug builds the build id must
/// also match, so any recompile swaps in a fresh daemon. In release builds
/// only the protocol version matters, allowing an in-place update to keep a
/// compatible daemon running (terminal sessions stay alive across updates).
pub fn handshake_accepted(
    daemon_protocol: u32,
    daemon_build_id: &str,
    self_protocol: u32,
    self_build_id: &str,
) -> bool {
    if daemon_protocol != self_protocol {
        return false;
    }
    if cfg!(debug_assertions) && daemon_build_id != self_build_id {
        return false;
    }
    true
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
            } => {
                if !handshake_accepted(
                    protocol_version,
                    &build_id,
                    PROTOCOL_VERSION,
                    &self.build_id,
                ) {
                    bail!("unexpected handshake response: protocol={protocol_version}, build_id={build_id}");
                }
                if build_id != self.build_id.as_ref() {
                    // Release builds accept a daemon from a different build as long as the
                    // protocol version matches (in-place updates keep the daemon alive).
                    eprintln!(
                        "connected to daemon with a different build id: daemon {build_id}, app {}",
                        self.build_id
                    );
                }
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
mod tests;

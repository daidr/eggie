mod osc_events;
use osc_events::*;
mod completion;
mod shell_integration;
use shell_integration::*;
mod progress;
use progress::*;
mod clipboard;
mod file_transfers;
mod notifications;
mod osc;
use osc::*;
mod urls;
use urls::*;
mod session;
use session::*;
mod paste;
mod snapshot;
mod ssh;
use snapshot::*;
mod command_output;
mod search;
use search::*;
mod shell;
use shell::*;
mod server;
use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    event_loop::{EventLoop, EventLoopSender, Msg},
    grid::{Dimensions, Scroll},
    index::{Boundary, Column, Direction, Line, Point, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        ClipboardType, Config, Osc52, Term, TermDamage, TermMode,
        cell::{Cell, Flags},
        color::COUNT,
        kitty_graphics::{ImageKey as KittyImageKey, PixelBuffer},
        point_to_viewport,
        search::{RegexIter, RegexSearch},
    },
    tty,
    vte::ansi::{
        Color, CursorShape, CursorStyle, NamedColor, ProgressState as VteProgressState,
        SemanticPrompt, SemanticPromptAction,
    },
};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::read::DecoderReader;
use eggie_domain::{ProjectId, RemoteTarget, SessionId};
#[cfg(test)]
use eggie_ipc::{DaemonConnection, daemon_socket_path};
use eggie_ipc::{configure_latency_sensitive_thread, wire::*};
#[cfg(test)]
use eggie_protocol::encode_line;
use eggie_protocol::{
    ClientRequest, DaemonResponse, PROTOCOL_VERSION, PortForwardSpec, ProcessSummary,
    SessionInspection, SessionStatus, SessionSummary, TERMINAL_SCROLL_DELTA_SCALE,
    TerminalAppearance, TerminalAttentionRequest, TerminalCell, TerminalCellPosition,
    TerminalCells, TerminalClipboardContent, TerminalClipboardSelection, TerminalColorOverride,
    TerminalCommandRecord, TerminalCursorShape, TerminalFileTransferOffer,
    TerminalFileTransferProtocol, TerminalImageDescriptor, TerminalImageKey,
    TerminalImagePlacement, TerminalLinkRange, TerminalMouseAction, TerminalMouseEvent,
    TerminalMousePosition, TerminalNotification, TerminalOscEvent, TerminalOscEventPayload,
    TerminalOscEventUpdate, TerminalProgress, TerminalProgressState, TerminalProgressTimeouts,
    TerminalProgressUpdate, TerminalReportedLocation, TerminalRow, TerminalScrollCommand,
    TerminalScrollEvent, TerminalScrollPhase, TerminalScrollUnit, TerminalSearchDirection,
    TerminalSearchMatch, TerminalSearchRequest, TerminalSearchResult, TerminalSelectionKind,
    TerminalSelectionRange, TerminalSelectionSide, TerminalSemanticPhase,
    TerminalShellIntegrationState, TerminalSize, TerminalSnapshot, TerminalSnapshotDelta,
    TerminalUserVariable,
};
use flate2::write::ZlibDecoder;
use parking_lot::{Condvar, Mutex, RwLock};
#[cfg(test)]
use server::DaemonState;
pub use server::run_daemon;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{self, OpenOptions},
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufReader, Read, Write},
    ops::RangeInclusive,
    os::{
        fd::AsRawFd,
        unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    process,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

mod config_service;
mod event_bus;
mod preferences_service;
mod process_monitor;
#[cfg(test)]
use process_monitor::listening_ports;
#[cfg(test)]
use process_monitor::{cpu_usage_tenths_percent, filter_descendant_processes, parse_lsof_ports};
use process_monitor::{local_inspection, parse_remote_inspection};
// Re-exported for the test module's `use super::*`; the daemon body no longer names these directly
// since the process-monitor helpers moved out.
#[cfg(test)]
use eggie_protocol::{ListeningPort, ProcessInfo};

mod osc_util;
use osc_util::*;
mod program_status;
use program_status::{ProgramStatusCommand, ProgramStatusTracker};

mod remote_bootstrap;
use remote_bootstrap::{RemoteScripts, control_path, remote_launch};

mod terminal_input_encode;
use terminal_input_encode::*;
// Re-exported for the test module's `use super::*`; production code no longer names these directly
// since the input-encoding helpers moved out.
#[cfg(test)]
use eggie_protocol::{
    TerminalInputModes, TerminalModifiers, TerminalMouseButton, TerminalMouseEncoding,
    TerminalMouseTracking,
};

const BUNDLED_ALACRITTY_TERMINFO: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/alacritty.terminfo"));
static INSTALLED_TERMINFO: OnceLock<std::result::Result<PathBuf, String>> = OnceLock::new();
const BUNDLED_ZSH_ZSHENV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eggie-zsh-zshenv"));
const BUNDLED_ZSH_INTEGRATION: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/eggie-zsh-integration"));
const BUNDLED_BASH_INTEGRATION: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eggie-bash"));
const BUNDLED_FISH_INTEGRATION: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/eggie-fish"));

/// Version marker written to the remote host. Derived from a hash of the bundled script **bytes**,
/// not the crate version: the idempotence check compares this against the marker already on the
/// host, and an unchanged crate version would make every post-release script fix (e.g. the OSC 7
/// directory reports) look "already installed" and never get pushed. Hashing the contents means a
/// changed script changes the marker and triggers a reinstall, while identical scripts stay a
/// no-op. [`DefaultHasher`] uses a fixed key, so the value is deterministic across builds and hosts.
fn integration_version() -> String {
    fn hash_bytes(hasher: &mut DefaultHasher, bytes: &[u8]) {
        bytes.hash(hasher);
    }
    let mut hasher = DefaultHasher::new();
    hash_bytes(&mut hasher, BUNDLED_ZSH_ZSHENV);
    hash_bytes(&mut hasher, BUNDLED_ZSH_INTEGRATION);
    hash_bytes(&mut hasher, BUNDLED_BASH_INTEGRATION);
    hash_bytes(&mut hasher, BUNDLED_FISH_INTEGRATION);
    format!("{}-{:016x}", env!("CARGO_PKG_VERSION"), hasher.finish())
}

#[cfg(test)]
mod integration_version_tests {
    use super::integration_version;

    #[test]
    fn marker_is_stable_and_versioned_but_distinct_from_the_bare_crate_version() {
        let marker = integration_version();
        // Deterministic across calls (DefaultHasher has a fixed key), so the idempotence check is
        // stable across builds/hosts.
        assert_eq!(marker, integration_version());
        // It carries the crate version for human identification…
        assert!(
            marker.starts_with(concat!(env!("CARGO_PKG_VERSION"), "-")),
            "marker should be `<crate-version>-<hash>`, got {marker}"
        );
        // …but is NOT equal to it — otherwise post-release script fixes would look "already installed".
        assert_ne!(marker, env!("CARGO_PKG_VERSION"));
    }
}

static INSTALLED_SHELL_INTEGRATION: OnceLock<std::result::Result<PathBuf, String>> =
    OnceLock::new();
static RENDER_METRICS_ENABLED: OnceLock<bool> = OnceLock::new();
/// Regex for auto-detecting bare URLs. regex-automata (the terminal core's engine) has no
/// look-around, so the scheme is anchored on `://` (or the literal `mailto:`) and the body is a
/// greedy run of non-whitespace, non-control, non-delimiter characters. Trailing punctuation and
/// unbalanced brackets are stripped in `refine_url_range` afterwards.
const URL_PATTERN: &str =
    r#"(?i)((https?|ftps?|ssh|git)://|mailto:)[^ \t\x00-\x1f\x7f<>"{}|\\^`\[\]]+"#;
const PROCESS_METADATA_REFRESH_INTERVAL: Duration = Duration::from_millis(200);
const MAX_SNAPSHOT_WAIT: Duration = Duration::from_secs(1);
const TERMINAL_FRAME_INTERVAL: Duration = Duration::from_millis(8);
const PROGRESS_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
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

#[cfg(test)]
mod resize_tests;
#[cfg(test)]
mod tests;

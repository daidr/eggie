use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientRequest {
    Handshake {
        protocol_version: u32,
    },
    GetConfig,
    GetPreferences,
    GetEventCursor,
    SetPreferences {
        base_revision: u64,
        value: serde_json::Value,
    },
    ReloadConfig,
    WaitForConfig {
        after_revision: u64,
        timeout_ms: u32,
    },
    WaitForEvents {
        after_sequence: u64,
        session_id: Option<SessionId>,
        timeout_ms: u32,
    },
    InvokeConfigAction {
        revision: u64,
        name: String,
        context: eggie_config::ActionContext,
        args: serde_json::Value,
    },
    CreateSession {
        project_id: ProjectId,
        cwd: PathBuf,
        size: TerminalSize,
        appearance: TerminalAppearance,
        /// Scrollback history depth in lines (`0` disables scrollback). The client always sends its
        /// resolved value; `#[serde(default)]` only guards deserializing a request that predates the
        /// field, which the version handshake already rules out in practice.
        #[serde(default)]
        scrollback_limit: usize,
        /// Shell executable to launch. Empty = the daemon falls back to `$SHELL` (then `/bin/zsh`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        shell_program: String,
        /// Custom arguments for the shell. Empty = Eggie's default launch args (`-l`, plus the
        /// mandatory `--posix` for bash integration).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        shell_args: Vec<String>,
        /// Comma-separated `EGGIE_SHELL_FEATURES` tokens (e.g. `"path"`). The daemon injects this
        /// verbatim into the shell environment; shell-integration scripts interpret the tokens they
        /// know and ignore the rest. Empty = no features. The client always sends its resolved value.
        #[serde(default = "default_shell_features")]
        shell_features: String,
        /// Extra environment variables for the shell, from the terminal's profile. Applied *before*
        /// Eggie's own integration variables (`TERM`, `ZDOTDIR`, `EGGIE_*`, …), which therefore win —
        /// a profile cannot break shell integration by naming one of them. A `BTreeMap` so the wire
        /// encoding is deterministic.
        ///
        /// Additive, like the shell fields above, so [`PROTOCOL_VERSION`] does not change: a release
        /// daemon predating profiles ignores the field (unknown fields are not denied) and spawns the
        /// shell without the profile's variables. The user-visible effect is a profile's `env` not
        /// applying until the daemon is restarted onto the new build — the session still starts, and a
        /// debug build swaps the daemon on every recompile via the build-id check.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        env: std::collections::BTreeMap<String, String>,
        /// When set, the session is an `ssh` connection to this host rather than a local shell, and
        /// the daemon may install its shell integration there (see the `remote_bootstrap` module).
        /// `None` = a local session. A profile can still run `ssh` as a plain `shell_program`; it just
        /// gets no automatic remote integration, because the host is not known before connecting.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote: Option<eggie_domain::RemoteTarget>,
        /// Whether the user has consented to Eggie writing its integration into this host's home
        /// directory. Ignored unless `remote` is set. `false` opens the session without installing
        /// anything — usable, just without integration.
        #[serde(default, skip_serializing_if = "is_false")]
        remote_install_consented: bool,
        /// Working directory on the *remote* host, from the profile. Distinct from `cwd`, which is a
        /// local path the `ssh` process itself starts in; a remote path is meaningless locally and
        /// vice versa. Ignored unless `remote` is set. `None` = the remote home directory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remote_cwd: Option<String>,
    },
    ListSessions,
    InspectSession {
        session_id: SessionId,
    },
    Snapshot {
        session_id: SessionId,
    },
    WaitForSnapshot {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    /// Rendering subscribers receive leased immutable rows; ordinary snapshots remain value data.
    WaitForRenderFrame {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    WaitForProgress {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    WaitForOscEvents {
        session_id: SessionId,
        after_revision: u64,
        timeout_ms: u32,
    },
    TerminalCompletions {
        session_id: SessionId,
        input: PromptInput,
        /// Enqueue a native query; candidates arrive via the input revision and a non-native fetch.
        native: bool,
    },
    GetShellIntegrationState {
        session_id: SessionId,
    },
    CompleteClipboardRead {
        session_id: SessionId,
        request_id: u64,
        contents: Vec<TerminalClipboardContent>,
    },
    NotificationResponse {
        session_id: SessionId,
        notification_id: String,
        activated: bool,
    },
    CompleteFileTransfer {
        session_id: SessionId,
        request_id: u64,
        destination: Option<PathBuf>,
    },
    TerminalImage {
        session_id: SessionId,
        key: TerminalImageKey,
        offset: u32,
        length: u32,
    },
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
    PasteText {
        session_id: SessionId,
        text: String,
        sequence: u64,
        /// A preview is tied to the application that was active when it opened.
        expected_context: Option<TerminalPasteContext>,
    },
    PasteStatus {
        session_id: SessionId,
        id: u64,
    },
    CancelPaste {
        session_id: SessionId,
        id: u64,
    },
    PasteClipboard {
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
        expected_context: Option<TerminalPasteContext>,
    },
    Mouse {
        session_id: SessionId,
        event: TerminalMouseEvent,
    },
    Scroll {
        session_id: SessionId,
        event: TerminalScrollEvent,
    },
    /// Scroll the viewport by a discrete command (page up/down, jump to top/bottom). Unlike
    /// [`ClientRequest::Scroll`] this maps directly onto the terminal core's `Scroll` primitives,
    /// giving exact page and boundary semantics without the wheel accumulator.
    TerminalScrollTo {
        session_id: SessionId,
        command: TerminalScrollCommand,
    },
    /// Scroll the viewport to an absolute scrollback offset (0 = live bottom, `history_size` = top).
    /// The daemon clamps the offset to the current history size. Used by the scrollbar thumb drag,
    /// which needs precise absolute positioning rather than the wheel accumulator's relative deltas.
    TerminalScrollToOffset {
        session_id: SessionId,
        offset: u32,
    },
    /// Begin an interactive selection at a viewport cell. The daemon converts the viewport point to
    /// an absolute grid point (accounting for the current scroll offset) and stores it as the
    /// terminal core's authoritative `Selection`, which stays valid across scrollback growth.
    TerminalSelectionStart {
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
        kind: TerminalSelectionKind,
    },
    /// Extend the active selection's head to a viewport cell (drag). Consecutive updates are merged
    /// on the daemon's input queue so a fast drag does not flood the terminal core.
    TerminalSelectionUpdate {
        session_id: SessionId,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    },
    /// Clear the active selection (e.g. a click that did not turn into a drag).
    TerminalSelectionClear {
        session_id: SessionId,
    },
    /// Select the entire buffer including scrollback.
    TerminalSelectAll {
        session_id: SessionId,
    },
    /// Extract the current selection's text (whole scrollback span, soft-wrap unwrapped, trailing
    /// whitespace trimmed) and return it in [`DaemonResponse::SelectionText`].
    TerminalCopySelection {
        session_id: SessionId,
    },
    /// Search the terminal grid (including scrollback) for `query`. The daemon runs the search in
    /// its terminal core, scrolls the viewport so the active match is visible, publishes a fresh
    /// snapshot, and replies with [`DaemonResponse::SearchResult`]. Coordinates in the reply are
    /// viewport-relative, matching the snapshot the client renders against.
    TerminalSearch {
        session_id: SessionId,
        request: TerminalSearchRequest,
    },
    Focus {
        session_id: SessionId,
        focused: bool,
    },
    Resize {
        session_id: SessionId,
        size: TerminalSize,
    },
    SetAppearance {
        session_id: SessionId,
        appearance: TerminalAppearance,
    },
    SetProgressTimeouts {
        session_id: SessionId,
        timeouts: TerminalProgressTimeouts,
    },
    SetOscPolicy {
        session_id: SessionId,
        allow_clipboard_read: bool,
    },
    /// Enable or disable bare-URL auto-detection for a session.
    SetUrlDetection {
        session_id: SessionId,
        detect_urls: bool,
    },
    /// Set the session's default cursor shape. A running program can still override this at runtime
    /// via DECSCUSR (`CSI Ps SP q`); this only changes the shape used when the program hasn't asked.
    SetCursorStyle {
        session_id: SessionId,
        shape: TerminalCursorShape,
    },
    /// Set the session's scrollback history depth at runtime (`0` disables scrollback). Shrinking
    /// below the current history evicts the oldest lines; the change applies to the live terminal.
    SetScrollbackLimit {
        session_id: SessionId,
        limit: usize,
    },
    Terminate {
        session_id: SessionId,
    },
    /// Open a port forward on a session's ssh master. The daemon replies with [`DaemonResponse::Ok`].
    ForwardPort {
        session_id: SessionId,
        spec: PortForwardSpec,
    },
    /// Cancel a port forward on a session's ssh master. The daemon replies with [`DaemonResponse::Ok`].
    CancelPort {
        session_id: SessionId,
        spec: PortForwardSpec,
    },
}

/// A port forward to open or cancel on a session's ssh master.
///
/// The bind address defaults to 127.0.0.1, so a tunnel is not exposed to the local network unless
/// the user explicitly asks for it. The destination is required for Local and Remote forwards (the
/// address and port to connect to on the other side) and ignored for Dynamic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForwardSpec {
    pub kind: PortForwardKind,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortForwardKind {
    /// Local forward: connections to the bound port are forwarded to the destination through the
    /// remote host (`ssh -L`).
    Local,
    /// Remote forward: connections to the bound port on the remote host are forwarded to the
    /// destination through the local machine (`ssh -R`).
    Remote,
    /// Dynamic forward: the bound port becomes a SOCKS proxy (`ssh -D`).
    Dynamic,
}

fn default_bind_address() -> String {
    "127.0.0.1".to_owned()
}

impl PortForwardSpec {
    /// The ssh tunnel option for this spec: `-L`, `-R`, or `-D` followed by the bind and destination.
    ///
    /// The bind address is included so a tunnel on a non-loopback address is explicit. The
    /// destination is required for Local and Remote forwards and ignored for Dynamic.
    pub fn tunnel_option(&self) -> String {
        let flag = match self.kind {
            PortForwardKind::Local => "-L",
            PortForwardKind::Remote => "-R",
            PortForwardKind::Dynamic => "-D",
        };
        match self.kind {
            PortForwardKind::Dynamic => format!("{flag}{}:{}", self.bind_address, self.port),
            _ => format!(
                "{flag}{}:{}:{}",
                self.bind_address,
                self.port,
                self.destination.as_deref().unwrap_or_default()
            ),
        }
    }
}

/// Default `CreateSession::shell_features` when deserializing a request that predates the field.
/// Matches the client's default (the `path` feature on), so an old client still gets PATH support.
fn default_shell_features() -> String {
    "path".to_owned()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    HandshakeAccepted {
        protocol_version: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        build_id: String,
    },
    Config {
        snapshot: eggie_config::ConfigSnapshot,
    },
    Preferences {
        snapshot: eggie_config::PreferencesSnapshot,
    },
    ConfigUnchanged {
        revision: u64,
    },
    ConfigActionResult {
        actions: Vec<eggie_config::QueuedAction>,
    },
    Events {
        update: EventUpdate,
    },
    EventsUnchanged {
        sequence: u64,
    },
    SessionCreated {
        session: SessionSummary,
    },
    Sessions {
        sessions: Vec<SessionSummary>,
    },
    PasteStatus {
        status: TerminalPasteStatus,
    },
    SessionInspection {
        inspection: SessionInspection,
    },
    Snapshot {
        snapshot: Arc<TerminalSnapshot>,
    },
    SnapshotDelta {
        delta: TerminalSnapshotDelta,
    },
    SnapshotUnchanged {
        revision: u64,
    },
    Progress {
        update: TerminalProgressUpdate,
    },
    ProgressUnchanged {
        session_id: SessionId,
        revision: u64,
    },
    OscEvents {
        update: TerminalOscEventUpdate,
    },
    OscEventsUnchanged {
        session_id: SessionId,
        revision: u64,
    },
    TerminalCompletions {
        session_id: SessionId,
        input: PromptInput,
        items: Vec<CompletionItem>,
    },
    ShellIntegrationState {
        session_id: SessionId,
        state: TerminalShellIntegrationState,
    },
    SearchResult {
        session_id: SessionId,
        result: TerminalSearchResult,
    },
    /// Reply to [`ClientRequest::TerminalCopySelection`]. `text` is `None` when there is no active
    /// selection or it is empty.
    SelectionText {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    TerminalImage {
        key: TerminalImageKey,
        width: u32,
        height: u32,
        total_length: u32,
        offset: u32,
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    Ok,
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub name: String,
    pub session_id: Option<SessionId>,
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventUpdate {
    pub sequence: u64,
    pub gap: bool,
    pub events: Vec<EventRecord>,
}

use crate::{TerminalImageChunkMetadata, TerminalImageFrame, wire::*};
use anyhow::{Context, Result, bail};
use eggie_domain::SessionId;
use eggie_protocol::*;
use std::{
    fs,
    io::BufReader,
    os::fd::AsRawFd,
    os::unix::{net::UnixStream, process::CommandExt},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    sync::{Arc, OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};
const DAEMON_ARGUMENT: &str = "--eggie-daemon";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_INPUT_BATCH_MESSAGES: usize = 8;
static RENDER_METRICS_ENABLED: OnceLock<bool> = OnceLock::new();

pub(crate) fn render_metrics_enabled() -> bool {
    *RENDER_METRICS_ENABLED.get_or_init(|| {
        std::env::var_os("EGGIE_RENDER_METRICS")
            .is_some_and(|value| !value.is_empty() && value != "0")
    })
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

#[cfg(target_os = "macos")]
pub fn configure_latency_sensitive_thread() {
    unsafe {
        // QOS_CLASS_USER_INTERACTIVE is stable in Darwin's pthread ABI but is not exported by
        // every libc crate release supported by this workspace.
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn configure_latency_sensitive_thread() {}

#[derive(Clone)]
pub struct DaemonClient {
    socket_path: Arc<PathBuf>,
    build_id: Arc<str>,
}

pub struct DaemonConnection {
    stream: BufReader<UnixStream>,
    request: Vec<u8>,
    response: Vec<u8>,
    #[cfg(unix)]
    grid: crate::GridReader,
}

#[derive(Clone)]
pub struct DaemonInputSender {
    sender: mpsc::Sender<QueuedTerminalInput>,
}

#[derive(Debug)]
enum QueuedTerminalInput {
    PasteText {
        session_id: SessionId,
        text: String,
        sequence: u64,
        expected_context: Option<TerminalPasteContext>,
        reply: mpsc::Sender<std::result::Result<TerminalPasteStatus, String>>,
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
            Self::PasteText { .. } => unreachable!("checked paste has a reply channel"),
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
                expected_context,
            } => ClientRequest::PasteClipboard {
                session_id,
                selection,
                contents,
                sequence,
                expected_context,
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
    /// Wrap a connected stream, preserving the caller's socket configuration.
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream: BufReader::new(stream),
            request: Vec::with_capacity(512),
            response: Vec::with_capacity(8 * 1024),
            #[cfg(unix)]
            grid: crate::GridReader::default(),
        }
    }

    /// Encoded response size for transport diagnostics.
    pub fn last_response_size(&self) -> usize {
        self.response.len()
    }

    pub fn request(&mut self, request: ClientRequest) -> Result<DaemonResponse> {
        write_wire_message(self.stream.get_mut(), &mut self.request, &request)?;
        #[cfg(unix)]
        let response =
            read_daemon_response_with_grid(&mut self.stream, &mut self.response, &mut self.grid)?;
        #[cfg(not(unix))]
        let response = read_daemon_response(&mut self.stream, &mut self.response)?;
        let Some(response) = response else {
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
            WireFrameKind::ShmGrid => bail!("unexpected shared grid image response"),
            WireFrameKind::Message => {}
        }

        let response =
            read_message_body::<DaemonResponse>(&mut self.stream, &mut self.response, length)?;
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
                let metadata =
                    read_terminal_image_wire_into(&mut self.stream, payload_length, &mut pixels)?;
                if metadata.chunk_length != metadata.total_length {
                    // Only the first chunk arrived; the caller must walk the rest with
                    // append_terminal_image_chunk. Signal that with a typed variant, not an error.
                    return Ok(TerminalImageFrame::InlineTooLarge { metadata });
                }
                Ok(TerminalImageFrame::Inline { metadata, pixels })
            }
            WireFrameKind::ShmGrid => bail!("unexpected shared grid image response"),
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
    /// Paste and subsequent keystrokes share the same FIFO, including the start acknowledgement.
    pub fn send_paste_text(
        &self,
        session_id: SessionId,
        text: String,
        sequence: u64,
        expected_context: Option<TerminalPasteContext>,
    ) -> Result<mpsc::Receiver<std::result::Result<TerminalPasteStatus, String>>> {
        let (reply, receiver) = mpsc::channel();
        self.enqueue(QueuedTerminalInput::PasteText {
            session_id,
            text,
            sequence,
            expected_context,
            reply,
        })?;
        Ok(receiver)
    }
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
        expected_context: Option<TerminalPasteContext>,
    ) -> Result<()> {
        self.enqueue(QueuedTerminalInput::PasteClipboard {
            session_id,
            selection,
            contents,
            sequence,
            expected_context,
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

/// The versioned protocol is the compatibility contract; build identity is diagnostic.
pub fn handshake_accepted(
    daemon_protocol_version: u32,
    _daemon_build_id: &str,
    self_protocol_version: u32,
    _self_build_id: &str,
) -> bool {
    // Process lifetime is independent of GUI builds; protocol changes are the compatibility gate.
    daemon_protocol_version == self_protocol_version
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
                    bail!(
                        "unexpected handshake response: protocol={protocol_version}, build_id={build_id}"
                    );
                }
                if build_id != self.build_id.as_ref() {
                    // GUI rebuilds preserve a compatible daemon and its running processes.
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
        Ok(DaemonConnection::from_stream(stream))
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
                        if let QueuedTerminalInput::PasteText {
                            session_id,
                            text,
                            sequence,
                            expected_context,
                            reply,
                        } = input
                        {
                            let large = text.len() >= LARGE_PASTE_BYTES;
                            let result = (|| {
                                if connection.is_none() {
                                    connection = Some(client.connect()?);
                                }
                                match connection.as_mut().unwrap().request(
                                    ClientRequest::PasteText {
                                        session_id,
                                        text,
                                        sequence,
                                        expected_context,
                                    },
                                )? {
                                    DaemonResponse::PasteStatus { status } => Ok(status),
                                    response => bail!("unexpected paste response: {response:?}"),
                                }
                            })();
                            // Do not retain the large serialization buffers on the long-lived input stream.
                            if result.is_err() || large {
                                connection = None;
                            }
                            let _ = reply
                                .send(result.map_err(|error: anyhow::Error| format!("{error:#}")));
                            continue;
                        }
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
    #[test]
    fn default_client_removes_only_obsolete_protocol_sockets() {
        let directory = std::env::temp_dir().join(format!(
            "eggie-daemon-cleanup-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let current = directory.join(format!("daemon-v{PROTOCOL_VERSION}.sock"));
        let obsolete = directory.join("daemon-v999.sock");
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
    fn handshake_acceptance_rules() {
        // Protocol mismatch is always rejected.
        assert!(!handshake_accepted(2, "any", 1, "any"));

        // Same protocol, same build id is always accepted.
        assert!(handshake_accepted(1, "build-a", 1, "build-a"));

        // A GUI rebuild must not end compatible daemon processes.
        assert_eq!(handshake_accepted(1, "build-a", 1, "build-b"), true);
    }
}

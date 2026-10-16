use super::*;
use std::os::unix::fs::OpenOptionsExt;

pub(super) struct DaemonState {
    pub(super) sessions: RwLock<HashMap<SessionId, Arc<TerminalSession>>>,
    pub(super) build_id: Arc<str>,
    pub(super) config: config_service::ConfigService,
    pub(super) events: Arc<event_bus::EventBus>,
    pub(super) preferences: preferences_service::PreferencesService,
    pub(super) ssh_connections: Arc<ssh::SshConnections>,
}

impl DaemonState {
    pub(super) fn handle(&self, request: ClientRequest) -> DaemonResponse {
        match self.try_handle(request) {
            Ok(response) => response,
            Err(error) => DaemonResponse::Error {
                message: format!("{error:#}"),
            },
        }
    }

    pub(super) fn try_handle(&self, request: ClientRequest) -> Result<DaemonResponse> {
        match request {
            ClientRequest::ListSshHosts => Ok(DaemonResponse::SshHosts {
                hosts: ssh::config::hosts()?,
            }),
            ClientRequest::ResolveSshConfig { target } => Ok(DaemonResponse::SshEffectiveConfig {
                config: ssh::config::resolve(target)?,
            }),
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
            ClientRequest::GetConfig => Ok(DaemonResponse::Config {
                snapshot: self.config.snapshot(),
            }),
            ClientRequest::GetPreferences => Ok(DaemonResponse::Preferences {
                snapshot: self.preferences.get(),
            }),
            ClientRequest::GetEventCursor => Ok(DaemonResponse::EventsUnchanged {
                sequence: self.events.sequence(),
            }),
            ClientRequest::SetPreferences {
                base_revision,
                value,
            } => Ok(DaemonResponse::Preferences {
                snapshot: self.preferences.set(base_revision, value)?,
            }),
            ClientRequest::ReloadConfig => Ok(DaemonResponse::Config {
                snapshot: self.config.reload()?,
            }),
            ClientRequest::WaitForConfig {
                after_revision,
                timeout_ms,
            } => Ok(
                match self.config.wait_after(
                    after_revision,
                    Duration::from_millis(u64::from(timeout_ms.min(30_000))),
                ) {
                    Some(snapshot) => DaemonResponse::Config { snapshot },
                    None => DaemonResponse::ConfigUnchanged {
                        revision: after_revision,
                    },
                },
            ),
            ClientRequest::WaitForEvents {
                after_sequence,
                session_id,
                timeout_ms,
            } => Ok(
                match self.events.wait_after(
                    after_sequence,
                    session_id,
                    Duration::from_millis(u64::from(timeout_ms.min(30_000))),
                ) {
                    Some(update) => DaemonResponse::Events { update },
                    None => DaemonResponse::EventsUnchanged {
                        sequence: after_sequence,
                    },
                },
            ),
            ClientRequest::InvokeConfigAction {
                revision,
                name,
                context,
                args,
            } => Ok(DaemonResponse::ConfigActionResult {
                actions: self.config.invoke(revision, &name, &context, &args)?,
            }),
            ClientRequest::CreateSession {
                project_id,

                cwd,
                size,
                appearance,
                scrollback_limit,
                shell_program,
                shell_args,
                shell_features,
                env,
                remote,
                remote_install_consented,
                remote_cwd,
            } => {
                if let Some(target) = &remote {
                    anyhow::ensure!(target.is_usable(), "Invalid SSH target");
                }
                let connection = remote
                    .as_ref()
                    .map(|target| self.ssh_connections.connection(target))
                    .transpose()?;
                let session = Arc::new(TerminalSession::spawn_with_connection(
                    SessionSpawnConfig {
                        project_id,

                        cwd,
                        size,
                        appearance,
                        scrollback_limit,
                        shell_program: (!shell_program.is_empty()).then_some(shell_program),
                        shell_args: (!shell_args.is_empty()).then_some(shell_args),
                        shell_features,
                        profile_env: env,
                        remote,
                        remote_install_consented,
                        remote_cwd,
                        event_bus: Some(self.events.clone()),
                    },
                    connection,
                )?);
                let summary = session.summary();
                self.sessions.write().insert(session.id, session);
                Ok(DaemonResponse::SessionCreated { session: summary })
            }
            ClientRequest::ReconnectSession { session_id } => {
                let previous = self.session(session_id)?;
                // Repeated requests return the same replacement until the old terminal is retired.
                let mut replacement = previous.reconnected_session.lock();
                if let Some(id) = *replacement
                    && let Ok(session) = self.session(id)
                {
                    return Ok(DaemonResponse::SessionCreated {
                        session: session.summary(),
                    });
                }
                let config = previous.reconnect_config()?;
                let connection = self
                    .ssh_connections
                    .connection(config.remote.as_ref().context("Not an SSH session")?)?;
                let session = Arc::new(TerminalSession::spawn_with_connection(
                    config,
                    Some(connection),
                )?);
                let summary = session.summary();
                self.sessions.write().insert(session.id, session);
                *replacement = Some(summary.id);
                Ok(DaemonResponse::SessionCreated { session: summary })
            }
            ClientRequest::ListSessions => {
                let sessions = self.sessions.read().values().cloned().collect::<Vec<_>>();
                Ok(DaemonResponse::Sessions {
                    sessions: sessions.iter().map(|s| s.summary()).collect(),
                })
            }
            ClientRequest::ReadTerminalText {
                session_id,
                tail_lines,
            } => {
                let (text, truncated) = self.session(session_id)?.read_terminal_text(tail_lines)?;
                Ok(DaemonResponse::TerminalText {
                    session_id,
                    text,
                    truncated,
                })
            }
            ClientRequest::ReadCommandOutput {
                session_id,
                command_id,
                max_bytes,
            } => {
                let (text, truncated) = self
                    .session(session_id)?
                    .read_command_output(command_id, max_bytes)?;
                Ok(DaemonResponse::TerminalText {
                    session_id,
                    text,
                    truncated,
                })
            }
            ClientRequest::LocateCommandOutput {
                session_id,
                command_id,
            } => {
                self.session(session_id)?
                    .locate_command_output(command_id)?;
                Ok(DaemonResponse::Ok)
            }
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
            ClientRequest::WaitForRenderFrame {
                session_id,
                after_revision,
                timeout_ms,
            } => {
                let session = self.session(session_id)?;
                let timeout = Duration::from_millis(u64::from(timeout_ms)).min(MAX_SNAPSHOT_WAIT);
                Ok(
                    if session.events.wait_for_revision(after_revision, timeout) {
                        DaemonResponse::Snapshot {
                            snapshot: session.snapshot(),
                        }
                    } else {
                        DaemonResponse::SnapshotUnchanged {
                            revision: after_revision,
                        }
                    },
                )
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
            ClientRequest::TerminalCompletions {
                session_id,
                input,
                native,
            } => {
                let items = self.session(session_id)?.completions(input.clone(), native);
                Ok(DaemonResponse::TerminalCompletions {
                    session_id,
                    input,
                    items,
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
                expected_context,
            } => {
                self.session(session_id)?.paste_clipboard(
                    selection,
                    contents,
                    sequence,
                    expected_context,
                )?;
                Ok(DaemonResponse::Ok)
            }
            ClientRequest::PasteText {
                session_id,
                text,
                sequence,
                expected_context,
            } => {
                let status =
                    self.session(session_id)?
                        .paste_text(text, sequence, expected_context)?;
                Ok(DaemonResponse::PasteStatus { status })
            }
            ClientRequest::PasteStatus { session_id, id } => {
                let status = self.session(session_id)?.paste_status(id)?;
                Ok(DaemonResponse::PasteStatus { status })
            }
            ClientRequest::CancelPaste { session_id, id } => {
                let status = self.session(session_id)?.cancel_paste(id)?;
                Ok(DaemonResponse::PasteStatus { status })
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
                self.session(session_id)?
                    .selection_start(point, side, kind)?;
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
            ClientRequest::SshConnections => {
                let (revision, connections) = self.ssh_connections.snapshots();
                Ok(DaemonResponse::SshConnections {
                    revision,
                    connections,
                })
            }
            ClientRequest::PortMappings {
                connection_id,
                discover,
            } => {
                let connection = self.ssh_connections.by_id(&connection_id)?;
                if discover {
                    let _ = connection.inspection_output();
                }
                Ok(DaemonResponse::PortMappings {
                    snapshot: connection.snapshot(),
                })
            }
            ClientRequest::CreatePortMapping {
                connection_id,
                request,
            } => {
                let connection = self.ssh_connections.by_id(&connection_id)?;
                let mapping = connection.create(request)?;
                Ok(DaemonResponse::PortMapping {
                    mapping,
                    snapshot: connection.snapshot(),
                })
            }
            ClientRequest::StartPortMapping { connection_id, id } => {
                let connection = self.ssh_connections.by_id(&connection_id)?;
                let mapping = connection.start(&id)?;
                Ok(DaemonResponse::PortMapping {
                    mapping,
                    snapshot: connection.snapshot(),
                })
            }
            ClientRequest::StopPortMapping {
                connection_id,
                id,
                remove,
            } => {
                let connection = self.ssh_connections.by_id(&connection_id)?;
                let mapping = connection.stop(&id, remove)?;
                Ok(DaemonResponse::PortMapping {
                    mapping,
                    snapshot: connection.snapshot(),
                })
            }
            ClientRequest::RememberPortMapping {
                connection_id,
                id,
                remember,
            } => {
                let connection = self.ssh_connections.by_id(&connection_id)?;
                let mapping = connection.remember(&id, remember)?;
                Ok(DaemonResponse::PortMapping {
                    mapping,
                    snapshot: connection.snapshot(),
                })
            }
        }
    }

    pub(super) fn session(&self, session_id: SessionId) -> Result<Arc<TerminalSession>> {
        self.sessions
            .read()
            .get(&session_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown terminal session {session_id}"))
    }
}

fn bind_daemon_listener(socket_path: &Path) -> Result<(UnixListener, fs::File)> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(socket_path.with_extension("lock"))?;
    anyhow::ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Eggie daemon already owns {}",
        socket_path.display()
    );
    match UnixStream::connect(socket_path) {
        Ok(_) => bail!(
            "Eggie daemon is already listening at {}",
            socket_path.display()
        ),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) => {}
        Err(error) => return Err(error.into()),
    }
    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind daemon socket {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    Ok((listener, lock))
}

#[cfg(test)]
mod listener_tests {
    use super::*;

    #[test]
    fn startup_cannot_replace_a_live_listener_or_its_lock() {
        let directory =
            std::env::temp_dir().join(format!("el-{:x}", SessionId::new_v4().as_u128() as u64));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("daemon.sock");
        let legacy = UnixListener::bind(&path).unwrap();
        assert!(bind_daemon_listener(&path).is_err());
        assert!(UnixStream::connect(&path).is_ok());
        drop(legacy);
        let (listener, lock) = bind_daemon_listener(&path).unwrap();
        assert!(bind_daemon_listener(&path).is_err());
        assert!(UnixStream::connect(&path).is_ok());
        drop((listener, lock));
        fs::remove_dir_all(directory).unwrap();
    }
}

pub fn run_daemon(socket_path: &Path, build_id: &str) -> Result<()> {
    let (listener, _lock) = bind_daemon_listener(socket_path)?;
    let state = Arc::new(DaemonState {
        sessions: RwLock::new(HashMap::new()),
        build_id: Arc::from(build_id),
        config: config_service::ConfigService::new(eggie_storage::config_path()),
        events: Arc::new(event_bus::EventBus::new()),
        preferences: preferences_service::PreferencesService::new(),
        ssh_connections: Arc::new(ssh::SshConnections::load(Some(
            eggie_storage::data_file_path("port-mappings.json"),
        ))),
    });

    ssh::SshConnections::start_maintenance(&state.ssh_connections);

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

pub(super) fn serve_connection(stream: UnixStream, state: &DaemonState) -> Result<()> {
    let mut stream = BufReader::new(stream);
    let mut request = Vec::with_capacity(512);
    let mut response = Vec::with_capacity(8 * 1024);
    let mut latency_sensitive = false;
    #[cfg(unix)]
    let mut grid = eggie_ipc::GridWriter::default();
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
                    | ClientRequest::WaitForRenderFrame { .. }
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
            request @ ClientRequest::WaitForRenderFrame { after_revision, .. } => {
                let result = state.handle(request);
                #[cfg(unix)]
                if let DaemonResponse::Snapshot { snapshot } = &result {
                    if grid.send(
                        stream.get_mut(),
                        &mut response,
                        snapshot.clone(),
                        after_revision,
                    )? {
                        continue;
                    }
                }
                write_wire_message(stream.get_mut(), &mut response, &result)?;
            }
            ClientRequest::TerminalImage {
                session_id,
                key,
                offset,
                length,
            } => match state
                .session(session_id)
                .and_then(|session| session.image_chunk_ref(key, offset, length))
            {
                Ok(chunk) => send_terminal_image_chunk(stream.get_mut(), &chunk.wire_chunk())?,
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

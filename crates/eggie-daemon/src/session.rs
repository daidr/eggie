use super::*;

#[derive(Clone)]

pub(super) struct DaemonEventListener(pub(super) Arc<ListenerState>);

impl EventListener for DaemonEventListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                *self.0.title.write() = title.clone();
                self.0.publish_event(
                    "terminal.title_changed",
                    serde_json::json!({ "title": title }),
                );
            }
            Event::ResetTitle => {
                *self.0.title.write() = "shell".to_owned();
                self.0.publish_event(
                    "terminal.title_changed",
                    serde_json::json!({ "title": "shell" }),
                );
            }
            Event::ProgressReport(progress) => {
                self.0.progress.report(progress);
                let state = progress.map(|report| match report.state {
                    VteProgressState::Normal => "normal",
                    VteProgressState::Error => "error",
                    VteProgressState::Indeterminate => "indeterminate",
                    VteProgressState::Paused => "paused",
                });
                self.0.publish_event(
                    "terminal.progress_report",
                    serde_json::json!({
                        "state": state,
                        "percent": progress.and_then(|report| report.percent),
                    }),
                );
            }
            Event::WorkingDirectory(directory) => {
                self.0.report_working_directory(&directory);
                self.0.publish_event(
                    "terminal.pwd_changed",
                    serde_json::json!({ "pwd": directory }),
                );
            }
            Event::SemanticPrompt(prompt, cursor_line, history_size, column, scroll_position) => {
                if matches!(
                    prompt.action,
                    SemanticPromptAction::PromptStart | SemanticPromptAction::FreshLineAndPrompt
                ) {
                    self.0
                        .completion_query_blocked
                        .store(false, Ordering::Release);
                }
                if matches!(
                    prompt.action,
                    SemanticPromptAction::InputStart
                        | SemanticPromptAction::InputStartAndTerminatePrompt
                ) {
                    let mut tracker = self.0.shell_integration.lock();
                    tracker.advance_scroll_base(history_size);
                    let global =
                        (tracker.total_scrolled_lines as i64 + cursor_line as i64).max(0) as u64;
                    if let Some(p) = tracker.input_starts.iter_mut().find(|(l, _)| *l == global) {
                        p.1 = column;
                    } else {
                        tracker.input_starts.push((global, column));
                    }
                }
                if prompt.action == SemanticPromptAction::FreshLineAndPrompt {
                    for id in self.0.program_status.lock().clear_active() {
                        self.0.publish_event(
                            "terminal.program_status_removed",
                            serde_json::json!({ "id": id }),
                        );
                    }
                }
                let mut tracker = self.0.shell_integration.lock();
                tracker.follow_grid_scroll(scroll_position);
                tracker.update(prompt, cursor_line, history_size);
            }
            Event::ProgramStatus(payload, terminator) => {
                self.0.handle_program_status(&payload, &terminator);
            }
            Event::ProgramStatusReset => {
                for id in self.0.program_status.lock().clear_all() {
                    self.0.publish_event(
                        "terminal.program_status_removed",
                        serde_json::json!({ "id": id }),
                    );
                }
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
                for id in self.0.program_status.lock().clear_active() {
                    self.0.publish_event(
                        "terminal.program_status_removed",
                        serde_json::json!({ "id": id }),
                    );
                }
                self.0.publish_event(
                    "terminal.child_exited",
                    serde_json::json!({ "exit_code": status.code() }),
                );
            }
            Event::Exit => {
                let mut status = self.0.status.write();
                if matches!(*status, SessionStatus::Running) {
                    *status = SessionStatus::Exited { code: None };
                }
                drop(status);
                self.0.progress.report(None);
                for id in self.0.program_status.lock().clear_active() {
                    self.0.publish_event(
                        "terminal.program_status_removed",
                        serde_json::json!({ "id": id }),
                    );
                }
                self.0.publish_event(
                    "terminal.child_exited",
                    serde_json::json!({ "exit_code": null }),
                );
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

pub(super) struct TerminalSession {
    pub(super) id: SessionId,
    pub(super) project_id: ProjectId,
    pub(super) initial_directory: PathBuf,
    pub(super) shell_pid: u32,
    pub(super) pty_fd: i32,
    /// Set for a remote profile: the session runs `ssh` instead of a local shell, and inspection and
    /// tunnels go through the control channel rather than local syscalls.
    pub(super) remote: Option<RemoteTarget>,
    /// The ControlPath for this session's ssh master, if multiplexing is enabled. Used by inspection
    /// and tunnels to run commands over the one authenticated connection.
    pub(super) remote_connection: Option<Arc<ssh::SshConnection>>,
    reconnect_config: Option<SessionSpawnConfig>,
    pub(super) reconnected_session: Mutex<Option<SessionId>>,
    pub(super) runtime_metadata: Mutex<SessionRuntimeMetadata>,
    pub(super) inspection_system: Mutex<System>,
    pub(super) terminal: Arc<FairMutex<Term<DaemonEventListener>>>,
    pub(super) events: Arc<ListenerState>,
    pub(super) sender: EventLoopSender,
    pub(super) last_input_sequence: Arc<AtomicU64>,
    pub(super) paste_state: Mutex<paste::PasteState>,
    pub(super) mouse_state: Mutex<TerminalMouseState>,
    pub(super) search_state: Mutex<TerminalSearchState>,
    /// Live terminal-core config knobs that are rebuilt together on every `set_options`. Kept as the
    /// single source of truth so a runtime change to one field (cursor shape, scrollback depth) does
    /// not clobber the other when `terminal_config` reconstructs the whole `Config`.
    pub(super) config_state: Mutex<TerminalConfigState>,
}

/// The subset of the terminal-core `Config` that Eggie mutates at runtime. `set_default_cursor_shape`
/// and `set_scrollback_limit` each update one field and rebuild via [`terminal_config`] from both, so
/// neither reset the other to the built-in default.
pub(super) struct TerminalConfigState {
    pub(super) cursor_shape: CursorShape,
    pub(super) scrollback_limit: usize,
}

/// Per-session terminal search cursor. Tracks the last active match (in grid-absolute coordinates)
/// so `find next`/`find previous` can advance relative to it across requests.
#[derive(Default)]
pub(super) struct TerminalSearchState {
    /// The query the current cursor position belongs to. Reset the cursor whenever it changes.
    pub(super) query: String,
    pub(super) regex: bool,
    /// Grid-absolute range of the current active match, if any. The start keys the match against
    /// the full-buffer enumeration; the end is where forward navigation advances from (and the
    /// start where backward navigation does), so overlapping matches are not re-found.
    pub(super) active: Option<RangeInclusive<Point>>,
}

pub(super) enum TerminalSnapshotUpdate {
    Full(Arc<TerminalSnapshot>),
    Delta(TerminalSnapshotDelta),
}

#[derive(Default)]
pub(super) struct TerminalMouseState {
    pub(super) last_position: Option<TerminalMousePosition>,
    pub(super) accumulated_scroll_x: f64,
    pub(super) accumulated_scroll_y: f64,
}

pub(super) struct SessionRuntimeMetadata {
    pub(super) system: System,
    pub(super) last_foreground_pid: Option<Pid>,
    pub(super) current_process: ProcessSummary,
    pub(super) current_directory: PathBuf,
    pub(super) last_refresh: Option<Instant>,
}

/// Everything the client picks per-session at creation time. Grouped into a struct so the growing
/// set of spawn-time knobs (scrollback depth, shell override, …) does not force every call site to
/// thread positional arguments.
#[derive(Clone)]
pub(super) struct SessionSpawnConfig {
    pub(super) project_id: ProjectId,

    pub(super) cwd: PathBuf,
    pub(super) size: TerminalSize,
    pub(super) appearance: TerminalAppearance,
    /// Scrollback history depth in lines (`0` disables scrollback).
    pub(super) scrollback_limit: usize,
    /// Shell executable override. `None`/empty falls back to `$SHELL` (then `/bin/zsh`).
    pub(super) shell_program: Option<String>,
    /// Custom shell arguments. `None` uses Eggie's default launch args (`-l`, plus `--posix` for bash
    /// integration).
    pub(super) shell_args: Option<Vec<String>>,
    /// Comma-separated `EGGIE_SHELL_FEATURES` tokens (e.g. `"path"`), injected verbatim into the
    /// shell environment. Empty = no features.
    pub(super) shell_features: String,
    /// Profile-supplied environment variables, applied before Eggie's own so integration always wins.
    pub(super) profile_env: BTreeMap<String, String>,
    /// When set, this session is an `ssh` connection rather than a local shell.
    pub(super) remote: Option<eggie_domain::RemoteTarget>,
    /// Whether the user consented to installing the integration on `remote`'s host.
    pub(super) remote_install_consented: bool,
    /// Working directory on the remote host (profile-supplied). Local `cwd` still applies to the
    /// `ssh` process itself.
    pub(super) remote_cwd: Option<String>,
    pub(super) event_bus: Option<Arc<event_bus::EventBus>>,
}

impl TerminalSession {
    #[cfg(test)]
    pub(super) fn spawn(config: SessionSpawnConfig) -> Result<Self> {
        Self::spawn_with_connection(config, None)
    }

    pub(super) fn spawn_with_connection(
        config: SessionSpawnConfig,
        connection: Option<Arc<ssh::SshConnection>>,
    ) -> Result<Self> {
        if let Some(target) = &config.remote {
            anyhow::ensure!(target.is_usable(), "Invalid SSH target");
        }
        let reconnect_config = config.remote.as_ref().map(|_| config.clone());
        let SessionSpawnConfig {
            project_id,

            cwd,
            size,
            appearance,
            scrollback_limit,
            shell_program,
            shell_args,
            shell_features,
            profile_env,
            remote,
            remote_install_consented,
            remote_cwd,
            event_bus,
        } = config;
        let id = SessionId::new_v4();
        let last_input_sequence = Arc::new(AtomicU64::new(0));
        let state = Arc::new(ListenerState::new(
            id,
            size,
            appearance,
            last_input_sequence.clone(),
            event_bus,
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
            &profile_env,
        );
        // A remote profile replaces the local shell with `ssh`. The environment built above still
        // applies to the *local* `ssh` process (TERM and TERMINFO especially — ssh propagates TERM to
        // the remote end), but the profile's own variables and working directory belong on the far
        // side, so they are carried inside the bootstrap payload instead of the local environment.
        let remote_connection = match remote.as_ref() {
            Some(target) => Some(match connection {
                Some(connection) => connection,
                None => ssh::SshConnections::default().connection(target)?,
            }),
            None => None,
        };
        let control_path = remote_connection
            .as_ref()
            .map(|connection| connection.path.as_path());
        let (program, args) = match remote.as_ref() {
            Some(target) => {
                let bootstrap = remote_launch(
                    target,
                    &RemoteScripts {
                        zsh_zshenv: BUNDLED_ZSH_ZSHENV,
                        zsh_integration: BUNDLED_ZSH_INTEGRATION,
                        bash: BUNDLED_BASH_INTEGRATION,
                        fish: BUNDLED_FISH_INTEGRATION,
                        version: integration_version(),
                    },
                    &profile_env,
                    remote_cwd.as_deref(),
                    remote_install_consented,
                    control_path,
                );
                (bootstrap.program, bootstrap.args)
            }
            None => (shell, launch.args),
        };
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: Some(cwd.clone()),
            drain_on_exit: true,
            env: launch.env,
            child_signal_mask: tty::SignalMask::current().ok(),
        };
        let pty = tty::new(&options, window_size(size), id.as_u128() as u64)
            .with_context(|| format!("failed to create PTY in {}", cwd.display()))?;
        let shell_pid = pty.child().id();
        let pty_fd = pty.file().as_raw_fd();
        state.publish_terminal(&mut terminal.lock());
        let publisher = state.clone();
        let resize_state = state.clone();
        let event_loop = EventLoop::new(terminal.clone(), listener, pty, true, false)
            .context("failed to create Alacritty event loop")?
            .with_terminal_resize(move |terminal, size| {
                resize_state.resize_terminal(
                    terminal,
                    TerminalSize {
                        columns: size.num_cols,
                        rows: size.num_lines,
                        cell_width: size.cell_width,
                        cell_height: size.cell_height,
                    },
                );
            })
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
            remote,
            remote_connection,
            reconnect_config,
            reconnected_session: Mutex::new(None),
            runtime_metadata: Mutex::new(SessionRuntimeMetadata {
                system: System::new(),
                last_foreground_pid: None,
                current_process: ProcessSummary {
                    pid: shell_pid,
                    name: shell_name,
                    command_line: None,
                },
                current_directory: cwd,
                last_refresh: None,
            }),
            inspection_system: Mutex::new(System::new()),
            terminal,
            events: state,
            sender,
            last_input_sequence,
            paste_state: Mutex::new(paste::PasteState::default()),
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
    pub(super) fn spawn_default(
        project_id: ProjectId,
        cwd: PathBuf,
        size: TerminalSize,
        appearance: TerminalAppearance,
    ) -> Result<Self> {
        Self::spawn(SessionSpawnConfig {
            project_id,

            cwd,
            size,
            appearance,
            scrollback_limit: TERMINAL_SCROLLBACK_LIMIT,
            // PTY fixtures skip personal shell startup files so initialization cannot consume test input.
            shell_program: Some("/bin/sh".to_owned()),
            shell_args: Some(vec!["-i".to_owned()]),
            shell_features: "path".to_owned(),
            profile_env: BTreeMap::from([("ENV".to_owned(), String::new())]),
            remote: None,
            remote_install_consented: false,
            remote_cwd: None,
            event_bus: None,
        })
    }

    pub(super) fn summary(&self) -> SessionSummary {
        self.refresh_runtime_metadata();
        let runtime_metadata = self.runtime_metadata.lock();
        let reported_location = self.events.reported_location.read().clone();
        let current_directory = reported_location
            .as_ref()
            .filter(|location| location.local && !location.path.is_empty())
            .map(|location| PathBuf::from(&location.path))
            .unwrap_or_else(|| runtime_metadata.current_directory.clone());
        let current_process = runtime_metadata.current_process.clone();
        drop(runtime_metadata);
        let active_command = self
            .events
            .shell_integration
            .lock()
            .current_command
            .as_ref()
            .and_then(|command| command.command_line.clone());
        let status = self.events.status.read().clone();
        SessionSummary {
            id: self.id,
            project_id: self.project_id,

            title: self.events.title.read().clone(),
            initial_directory: self.initial_directory.clone(),
            current_directory,
            reported_location,
            shell_pid: self.shell_pid,
            current_process,
            active_command,
            program_status: self.events.program_status.lock().display(),
            remote: self.remote.as_ref().map(|target| {
                let initial_directory = self.events.remote_ready_directory.read().clone();
                let ready = initial_directory.is_some();
                eggie_protocol::RemoteSessionInfo {
                    target: target.clone(),
                    phase: eggie_protocol::RemoteSessionPhase::from_process(&status, ready),
                    initial_directory,
                }
            }),
            status,
        }
    }

    pub(super) fn reconnect_config(&self) -> Result<SessionSpawnConfig> {
        anyhow::ensure!(matches!(*self.events.status.read(), SessionStatus::Exited { .. }), "SSH session is still running");
        let mut config = self.reconnect_config.clone().context("Not an SSH session")?;
        config.size = *self.events.size.read();
        config.appearance = *self.events.appearance.read();
        config.scrollback_limit = self.config_state.lock().scrollback_limit;
        config.remote_cwd = self.summary().remote_directory().map(str::to_owned).or(config.remote_cwd);
        Ok(config)
    }

    pub(super) fn inspection(&self) -> SessionInspection {
        // A remote session cannot use local syscalls: the shell_pid is the local ssh client, and the
        // processes and ports are on the far side. Run the queries over the control channel instead.
        if self.remote.is_some() {
            return self.remote_inspection();
        }
        let processes = descendant_processes(self.shell_pid, &mut self.inspection_system.lock());
        let ports = listening_ports(&processes);
        SessionInspection {
            session_id: self.id,
            processes,
            ports,
            port_mappings: None,
        }
    }

    /// Share one cached host probe, then select this tab's descendants using its reported shell PID.
    pub(super) fn remote_inspection(&self) -> SessionInspection {
        let empty = SessionInspection {
            session_id: self.id,
            processes: Vec::new(),
            ports: Vec::new(),
            port_mappings: self.remote_connection.as_ref().map(|c| c.snapshot()),
        };
        let Some(mut output) = self
            .remote_connection
            .as_ref()
            .and_then(|c| c.inspection_output().ok())
        else {
            return empty;
        };
        if let Some(pid) = self
            .events
            .shell_integration
            .lock()
            .user_variables
            .get("eggie_shell_pid")
            .and_then(|p| p.parse::<u32>().ok())
        {
            // A shared master owns multiple shells; keep the process tree anchored to this tab.
            output.push_str(&format!("\nSESSION {pid}\n"));
        }
        let (processes, ports) = parse_remote_inspection(&output);
        SessionInspection {
            session_id: self.id,
            processes,
            ports,
            port_mappings: self.remote_connection.as_ref().map(|c| c.snapshot()),
        }
    }

    pub(super) fn refresh_runtime_metadata(&self) {
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
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always);
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
                let command_line = Some(
                    process
                        .cmd()
                        .iter()
                        .map(|arg| arg.to_string_lossy().into_owned())
                        .collect(),
                );
                (process.pid().as_u32(), name, cwd, command_line)
            });
        if let Some((pid, name, cwd, command_line)) = process_metadata {
            let next_process = ProcessSummary {
                pid,
                name,
                command_line,
            };
            if metadata.current_process != next_process {
                self.events.publish_event(
                    "terminal.process_changed",
                    serde_json::to_value(&next_process).unwrap_or_default(),
                );
                metadata.current_process = next_process;
            }
            if let Some(cwd) = cwd.filter(|cwd| !cwd.as_os_str().is_empty()) {
                if metadata.current_directory != cwd {
                    self.events
                        .publish_event("terminal.pwd_changed", serde_json::json!({ "pwd": cwd }));
                    metadata.current_directory = cwd;
                }
            }
        }
    }

    pub(super) fn input(&self, bytes: Vec<u8>, sequence: u64) -> Result<()> {
        let mut paste = self.paste_state.lock();
        if bytes.starts_with(b"\x18\x1e") && paste.is_sending() {
            anyhow::bail!("paste is still in progress");
        }
        self.finish_paste_locked(&mut paste, true)?;
        self.input_unchecked(bytes, sequence)
    }

    pub(super) fn input_unchecked(&self, bytes: Vec<u8>, sequence: u64) -> Result<()> {
        // Serialize private completion queries with input; Enter blocks them until the next prompt.
        let _gate = self.events.completion_input_gate.lock();
        if bytes.len() > 2
            && bytes.starts_with(b"\x18\x1e")
            && (self.events.completion_query_blocked.load(Ordering::Acquire)
                || !matches!(
                    self.events.shell_integration.lock().phase,
                    TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
                ))
        {
            anyhow::bail!("shell input is no longer editable");
        }
        if crate::completion::submits_command(&bytes) {
            self.events
                .completion_query_blocked
                .store(true, Ordering::Release);
        }
        self.events.shell_integration.lock().prompt_input = None;
        *self.events.native_completions.lock() = None;
        *self.events.native_completion_request.lock() = None;
        self.last_input_sequence.store(sequence, Ordering::Release);
        // Scroll-on-keystroke: typing while scrolled back into history snaps the viewport to the
        // live bottom, so the user sees what they type. The kernel pins the viewport during output
        // when display_offset != 0, so without this a keystroke's echo would land off-screen.
        {
            let mut terminal = self.terminal.lock();
            if terminal.grid().display_offset() != 0 {
                terminal.scroll_display(Scroll::Bottom);
                self.events.publish_terminal(&mut terminal);
            }
        }
        self.sender
            .send(Msg::Input(bytes.into()))
            .context("failed to write terminal input")
    }

    pub(super) fn paste(&self, text: String, sequence: u64) -> Result<()> {
        let bracketed = self
            .terminal
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        self.input(paste_bytes(&text, bracketed), sequence)
    }

    pub(super) fn paste_clipboard(
        &self,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
        sequence: u64,
        expected_context: Option<eggie_protocol::TerminalPasteContext>,
    ) -> Result<()> {
        anyhow::ensure!(
            expected_context.is_none_or(|expected| expected == self.paste_context()),
            "paste target changed"
        );
        let mode = *self.terminal.lock().mode();
        if mode.contains(TermMode::PASTE_EVENTS) {
            let mut paste = self.paste_state.lock();
            self.finish_paste_locked(&mut paste, true)?;
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

    pub(super) fn mouse(&self, event: TerminalMouseEvent) -> Result<()> {
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
            let mut paste = self.paste_state.lock();
            if paste.is_sending() && event.action == TerminalMouseAction::Move {
                return Ok(());
            }
            self.finish_paste_locked(&mut paste, true)?;
            self.sender
                .send(Msg::Input(bytes.into()))
                .context("failed to write terminal mouse input")?;
        }
        Ok(())
    }

    pub(super) fn scroll(&self, event: TerminalScrollEvent) -> Result<()> {
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
                self.events
                    .enqueue_pty_reply(bytes)
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
                self.events
                    .enqueue_pty_reply(bytes)
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
            self.events.publish_terminal(&mut terminal);
        }
        drop(terminal);
        Ok(())
    }

    pub(super) fn scroll_to(&self, command: TerminalScrollCommand) -> Result<()> {
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
            self.events.publish_terminal(&mut terminal);
        }
        drop(terminal);
        Ok(())
    }

    /// Scroll the viewport to an absolute scrollback offset (0 = live bottom, `history_size` = the
    /// oldest line). Drives the scrollbar thumb drag. The offset is clamped to the current history
    /// size, and a new frame is published only when the viewport actually moved (matching
    /// [`Self::scroll_to`]).
    pub(super) fn scroll_to_offset(&self, offset: u32) -> Result<()> {
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
                self.events.publish_terminal(&mut terminal);
            }
        }
        drop(terminal);
        Ok(())
    }

    /// Scroll to the previous/next OSC 133 shell prompt. No-op (returns `Ok`) when shell integration
    /// recorded no reachable prompt in that direction, so the keybinding is silent without data.
    pub(super) fn jump_to_prompt(&self, direction: TerminalJumpDirection) -> Result<()> {
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
            self.events.publish_terminal(&mut terminal);
        }
        drop(terminal);
        Ok(())
    }

    pub(super) fn search(&self, request: TerminalSearchRequest) -> Result<TerminalSearchResult> {
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
            self.events.publish_terminal(&mut terminal);
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
    pub(super) fn selection_start(
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
        self.events.publish_terminal(&mut terminal);
        Ok(())
    }

    /// Extend the active selection's head to a viewport cell (drag).
    pub(super) fn selection_update(
        &self,
        point: TerminalCellPosition,
        side: TerminalSelectionSide,
    ) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let absolute = viewport_point_to_absolute(&*terminal, point);
        if let Some(selection) = terminal.selection.as_mut() {
            selection.update(absolute, selection_side(side));
        }
        self.events.publish_terminal(&mut terminal);
        Ok(())
    }

    /// Clear the active selection.
    pub(super) fn selection_clear(&self) -> Result<()> {
        let mut terminal = self.terminal.lock();
        if terminal.selection.take().is_some() {
            self.events.publish_terminal(&mut terminal);
        }
        Ok(())
    }

    /// Select the entire buffer including scrollback.
    pub(super) fn select_all(&self) -> Result<()> {
        let mut terminal = self.terminal.lock();
        let start = Point::new(terminal.topmost_line(), Column(0));
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.include_all();
        let end = Point::new(terminal.bottommost_line(), terminal.grid().last_column());
        selection.update(end, Side::Right);
        terminal.selection = Some(selection);
        self.events.publish_terminal(&mut terminal);
        Ok(())
    }

    /// Extract the current selection's text (whole scrollback span, soft-wrap unwrapped, trailing
    /// whitespace trimmed). Returns `None` when there is no active or non-empty selection.
    pub(super) fn selection_text(&self) -> Result<Option<String>> {
        let terminal = self.terminal.lock();
        Ok(terminal.selection_to_string())
    }

    pub(super) fn focus(&self, focused: bool) -> Result<()> {
        let terminal = self.terminal.lock();
        let report_focus = terminal.mode().contains(TermMode::FOCUS_IN_OUT);
        drop(terminal);
        if report_focus {
            // A focus change must not turn its escape sequence into pasted document text.
            let mut paste = self.paste_state.lock();
            if paste.is_sending() {
                paste.deferred_focus = Some(focused);
                return Ok(());
            }
            let bytes = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.sender
                .send(Msg::Input(bytes.to_vec().into()))
                .context("failed to write terminal focus input")?;
        }
        Ok(())
    }

    pub(super) fn resize(&self, size: TerminalSize) -> Result<()> {
        // Keep only the latest width during a prompt shrink, so a slow shell cannot paint every
        // intermediate old-width prompt into a progressively narrower grid.
        let settle = {
            let terminal = self.terminal.lock();
            let prompt = matches!(
                self.events.shell_integration.lock().phase,
                TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
            );
            if (size.columns as usize) < terminal.columns()
                && prompt
                && !terminal.mode().contains(TermMode::ALT_SCREEN)
            {
                Duration::from_millis(100)
            } else {
                Duration::ZERO
            }
        };
        self.sender
            .send(Msg::ResizeQueued(window_size(size), settle))
            .context("failed to resize PTY")
    }

    pub(super) fn set_appearance(&self, appearance: TerminalAppearance) {
        *self.events.appearance.write() = appearance;
    }

    pub(super) fn set_progress_timeouts(&self, timeouts: TerminalProgressTimeouts) {
        self.events.progress.set_timeouts(timeouts);
    }

    pub(super) fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.events.snapshot()
    }

    /// Copy a chunk of a published image into an owned `Vec`. The production transfer path uses the
    /// zero-copy [`image_chunk_ref`](Self::image_chunk_ref) instead; this allocating variant exists
    /// only for tests that want to assert on the pixel bytes directly.
    #[cfg(test)]
    pub(super) fn image_chunk(
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
            chunk.wire_chunk().bytes().to_vec(),
        ))
    }

    pub(super) fn image_chunk_ref(
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

    pub(super) fn wait_for_snapshot(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalSnapshotUpdate> {
        self.events
            .wait_for_revision(after_revision, timeout)
            .then(|| self.events.snapshot_after(after_revision))
    }

    pub(super) fn wait_for_progress(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalProgressUpdate> {
        self.events.progress.wait_after(after_revision, timeout)
    }

    pub(super) fn progress_revision(&self) -> u64 {
        self.events.progress.revision()
    }

    pub(super) fn wait_for_osc_events(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<TerminalOscEventUpdate> {
        self.events.osc_events.wait_after(after_revision, timeout)
    }

    pub(super) fn osc_event_revision(&self) -> u64 {
        self.events.osc_events.revision()
    }

    pub(super) fn shell_integration_state(&self) -> TerminalShellIntegrationState {
        self.events.shell_integration.lock().snapshot()
    }

    pub(super) fn complete_clipboard_read(
        &self,
        request_id: u64,
        contents: Vec<TerminalClipboardContent>,
    ) {
        self.events.complete_clipboard_read(request_id, contents);
    }

    pub(super) fn notification_response(&self, notification_id: &str, activated: bool) {
        self.events
            .notification_response(notification_id, activated);
    }

    pub(super) fn complete_file_transfer(
        &self,
        request_id: u64,
        destination: Option<PathBuf>,
    ) -> Result<()> {
        self.events.complete_file_transfer(request_id, destination)
    }

    pub(super) fn set_osc_policy(&self, allow_clipboard_read: bool) {
        self.events
            .allow_clipboard_read
            .store(allow_clipboard_read, Ordering::Release);
    }

    pub(super) fn set_url_detection(&self, detect_urls: bool) {
        let previous = self.events.detect_urls.swap(detect_urls, Ordering::AcqRel);
        if previous != detect_urls {
            // Republish so detected-link highlights appear (or clear) without waiting for the next
            // terminal output.
            self.events.publish_terminal(&mut self.terminal.lock());
        }
    }

    /// Set the session's default cursor shape. This changes only the *default* (`set_options`
    /// replaces the whole config, reconstructed from the session's live [`TerminalConfigState`]), so a
    /// program that has issued DECSCUSR keeps its runtime override — matching Ghostty's semantics.
    /// Rebuilding from `config_state` also preserves any custom scrollback depth.
    pub(super) fn set_default_cursor_shape(&self, shape: TerminalCursorShape) {
        let mut terminal = self.terminal.lock();
        let mut state = self.config_state.lock();
        state.cursor_shape = kernel_cursor_shape(shape);
        let config = terminal_config(state.cursor_shape, state.scrollback_limit);
        drop(state);
        terminal.set_options(config);
        self.events.publish_terminal(&mut terminal);
    }

    /// Set the session's scrollback history depth at runtime. Rebuilds the whole config from the
    /// live [`TerminalConfigState`] (preserving the current default cursor shape); the terminal core
    /// shrinks the buffer if the new depth is smaller than the current history.
    pub(super) fn set_scrollback_limit(&self, limit: usize) {
        let mut terminal = self.terminal.lock();
        let mut state = self.config_state.lock();
        if state.scrollback_limit == limit {
            return;
        }
        state.scrollback_limit = limit;
        let config = terminal_config(state.cursor_shape, state.scrollback_limit);
        drop(state);
        terminal.set_options(config);
        self.events.publish_terminal(&mut terminal);
    }

    pub(super) fn terminate(&self) {
        self.events.progress.report(None);
        let _ = self.sender.send(Msg::Shutdown);
    }
}

impl ListenerState {
    fn resize_terminal(&self, terminal: &mut Term<DaemonEventListener>, size: TerminalSize) {
        // Match the PTY publisher's terminal -> metadata lock ordering. Taking the size lock
        // first can deadlock when sustained output publishes while a resize is waiting.
        *self.size.write() = size;
        // Read the semantic phase and prompt-start line under the same terminal ->
        // shell_integration lock order used by `send_event`, so this can never deadlock. Together
        // they decide whether — and where — the active prompt region is cleared before reflow.
        // A column change reflows the whole buffer, invalidating every stored jump-point line
        // coordinate, so the index is dropped and rebuilt from subsequent prompts.
        let columns_changed = terminal.columns() != size.columns as usize;
        let old_cursor_line = terminal.grid().cursor.point.line.0;
        let (phase, prompt_start_line) = {
            let mut shell = self.shell_integration.lock();
            shell.follow_grid_scroll(terminal.grid().scroll_position());
            shell.advance_scroll_base(terminal.grid().history_size());
            if columns_changed || terminal.mode().contains(TermMode::ALT_SCREEN) {
                shell.clear_jump_points();
                shell.input_starts.clear();
            }
            (shell.phase, shell.prompt_start_line)
        };
        resize_terminal_with_history_reflow(terminal, size, phase, prompt_start_line);
        self.shell_integration.lock().rebase_after_resize(
            terminal.grid().cursor.point.line.0 - old_cursor_line,
            terminal.grid().history_size(),
            terminal.grid().scroll_position(),
        );
        terminal.set_kitty_graphics_cell_size(size.cell_width, size.cell_height);
    }
}

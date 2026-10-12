use super::*;

pub(super) struct ListenerState {
    pub(super) native_result_revision: AtomicU64,
    pub(super) completion_input_gate: Mutex<()>,
    pub(super) completion_query_blocked: AtomicBool,
    pub(super) completion_engine: &'static eggie_completion::CompletionEngine,
    pub(super) native_completions: Mutex<
        Option<(
            eggie_completion::PromptInput,
            Vec<eggie_completion::CompletionItem>,
        )>,
    >,
    pub(super) native_completion_request: Mutex<Option<(eggie_completion::PromptInput, Instant)>>,
    pub(super) session_id: SessionId,
    pub(super) event_bus: Option<Arc<event_bus::EventBus>>,
    pub(super) title: RwLock<String>,
    pub(super) size: RwLock<TerminalSize>,
    pub(super) status: RwLock<SessionStatus>,
    pub(super) revision: AtomicU64,
    pub(super) revision_wait: Mutex<()>,
    pub(super) revision_changed: Condvar,
    pub(super) sender: Mutex<Option<EventLoopSender>>,
    pub(super) paste_replies: Mutex<Option<VecDeque<Vec<u8>>>>,
    pub(super) appearance: RwLock<TerminalAppearance>,
    pub(super) last_input_sequence: Arc<AtomicU64>,
    pub(super) published_snapshots: RwLock<VecDeque<Arc<TerminalSnapshot>>>,
    pub(super) published_images: RwLock<HashMap<TerminalImageKey, PublishedTerminalImage>>,
    pub(super) progress: ProgressTracker,
    pub(super) reported_location: RwLock<Option<TerminalReportedLocation>>,
    pub(super) shell_integration: Mutex<ShellIntegrationTracker>,
    pub(super) program_status: Mutex<ProgramStatusTracker>,
    pub(super) osc_events: OscEventTracker,
    pub(super) next_osc_request_id: AtomicU64,
    pub(super) pending_clipboard_reads: Mutex<HashMap<u64, ClipboardReadResponder>>,
    pub(super) paste_clipboard_grants: Mutex<HashMap<String, PasteClipboardGrant>>,
    pub(super) notification_replies: Mutex<HashMap<String, NotificationReply>>,
    pub(super) live_notifications: Mutex<HashSet<String>>,
    pub(super) pending_notifications: Mutex<HashMap<String, PendingNotification>>,
    pub(super) rich_clipboard_write: Mutex<Option<RichClipboardWrite>>,
    pub(super) pending_file_transfers: Mutex<HashMap<u64, PendingFileTransfer>>,
    pub(super) kitty_file_transfers: Mutex<HashMap<String, KittyIncomingTransfer>>,
    pub(super) iterm2_multipart: Mutex<Option<Iterm2MultipartTransfer>>,
    pub(super) allow_clipboard_read: AtomicBool,
    /// Whether bare-URL auto-detection is enabled for this session (client-configurable, default on).
    pub(super) detect_urls: AtomicBool,
    /// Lazily-compiled regex for auto-detecting bare URLs in terminal output. `None` = not yet
    /// built; `Some(None)` = compilation failed (detection permanently disabled); `Some(Some(_))` =
    /// the compiled DFA, reused across every publish so the DFA is never rebuilt per frame.
    pub(super) url_regex: Mutex<Option<Option<RegexSearch>>>,
    /// Timestamp of the last bell we forwarded, used to throttle a burst of `\a` so a runaway
    /// program cannot flood the bounded OSC event queue or thrash the UI flash animation.
    pub(super) last_bell: Mutex<Option<Instant>>,
}

pub(super) enum ClipboardReadResponder {
    Osc52(Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
    Kitty {
        id: String,
        terminator: String,
        selection: TerminalClipboardSelection,
        list_only: bool,
    },
}

pub(super) struct PasteClipboardGrant {
    pub(super) selection: TerminalClipboardSelection,
    pub(super) contents: Vec<TerminalClipboardContent>,
    pub(super) expires_at: Instant,
}

pub(super) struct RichClipboardWrite {
    pub(super) id: String,
    pub(super) selection: TerminalClipboardSelection,
    pub(super) contents: Vec<TerminalClipboardContent>,
    pub(super) aliases: Vec<(String, String)>,
    pub(super) total_size: usize,
    pub(super) terminator: String,
}

pub(super) enum PendingFileTransfer {
    Iterm2 { temp_path: PathBuf },
    Kitty { transfer_id: String },
}

pub(super) struct KittyIncomingTransfer {
    pub(super) destination: Option<PathBuf>,
    pub(super) terminator: String,
    pub(super) files: HashMap<String, KittyIncomingFile>,
}

pub(super) struct KittyIncomingFile {
    pub(super) path: PathBuf,
    pub(super) sink: Option<KittyIncomingSink>,
    pub(super) size: u64,
    pub(super) expected_size: Option<u64>,
    pub(super) permissions: Option<u32>,
    pub(super) directory: bool,
}

pub(super) enum KittyIncomingSink {
    Plain(fs::File),
    Zlib(ZlibDecoder<fs::File>),
}

impl KittyIncomingSink {
    pub(super) fn write_chunk(&mut self, data: &[u8]) -> std::io::Result<u64> {
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

    pub(super) fn finish(self) -> std::io::Result<fs::File> {
        match self {
            Self::Plain(mut file) => {
                file.flush()?;
                Ok(file)
            }
            Self::Zlib(decoder) => decoder.finish(),
        }
    }
}

pub(super) struct Iterm2MultipartTransfer {
    pub(super) request_id: u64,
    pub(super) suggested_name: String,
    pub(super) temp_path: PathBuf,
    pub(super) file: fs::File,
    pub(super) size: u64,
}

#[derive(Clone)]
pub(super) struct NotificationReply {
    pub(super) protocol_id: String,
    pub(super) terminator: String,
    pub(super) activation_response_requested: bool,
    pub(super) close_response_requested: bool,
}

#[derive(Clone, Default)]
pub(super) struct PendingNotification {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) activation_response_requested: bool,
    pub(super) close_response_requested: bool,
}

pub(super) struct ReadyNotification {
    pub(super) id: String,
    pub(super) protocol_id: String,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) activation_response_requested: bool,
    pub(super) close_response_requested: bool,
    pub(super) terminator: String,
}

pub(super) struct PublishedTerminalImage {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pixels: Arc<PixelBuffer>,
}

pub(super) struct PublishedTerminalImageChunk {
    pub(super) key: TerminalImageKey,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) total_length: u32,
    pub(super) offset: u32,
    pub(super) pixels: Arc<PixelBuffer>,
    pub(super) end: usize,
}

impl PublishedTerminalImageChunk {
    pub(super) fn wire_chunk(&self) -> TerminalImageChunk<'_> {
        TerminalImageChunk {
            key: self.key,
            width: self.width,
            height: self.height,
            total_length: self.total_length,
            offset: self.offset,
            pixels: &self.pixels,
            end: self.end,
        }
    }
}

impl ListenerState {
    pub(super) fn new(
        session_id: SessionId,
        size: TerminalSize,
        appearance: TerminalAppearance,
        last_input_sequence: Arc<AtomicU64>,
        event_bus: Option<Arc<event_bus::EventBus>>,
    ) -> Self {
        Self {
            native_result_revision: AtomicU64::new(0),
            completion_input_gate: Mutex::new(()),
            completion_query_blocked: AtomicBool::new(false),
            completion_engine: eggie_completion::CompletionEngine::shared(),
            native_completions: Mutex::new(None),
            native_completion_request: Mutex::new(None),
            session_id,
            event_bus,
            title: RwLock::new("shell".to_owned()),
            size: RwLock::new(size),
            status: RwLock::new(SessionStatus::Running),
            revision: AtomicU64::new(0),
            revision_wait: Mutex::new(()),
            revision_changed: Condvar::new(),
            sender: Mutex::new(None),
            paste_replies: Mutex::new(None),
            appearance: RwLock::new(appearance),
            last_input_sequence,
            published_snapshots: RwLock::new(VecDeque::with_capacity(PUBLISHED_SNAPSHOT_HISTORY)),
            published_images: RwLock::new(HashMap::new()),
            progress: ProgressTracker::new(session_id),
            reported_location: RwLock::new(None),
            shell_integration: Mutex::new(ShellIntegrationTracker::default()),
            program_status: Mutex::new(ProgramStatusTracker::default()),
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

    pub(super) fn publish_event(&self, name: &str, payload: serde_json::Value) {
        if let Some(bus) = &self.event_bus {
            bus.publish(name, Some(self.session_id), payload);
        }
    }

    pub(super) fn publish_terminal(&self, terminal: &mut Term<DaemonEventListener>) {
        // Every caller already owns the terminal mutex. Build the immutable frame under that
        // existing ownership and only take the snapshot lock for the final Arc swap.
        let started = Instant::now();
        // Keep the jump-to-prompt coordinate base current with the live scrollback depth.
        self.shell_integration
            .lock()
            .observe_scroll(terminal.grid().history_size(), TERMINAL_SCROLLBACK_LIMIT);
        let revision = self.revision.load(Ordering::Relaxed).wrapping_add(1);
        let base = self.published_snapshots.read().back().cloned();
        let mut snapshot = snapshot_terminal_incremental(
            terminal,
            base.as_deref(),
            self.session_id,
            *self.size.read(),
            self.title.read().clone(),
            revision,
            self.last_input_sequence.load(Ordering::Acquire),
        );
        snapshot.prompt_input = self.prompt_input_for_frame(terminal);
        snapshot.prompt_editable = !self.completion_query_blocked.load(Ordering::Acquire)
            && matches!(
                self.shell_integration.lock().phase,
                TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
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
    pub(super) fn detect_urls_into(
        &self,
        snapshot: &mut TerminalSnapshot,
        terminal: &Term<DaemonEventListener>,
    ) {
        if !self.detect_urls.load(Ordering::Acquire) {
            return;
        }
        // Precheck: only scan when a scheme separator is visible. This keeps high-throughput output
        // (which rarely contains URLs) from paying for a regex pass every frame. Scans the sparse
        // cells directly — materializing the grid into per-row `String`s here (the old
        // `plain_lines()` precheck) cost as much as the regex it guards.
        if !snapshot.has_scheme_candidate() {
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

    pub(super) fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.published_snapshots
            .read()
            .back()
            .expect("terminal publishes its initial frame before becoming visible")
            .clone()
    }

    pub(super) fn snapshot_after(&self, after_revision: u64) -> TerminalSnapshotUpdate {
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
    pub(super) fn signal_revision_for_test(&self) {
        self.revision.fetch_add(1, Ordering::Release);
        self.revision_changed.notify_all();
    }

    pub(super) fn wait_for_revision(&self, after_revision: u64, timeout: Duration) -> bool {
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

    pub(super) fn write_to_pty(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.enqueue_pty_reply(bytes.into());
    }

    pub(super) fn enqueue_pty_reply(&self, bytes: Vec<u8>) -> Result<()> {
        let mut replies = self.paste_replies.lock();
        if let Some(queue) = replies.as_mut() {
            queue.push_back(bytes);
            return Ok(());
        }
        let sender = self
            .sender
            .lock()
            .clone()
            .context("terminal input unavailable")?;
        sender
            .send(Msg::Input(bytes.into()))
            .context("failed to write terminal reply")
    }

    pub(super) fn release_paste_replies(&self) {
        let mut replies = self.paste_replies.lock();
        if let Some(queue) = replies.take()
            && let Some(sender) = self.sender.lock().clone()
        {
            for bytes in queue {
                let _ = sender.send(Msg::Input(bytes.into()));
            }
        }
    }

    pub(super) fn report_working_directory(&self, value: &str) {
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
    pub(super) fn ring_bell(&self) -> bool {
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
        self.publish_event("terminal.bell", serde_json::Value::Null);
        true
    }

    pub(super) fn update_remote_host(&self, value: &str) {
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

    pub(super) fn handle_program_status(&self, payload: &str, terminator: &str) {
        match program_status::parse(payload) {
            Some(ProgramStatusCommand::Query) => {
                self.write_to_pty(format!("\x1b]7501;?{terminator}").into_bytes());
            }
            Some(ProgramStatusCommand::Report { id, status }) => {
                let removed = self.program_status.lock().apply(id.clone(), status.clone());
                for removed_id in removed {
                    self.publish_event(
                        "terminal.program_status_removed",
                        serde_json::json!({ "id": removed_id }),
                    );
                }
                if let Some(status) = status {
                    self.publish_event(
                        "terminal.program_status_changed",
                        serde_json::json!({ "id": id, "status": status }),
                    );
                }
            }
            None => {}
        }
    }

    pub(super) fn handle_iterm2_command(&self, payload: &str, terminator: &str) {
        if self.handle_completion_report(payload) {
            return;
        }
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

    pub(super) fn iterm2_variable(&self, name: &str) -> Option<String> {
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

pub(super) fn cleanup_kitty_transfer(transfer: KittyIncomingTransfer) {
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

pub(super) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(super) fn render_metrics_enabled() -> bool {
    *RENDER_METRICS_ENABLED.get_or_init(|| {
        std::env::var_os("EGGIE_RENDER_METRICS")
            .is_some_and(|value| !value.is_empty() && value != "0")
    })
}

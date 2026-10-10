use super::*;

impl EggieApp {
    pub(super) fn start_polling(&mut self, cx: &mut Context<Self>) {
        self.ensure_snapshot_watchers(cx);
        self.start_session_polling(cx);
        self.ensure_progress_watchers(cx);
        self.ensure_osc_watchers(cx);
    }

    pub(super) fn ensure_progress_watchers(&mut self, cx: &mut Context<Self>) {
        let live_sessions = self
            .window_session_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        self.session_feed
            .progress_updates
            .retain(|session_id, _| live_sessions.contains(session_id));
        self.session_feed
            .pending_progress_updates
            .retain(|session_id, _| live_sessions.contains(session_id));
        self.session_feed
            .progress_watchers
            .retain(|session_id| live_sessions.contains(session_id));

        for session_id in live_sessions {
            if !self.session_feed.progress_watchers.insert(session_id) {
                continue;
            }
            self.start_progress_watching(session_id, cx);
        }
    }

    pub(super) fn ensure_osc_watchers(&mut self, cx: &mut Context<Self>) {
        let live_sessions = self
            .window_session_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        self.session_feed
            .osc_watchers
            .retain(|session_id| live_sessions.contains(session_id));
        for session_id in live_sessions {
            if self.session_feed.osc_watchers.insert(session_id) {
                self.start_osc_watching(session_id, cx);
            }
        }
    }

    pub(super) fn start_osc_watching(&self, session_id: SessionId, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection = None;
            let mut after_revision = 0;
            loop {
                let session_exists = this
                    .update(cx, |app, _| {
                        app.session_feed
                            .sessions
                            .iter()
                            .any(|session| session.id == session_id)
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

    pub(super) fn handle_osc_event(
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
                    .session_feed
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
                        prompt: Some(
                            self.language
                                .choose_receive_files_prompt()
                                .to_owned()
                                .into(),
                        ),
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

    pub(super) fn start_progress_watching(&self, session_id: SessionId, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection = None;
            let mut after_revision = 0;
            loop {
                let session_exists = this
                    .update(cx, |app, _| {
                        app.session_feed
                            .sessions
                            .iter()
                            .any(|session| session.id == session_id)
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
                            .session_feed
                            .pending_progress_updates
                            .get(&session_id)
                            .or_else(|| app.session_feed.progress_updates.get(&session_id))
                            .is_none_or(|current| current.revision < update.revision);
                        if should_replace {
                            app.session_feed
                                .pending_progress_updates
                                .insert(session_id, update);
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

    pub(super) fn schedule_progress_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.progress_frame_scheduled {
            return;
        }
        self.progress_frame_scheduled = true;
        cx.on_next_frame(window, |app, window, cx| {
            app.progress_frame_scheduled = false;
            for (session_id, update) in app.session_feed.pending_progress_updates.drain() {
                app.session_feed.progress_updates.insert(session_id, update);
            }
            app.ensure_progress_animation(window, cx);
            cx.notify();
        });
        window.refresh();
    }

    pub(super) fn has_animated_icon_indicator(&self) -> bool {
        self.session_feed.sessions.iter().any(|session| {
            icon_indicator(
                session.program_status.as_ref(),
                self.session_feed
                    .progress_updates
                    .get(&session.id)
                    .and_then(|update| update.progress),
            )
            .is_some_and(IconIndicator::is_animated)
        })
    }

    pub(super) fn ensure_progress_animation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_animated_icon_indicator() || self.progress_animation_scheduled {
            return;
        }

        self.progress_animation_scheduled = true;
        cx.on_next_frame(window, |app, window, cx| {
            app.progress_animation_scheduled = false;
            if app.has_animated_icon_indicator() {
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
    pub(super) fn ensure_bell_flash(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn bell_flash_tint(&self, session_id: SessionId) -> Option<u32> {
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
    pub(super) fn cursor_should_blink(&self, snapshot: &TerminalSnapshot) -> bool {
        snapshot.cursor_shape != TerminalCursorShape::Hidden
            && self.cursor_blink.resolve(snapshot.cursor_blinking)
    }

    /// Whether the cursor should be painted this frame: solid unless it is blinking and currently in
    /// the "off" half of the blink cycle.
    pub(super) fn cursor_visible_this_frame(&self, snapshot: &TerminalSnapshot) -> bool {
        if !self.cursor_should_blink(snapshot) {
            return true;
        }
        let elapsed = self.cursor_blink_epoch.elapsed().as_secs_f32();
        elapsed.rem_euclid(CURSOR_BLINK_PERIOD.as_secs_f32())
            < CURSOR_BLINK_PERIOD.as_secs_f32() / 2.
    }

    /// Whether the active session's cursor is blinking right now (drives the blink timer loop).
    pub(super) fn active_cursor_blinks(&self) -> bool {
        self.active_session_id()
            .and_then(|session_id| self.session_feed.snapshots.get(&session_id))
            .is_some_and(|snapshot| self.cursor_should_blink(snapshot))
    }

    /// Drive the cursor blink: a background timer that repaints every half-cycle while the active
    /// session's cursor is blinking, and exits as soon as it stops (so an idle, non-blinking cursor
    /// costs nothing). Uses a background timer (not the display link) so it ticks regardless of
    /// frontmost state, mirroring [`Self::ensure_bell_flash`].
    pub(super) fn ensure_cursor_blink(&mut self, cx: &mut Context<Self>) {
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

    /// The badge color and profile name for a tab, or `None` when it should carry no badge.
    ///
    /// Uses [`ProfileStore::profile`] rather than `resolve`: a tab whose profile was deleted while it
    /// ran must lose its badge, not inherit the built-in Default's. That matches
    /// [`ProfileStore::appearance_override`] — a deleted profile means "no overrides" everywhere, so a
    /// terminal never silently adopts a different profile's identity.
    pub(super) fn tab_profile_badge(
        &self,
        item: &TabItem,
        cx: &App,
    ) -> Option<(u32, SharedString)> {
        let profile = self.profile_store.read(cx).profile(item.profile_id?)?;
        Some((
            profile.badge_color?,
            SharedString::from(profile.name.clone()),
        ))
    }

    pub(super) fn icon_for_item(
        &self,
        kind: ItemKind,
        session_id: Option<SessionId>,
    ) -> ProcessIcon {
        match kind {
            ItemKind::Terminal => session_id
                .and_then(|id| {
                    self.session_feed
                        .sessions
                        .iter()
                        .find(|session| session.id == id)
                })
                .map(|session| process_icons::for_session(session))
                .unwrap_or(ProcessIcon::Terminal),
            ItemKind::File => ProcessIcon::File,
            ItemKind::Browser => ProcessIcon::Browser,
            ItemKind::Settings => ProcessIcon::Settings,
        }
    }

    pub(super) fn render_item_icon(
        &self,
        kind: ItemKind,
        session_id: Option<SessionId>,
        element_id: String,
    ) -> AnyElement {
        let icon_kind = self.icon_for_item(kind, session_id);
        let transition = session_id.and_then(|id| {
            let mut states = self.sidebar.tab_icon_states.borrow_mut();
            let state = states.entry(id).or_insert(TabIconState {
                icon: icon_kind,
                previous: None,
                revision: 1,
                started_at: Instant::now(),
            });
            if state.icon != icon_kind {
                state.previous = Some(std::mem::replace(&mut state.icon, icon_kind));
                state.revision += 1;
                state.started_at = Instant::now();
            }
            if state.started_at.elapsed() < Duration::from_millis(220) {
                Some((id, *state))
            } else {
                state.previous = None;
                None
            }
        });
        let image_for = |icon| {
            process_icons::card_stack(&[icon], self.use_dark, SIDEBAR_CARD_SIZE)
                .expect("single process card")
        };
        let image = image_for(icon_kind);
        let width = process_icons::stack_width(1, SIDEBAR_CARD_SIZE);
        let height = SIDEBAR_CARD_SIZE + 5.;
        let content: AnyElement = if let Some((id, state)) = transition {
            let previous = state.previous.map(image_for);
            div()
                .relative()
                .w(px(width))
                .h(px(height))
                .with_animation(
                    format!("tab-icon-{id}-{element_id}-{}", state.revision),
                    Animation::new(Duration::from_millis(220)).with_easing(gpui::ease_in_out),
                    move |mut element, progress| {
                        if let Some(previous) = &previous {
                            element = element.child(
                                img(previous.clone())
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .w(px(width))
                                    .h(px(height))
                                    .opacity(1. - progress),
                            );
                        }
                        element.child(
                            img(image.clone())
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(width))
                                .h(px(height))
                                .opacity(progress),
                        )
                    },
                )
                .into_any_element()
        } else {
            img(image).w(px(width)).h(px(height)).into_any_element()
        };
        let program_status = session_id
            .and_then(|id| {
                self.session_feed
                    .sessions
                    .iter()
                    .find(|session| session.id == id)
            })
            .and_then(|session| session.program_status.as_ref());
        let progress = session_id
            .and_then(|id| self.session_feed.progress_updates.get(&id))
            .and_then(|update| update.progress);
        let indicator = icon_indicator(program_status, progress);
        let base = div()
            .flex()
            .flex_none()
            .relative()
            .items_center()
            .justify_center()
            .w(px(width))
            .h(px(height))
            .child(content);
        let Some(indicator) = indicator else {
            return base.into_any_element();
        };
        let label = match indicator {
            IconIndicator::Working(percent) => {
                let mut label =
                    program_state_label(TerminalProgramState::Working, self.language).to_owned();
                if let Some(percent) = percent {
                    label.push_str(&format!(" · {percent}%"));
                }
                label
            }
            IconIndicator::Attention(state) => program_state_label(state, self.language).to_owned(),
            IconIndicator::Progress(progress) => progress_label(progress, self.language),
        };
        let base = base
            .id(element_id.clone())
            .role(Role::Image)
            .aria_label(SharedString::from(label.clone()))
            .tooltip({
                let colors = self.colors;
                let language = self.language;
                move |_, cx| {
                    let text = match indicator {
                        IconIndicator::Progress(progress) => progress_label(progress, language),
                        _ => label.clone(),
                    };
                    cx.new(|_| TextTooltip {
                        text: text.into(),
                        colors,
                    })
                    .into()
                }
            });

        let ring = match indicator {
            IconIndicator::Working(percent) => Some((
                if percent.is_some() {
                    TerminalProgressState::Normal
                } else {
                    TerminalProgressState::Indeterminate
                },
                percent,
                0x74aaf4,
            )),
            IconIndicator::Progress(progress) => Some((
                progress.state,
                progress.percent,
                match progress.state {
                    TerminalProgressState::Error => self.interface_theme.palette[1],
                    TerminalProgressState::Paused => self.interface_theme.palette[3],
                    TerminalProgressState::Normal | TerminalProgressState::Indeterminate => {
                        self.colors.accent
                    }
                },
            )),
            IconIndicator::Attention(_) => None,
        };
        if let Some((state, percent, color)) = ring {
            let phase = (self.progress_animation_epoch.elapsed().as_secs_f32() * 0.8).fract();
            let track_color = self.colors.border;
            return base
                .child(
                    div()
                        .id(format!("{element_id}-indicator"))
                        .absolute()
                        .top_0()
                        .right_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(12.))
                        .rounded_full()
                        .bg(rgb(self.colors.panel))
                        .shadow_sm()
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| {
                                    paint_indicator_ring(
                                        bounds,
                                        state,
                                        percent,
                                        phase,
                                        color,
                                        track_color,
                                        window,
                                    )
                                },
                            )
                            .size(px(10.)),
                        ),
                )
                .into_any_element();
        }

        let IconIndicator::Attention(state) = indicator else {
            return base.into_any_element();
        };
        let color = if state == TerminalProgramState::Error {
            0xe98279
        } else {
            0x83b5f5
        };
        base.child(
            div()
                .id(format!("{element_id}-indicator"))
                .absolute()
                .top_0()
                .right_0()
                .size(px(9.))
                .rounded_full()
                .border_1()
                .border_color(rgb(self.colors.panel))
                .bg(rgb(color))
                .shadow_sm(),
        )
        .into_any_element()
    }

    pub(super) fn schedule_terminal_images(
        &mut self,
        snapshot: &TerminalSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.terminal_renderer.retain_snapshot_images(snapshot);
        for descriptor in &snapshot.images {
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
            // A persistent worker now owns this image stream: it holds one reusable daemon
            // connection and keeps pulling the newest un-cached generation of `image_id` until the
            // image disappears or the session hides. `terminal_images_in_flight` marks the worker
            // as alive; the worker clears it atomically on exit, and the next snapshot re-arms it
            // through the frame path if a fresh generation appears afterward.
            self.start_terminal_image_stream(snapshot.session_id, descriptor.key.id, cx);
        }
    }

    /// Own one `(session_id, image_id)` image stream for its whole animated lifetime.
    ///
    /// Replaces the old "spawn a one-shot fetch per generation" scheme, which paid a fresh
    /// `connect()` every frame and only started the next generation after a main-thread round-trip.
    /// This worker mirrors [`Self::start_snapshot_watching`]: it keeps a reusable
    /// [`DaemonConnection`] across generations, and each round pulls whatever generation of
    /// `image_id` the current snapshot references — so intermediate generations produced while a
    /// transfer was in flight are skipped automatically rather than fetched in order.
    ///
    /// Exit and the `terminal_images_in_flight` removal happen inside the same `update` (one app
    /// lock), so a generation arriving after the worker exits is never lost: the snapshot's frame
    /// path re-runs [`Self::schedule_terminal_images`], finds the stream key absent, and re-spawns.
    pub(super) fn start_terminal_image_stream(
        &self,
        session_id: SessionId,
        image_id: u32,
        cx: &mut Context<Self>,
    ) {
        let stream_key = TerminalImageStreamKey {
            session_id,
            image_id,
        };
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection: Option<DaemonConnection> = None;
            // Read the newest un-cached descriptor for this stream. Returns `None` (and clears the
            // alive marker under the same lock) when there is nothing left to fetch, ending the
            // worker.
            while let Some(descriptor) = this
                .update(cx, |app, _| {
                    let descriptor = app
                        .is_visible_active_session(session_id)
                        .then(|| {
                            app.session_feed.snapshots.get(&session_id).and_then(|snapshot| {
                                snapshot
                                    .images
                                    .iter()
                                    .find(|descriptor| descriptor.key.id == image_id)
                                    .cloned()
                            })
                        })
                        .flatten()
                        .filter(|descriptor| {
                            !app.terminal_renderer.has_image(session_id, descriptor.key)
                        });
                    if descriptor.is_none() {
                        app.terminal_images_in_flight.remove(&stream_key);
                    }
                    descriptor
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
                        let result =
                            fetch_terminal_image_chunked(&mut connected, session_id, &descriptor);
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
                    Ok(image) => {
                        // The descriptor can vanish between the fetch and now (fast animation).
                        // Only install pixels the current snapshot still references.
                        let still_live = app.session_feed.snapshots.get(&session_id).is_some_and(|snapshot| {
                            snapshot
                                .images
                                .iter()
                                .any(|descriptor| descriptor.key == image.key.image)
                        });
                        if still_live {
                            app.terminal_renderer.install_image(image);
                        }
                        app.schedule_terminal_frame(window, cx);
                    }
                    Err(error) => {
                        eprintln!(
                            "failed to fetch terminal image {image_id} of session {session_id}: {error:#}"
                        );
                    }
                });
                if update.is_err() {
                    eprintln!("terminal image update lost its window");
                    break;
                }
                if request_failed {
                    // Connection was dropped above; back off before reconnecting. If the image is
                    // simply gone, the next loop head reads no descriptor and exits cleanly.
                    executor.timer(Duration::from_millis(25)).await;
                }
            }
        })
        .detach();
    }

    /// Coalesce terminal revisions and image arrivals onto the platform frame boundary.
    ///
    /// The daemon can publish faster than the display. Invalidating immediately for every IPC
    /// response makes GPUI start redundant draws and can deliver another revision while the
    /// previous frame is still being encoded. Keep applying deltas so the newest state is ready,
    /// but invalidate the view at most once per animation frame.
    pub(super) fn schedule_terminal_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_frame_scheduled {
            return;
        }
        self.terminal_frame_scheduled = true;
        cx.on_next_frame(window, |app, _window, cx| {
            app.terminal_frame_scheduled = false;
            let visible_snapshots = app
                .visible_active_session_ids()
                .into_iter()
                .filter_map(|session_id| app.session_feed.snapshots.get(&session_id).cloned())
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

    pub(super) fn start_latency_reporting(&self, cx: &mut Context<Self>) {
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

    pub(super) fn ensure_snapshot_watchers(&mut self, cx: &mut Context<Self>) {
        for session_id in self.visible_active_session_ids() {
            if let Some(snapshot) = self.session_feed.snapshots.get(&session_id).cloned() {
                self.schedule_terminal_images(&snapshot, cx);
            }
            if self.session_feed.snapshot_watchers.insert(session_id) {
                self.start_snapshot_watching(session_id, cx);
            }
        }
    }

    pub(super) fn start_snapshot_watching(&self, session_id: SessionId, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let mut connection = None;
            while let Some(after_revision) = this
                .update(cx, |app, _| {
                    app.is_visible_active_session(session_id).then(|| {
                        app.session_feed
                            .snapshots
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
                                .session_feed
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
                                app.session_feed
                                    .snapshots
                                    .insert(snapshot.session_id, Arc::clone(&snapshot));
                                changed = true;
                            }
                        }
                        Ok(DaemonResponse::SnapshotDelta { delta })
                            if delta.session_id == session_id =>
                        {
                            app.poll_error.take();
                            let snapshot = app
                                .session_feed
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
                                app.session_feed
                                    .snapshots
                                    .insert(snapshot.session_id, Arc::clone(&snapshot));
                                changed = true;
                            } else {
                                // Dropping the stale base makes the next request ask for a full
                                // frame instead of retrying an inapplicable delta indefinitely.
                                app.session_feed.snapshots.remove(&delta.session_id);
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
                app.session_feed.snapshot_watchers.remove(&session_id);
                // The session can become visible again between the loop's visibility check and
                // cleanup. Reconcile here so that transition cannot leave it without a watcher.
                if app.is_visible_active_session(session_id) {
                    app.ensure_snapshot_watchers(cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn start_session_polling(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let request_client = client.clone();
                let result = executor
                    .spawn(async move { request_client.request(ClientRequest::ListSessions) })
                    .await;
                let update = this.update_in(cx, |app, window, cx| match result {
                    Ok(DaemonResponse::Sessions { sessions }) => {
                        app.apply_session_list(sessions, cx);
                        app.ensure_progress_animation(window, cx);
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
    pub(super) fn apply_session_list(
        &mut self,
        sessions: Vec<SessionSummary>,
        cx: &mut Context<Self>,
    ) {
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
            .session_feed
            .sessions
            .iter()
            .filter(|session| !owned_ids.contains(&session.id))
            .map(|session| session.id)
            .collect();
        for session_id in vanished {
            if self
                .sidebar
                .sidebar_preview
                .as_ref()
                .map(|preview| preview.session_id)
                == Some(session_id)
            {
                self.dismiss_sidebar_preview(cx);
            }
            changed |= self.prune_session_from_layouts(session_id);
        }

        if self.session_feed.sessions != owned {
            self.session_feed.sessions = owned;
            changed = true;
        }
        if self.session_feed.detached_sessions != detached {
            self.session_feed.detached_sessions = detached;
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
    pub(super) fn prune_session_from_layouts(&mut self, session_id: SessionId) -> bool {
        let mut changed = false;
        // Collect (project_id, group_id, item_id) for every tab of this session across all views.
        let targets: Vec<(ProjectId, GroupId, ItemId)> = self
            .workspace_state
            .project_views
            .iter()
            .filter_map(|(project_id, view)| {
                view.layout
                    .find_terminal_item(session_id)
                    .map(|(group_id, item_id)| (*project_id, group_id, item_id))
            })
            .collect();
        for (project_id, group_id, item_id) in targets {
            if let Some(view) = self.workspace_state.project_views.get_mut(&project_id) {
                view.layout.close_item(group_id, item_id);
                self.sidebar
                    .marquee_states
                    .borrow_mut()
                    .remove(&format!("tab-title-{item_id}"));
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
            self.session_feed.snapshots.remove(&session_id);
            self.terminal_sizes.remove(&session_id);
            self.terminal_resize_in_flight.remove(&session_id);
            self.terminal_ime_states.remove(&session_id);
            self.session_feed.session_inspections.remove(&session_id);
            self.session_feed
                .session_inspection_errors
                .remove(&session_id);
            self.sidebar
                .tab_icon_states
                .borrow_mut()
                .remove(&session_id);
            self.sidebar
                .sidebar_status_rows
                .borrow_mut()
                .remove(&session_id);
            self.sidebar
                .sidebar_row_bounds
                .borrow_mut()
                .remove(&session_id);
            self.sidebar
                .marquee_states
                .borrow_mut()
                .remove(&format!("sidebar-title-{session_id}"));
            self.sidebar
                .marquee_states
                .borrow_mut()
                .remove(&format!("sidebar-subtitle-{session_id}"));
            self.session_feed.shell_integration.remove(&session_id);
            self.notification_routes
                .borrow_mut()
                .retain(|_, route| route.session_id != session_id);
            self.reconcile_layout_caches();
        }
        changed
    }

    pub(super) fn start_inspection_polling(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                // A dead weak entity (window closed) must end the loop; `Ok(None)` only means there
                // is no active session right now, so keep polling. Folding both into `None` here
                // would make the `break` below unreachable and leak this 1Hz task forever.
                let session_id = match this.update(cx, |app, _| app.active_session_id()) {
                    Ok(session_id) => session_id,
                    Err(_) => break,
                };
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
                            let error_cleared = app
                                .session_feed
                                .session_inspection_errors
                                .remove(&session_id)
                                .is_some();
                            let changed = error_cleared
                                || app.session_feed.session_inspections.get(&session_id)
                                    != Some(&inspection);
                            if changed {
                                app.session_feed
                                    .session_inspections
                                    .insert(session_id, inspection);
                                cx.notify();
                            }
                        }
                        Ok(response) => {
                            let message =
                                format!("unexpected session-inspection response: {response:?}");
                            eprintln!("{message}");
                            app.session_feed
                                .session_inspection_errors
                                .insert(session_id, message);
                            cx.notify();
                        }
                        Err(error) => {
                            let message = format!("session inspection failed: {error:#}");
                            eprintln!("{message}");
                            app.session_feed
                                .session_inspection_errors
                                .insert(session_id, message);
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

    /// Poll the daemon's OSC 133 shell-integration state for the active session, but only while the
    /// Commands tab is visible (the sidebar could be collapsed or on another tab). Mirrors the
    /// `start_inspection_polling` template; caches into `shell_integration`.
    pub(super) fn start_shell_integration_polling(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                // `Err` means the window is gone — stop. `Ok(None)` means the Commands tab is not
                // currently visible (or no active session); keep polling so the task resumes when
                // the user switches back. Collapsing both into `None` would strand this task
                // spinning once per second for the window's whole life.
                let target = match this.update(cx, |app, _| {
                    if app.right_sidebar_collapsed || app.right_tab != RightTab::Commands {
                        None
                    } else {
                        app.active_session_id()
                    }
                }) {
                    Ok(target) => target,
                    Err(_) => break,
                };
                if let Some(session_id) = target {
                    let request_client = client.clone();
                    let result = executor
                        .spawn(async move {
                            request_client
                                .request(ClientRequest::GetShellIntegrationState { session_id })
                        })
                        .await;
                    let update = this.update(cx, |app, cx| match result {
                        Ok(DaemonResponse::ShellIntegrationState { state, .. }) => {
                            let changed =
                                app.session_feed.shell_integration.get(&session_id) != Some(&state);
                            if changed {
                                app.session_feed.shell_integration.insert(session_id, state);
                                cx.notify();
                            }
                        }
                        Ok(response) => {
                            eprintln!("unexpected shell-integration response: {response:?}");
                        }
                        Err(error) => {
                            eprintln!("shell-integration request failed: {error:#}");
                        }
                    });
                    if update.is_err() {
                        break;
                    }
                }
                executor.timer(SESSION_INSPECTION_INTERVAL).await;
            }
        })
        .detach();
    }

    pub(super) fn fill_missing_process_metrics(&mut self, inspection: &mut SessionInspection) {
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

        self.session_feed
            .process_metrics_system
            .refresh_processes_specifics(
                ProcessesToUpdate::Some(&pids),
                ProcessRefreshKind::new().with_cpu().with_memory(),
            );
        for process in &mut inspection.processes {
            let Some(sample) = self
                .session_feed
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

    // Snapshot this window's workspace (project layouts, active project, sidebar states) to disk
    // so the next cold start can restore it. Registered on `on_app_quit` for every window;
    // best-effort, errors are logged inside `save`. v1 persists a single snapshot — with multiple
    // windows open the last writer wins and one window is restored.
}

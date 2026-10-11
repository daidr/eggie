use super::*;

impl EggieApp {
    pub(super) fn resize_terminal(
        &mut self,
        session_id: SessionId,
        size: TerminalSize,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_sizes.get(&session_id) == Some(&size) {
            return;
        }
        self.terminal_sizes.insert(session_id, size);
        self.send_pending_terminal_resize(session_id, cx);
    }

    pub(super) fn send_pending_terminal_resize(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_resize_in_flight.contains_key(&session_id) {
            return;
        }
        let Some(size) = self.terminal_sizes.get(&session_id).copied() else {
            return;
        };
        self.terminal_resize_in_flight.insert(session_id, size);
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = executor
                .spawn(async move {
                    client
                        .request(ClientRequest::Resize { session_id, size })
                        .map(|_| ())
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |app, cx| {
                app.finish_terminal_resize(session_id, size, result, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn finish_terminal_resize(
        &mut self,
        session_id: SessionId,
        sent_size: TerminalSize,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_resize_in_flight.get(&session_id) != Some(&sent_size) {
            return;
        }
        self.terminal_resize_in_flight.remove(&session_id);
        if let Err(error) = result {
            // A resize can fail because the PTY has already closed (the process exited but the
            // tab still renders its last snapshot). Keep the last-sent size recorded instead of
            // clearing it: resize_terminal's "same size" early-return then suppresses the
            // identical resend the per-frame prepaint would otherwise issue, so a dead session
            // no longer spins the resize path every frame. A different viewport size still retries.
            eprintln!("failed to resize terminal {session_id}: {error}");
            return;
        }
        if self.terminal_sizes.get(&session_id) != Some(&sent_size) {
            self.send_pending_terminal_resize(session_id, cx);
        }
    }

    pub(super) fn record_terminal_viewport(
        &mut self,
        group_id: GroupId,
        session_id: SessionId,
        bounds: Bounds<Pixels>,
        cell_width: f32,
        line_height: f32,
        size: TerminalSize,
    ) {
        self.terminal_viewports.insert(
            group_id,
            TerminalViewport {
                session_id,
                bounds,
                cell_width,
                line_height,
                rows: size.rows,
                columns: size.columns,
            },
        );
    }

    pub(super) fn terminal_mouse_down(
        &mut self,
        group_id: GroupId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_completion();
        self.focus_group(group_id, window, cx);
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        // Cmd/Ctrl+click opens a link. Explicit OSC 8 hyperlinks take priority; a bare
        // auto-detected URL under the cursor is the fallback.
        let hyperlink = self
            .session_feed
            .snapshots
            .get(&viewport.session_id)
            .and_then(|snapshot| {
                terminal_hyperlink_at(snapshot, point)
                    .or_else(|| detected_link_at(snapshot, point).map(|link| link.url.clone()))
            });
        if event.button == MouseButton::Left
            && event.modifiers.platform
            && let Some(url) = hyperlink
        {
            self.hyperlink_mouse_down = true;
            self.terminal_selection_drag = None;
            if url_has_safe_external_scheme(&url) {
                cx.open_url(&url);
            } else {
                let language = self.language;
                let detail = format!("{}\n{url}", language.hyperlink_open_detail());
                let prompt = window.prompt(
                    PromptLevel::Warning,
                    language.open_hyperlink_title(),
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
            cx.stop_propagation();
            return;
        }
        let Some(button) = protocol_mouse_button(event.button) else {
            return;
        };
        let modifiers = protocol_terminal_modifiers(event.modifiers);
        if let Err(error) = self.input_sender.send_mouse(
            viewport.session_id,
            TerminalMouseEvent {
                action: TerminalMouseAction::Press,
                button: Some(button),
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers,
            },
        ) {
            eprintln!("failed to enqueue terminal mouse press: {error:#}");
        }

        let Some(snapshot) = self.session_feed.snapshots.get(&viewport.session_id) else {
            return;
        };
        let captured = snapshot.input_modes.captures_mouse() && !event.modifiers.shift;
        if captured {
            self.terminal_selection_drag = None;
            let _ = self.input_sender.send_selection_clear(viewport.session_id);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            cx.stop_propagation();
            return;
        }

        let side = terminal_selection_side(viewport, event.position);
        // The daemon owns the authoritative selection (a terminal-core `Selection` spanning
        // scrollback). We only forward the viewport point + intent; the daemon expands word/line
        // selections and keeps the anchor valid across scroll. `preserve_without_drag` records
        // whether a selection should survive a click that never becomes a drag.
        let preserve_without_drag = match event.click_count {
            0 | 1 if event.modifiers.shift => {
                // Extend the existing selection's head to the clicked cell.
                let _ = self.input_sender.send_selection_update(
                    viewport.session_id,
                    cell_position(point),
                    side,
                );
                true
            }
            0 | 1 => {
                let _ = self.input_sender.send_selection_start(
                    viewport.session_id,
                    cell_position(point),
                    side,
                    TerminalSelectionKind::Simple,
                );
                false
            }
            2 => {
                let _ = self.input_sender.send_selection_start(
                    viewport.session_id,
                    cell_position(point),
                    side,
                    TerminalSelectionKind::Semantic,
                );
                true
            }
            _ => {
                let _ = self.input_sender.send_selection_start(
                    viewport.session_id,
                    cell_position(point),
                    side,
                    TerminalSelectionKind::Lines,
                );
                true
            }
        };
        self.terminal_selection_drag = Some(TerminalSelectionDrag {
            group_id,
            session_id: viewport.session_id,
            dragged: preserve_without_drag,
        });
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn terminal_mouse_up(
        &mut self,
        group_id: GroupId,
        event: &MouseUpEvent,
        cx: &mut Context<Self>,
    ) {
        if self.hyperlink_mouse_down {
            self.hyperlink_mouse_down = false;
            cx.stop_propagation();
            return;
        }
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        let Some(button) = protocol_mouse_button(event.button) else {
            return;
        };
        if let Err(error) = self.input_sender.send_mouse(
            viewport.session_id,
            TerminalMouseEvent {
                action: TerminalMouseAction::Release,
                button: Some(button),
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers: protocol_terminal_modifiers(event.modifiers),
            },
        ) {
            eprintln!("failed to enqueue terminal mouse release: {error:#}");
        }
        if self
            .session_feed
            .snapshots
            .get(&viewport.session_id)
            .is_some_and(|snapshot| snapshot.input_modes.captures_mouse())
            && !event.modifiers.shift
        {
            cx.stop_propagation();
        }
    }

    pub(super) fn terminal_mouse_move(
        &mut self,
        group_id: GroupId,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        // Update the hovered-URL state first, before the mouse-tracking early-returns, so the hover
        // underline and pointer cursor work even for apps that consume mouse motion.
        self.update_terminal_hovered_link(viewport.session_id, point, cx);
        let Some(tracking) = self
            .session_feed
            .snapshots
            .get(&viewport.session_id)
            .map(|snapshot| snapshot.input_modes.mouse_tracking)
        else {
            return;
        };
        let button = event.pressed_button.and_then(protocol_mouse_button);
        let should_report = match tracking {
            TerminalMouseTracking::Disabled | TerminalMouseTracking::Click => false,
            TerminalMouseTracking::Drag => button.is_some(),
            TerminalMouseTracking::Motion => true,
        };
        if !should_report
            || (event.modifiers.shift
                && matches!(
                    button,
                    Some(TerminalMouseButton::Left | TerminalMouseButton::Right)
                ))
        {
            return;
        }
        if let Err(error) = self.input_sender.send_mouse(
            viewport.session_id,
            TerminalMouseEvent {
                action: TerminalMouseAction::Move,
                button,
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers: protocol_terminal_modifiers(event.modifiers),
            },
        ) {
            eprintln!("failed to enqueue terminal mouse motion: {error:#}");
        }
    }

    /// Recompute which auto-detected URL (if any) is under `point` and update hover state. Only
    /// notifies when the hovered link actually changes, to avoid a rerender on every mouse move.
    pub(super) fn update_terminal_hovered_link(
        &mut self,
        session_id: SessionId,
        point: TerminalPoint,
        cx: &mut Context<Self>,
    ) {
        let hovered = self
            .session_feed
            .snapshots
            .get(&session_id)
            .and_then(|snapshot| detected_link_at(snapshot, point))
            .map(|link| (session_id, link.clone()));
        if self.terminal_hovered_link != hovered {
            self.terminal_hovered_link = hovered;
            cx.notify();
        }
    }

    pub(super) fn terminal_scroll(
        &mut self,
        group_id: GroupId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(viewport) = self.terminal_viewports.get(&group_id).copied() else {
            return;
        };
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        let (x, y, unit) = match event.delta {
            ScrollDelta::Pixels(delta) => (
                f32::from(delta.x),
                f32::from(delta.y),
                TerminalScrollUnit::Pixels,
            ),
            ScrollDelta::Lines(delta) => (delta.x, delta.y, TerminalScrollUnit::Lines),
        };
        let phase = match event.touch_phase {
            TouchPhase::Started => TerminalScrollPhase::Started,
            TouchPhase::Moved => TerminalScrollPhase::Moved,
            TouchPhase::Ended | TouchPhase::Cancelled => TerminalScrollPhase::Ended,
        };
        if let Err(error) = self.input_sender.send_scroll(
            viewport.session_id,
            TerminalScrollEvent {
                delta: TerminalScrollDelta {
                    x: fixed_terminal_scroll_delta(x),
                    y: fixed_terminal_scroll_delta(y),
                    unit,
                },
                phase,
                position: protocol_mouse_position(point, viewport, event.position),
                modifiers: protocol_terminal_modifiers(event.modifiers),
            },
        ) {
            eprintln!("failed to enqueue terminal scroll: {error:#}");
        }
        cx.stop_propagation();
    }

    pub(super) fn update_terminal_selection_drag(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut drag) = self.terminal_selection_drag else {
            return;
        };
        if !event.dragging() {
            return;
        }
        let Some(viewport) = self.terminal_viewports.get(&drag.group_id).copied() else {
            return;
        };
        // Auto-scroll when the pointer is dragged above or below the viewport, so a selection can be
        // extended into scrollback. The daemon scrolls the grid and the terminal core keeps the
        // anchor valid; the next snapshot reprojects the selection over the new viewport.
        let top = f32::from(viewport.bounds.origin.y);
        let bottom = top + f32::from(viewport.bounds.size.height);
        let pointer_y = f32::from(event.position.y);
        if viewport.line_height > 0. {
            let lines = if pointer_y < top {
                ((top - pointer_y) / viewport.line_height).ceil() as i32
            } else if pointer_y > bottom {
                -(((pointer_y - bottom) / viewport.line_height).ceil() as i32)
            } else {
                0
            };
            if lines != 0 {
                let clamped_point = terminal_point_from_position(viewport, event.position)
                    .unwrap_or(TerminalPoint { line: 0, column: 0 });
                let _ = self.input_sender.send_scroll(
                    drag.session_id,
                    TerminalScrollEvent {
                        delta: TerminalScrollDelta {
                            x: 0,
                            y: lines.saturating_mul(TERMINAL_SCROLL_DELTA_SCALE),
                            unit: TerminalScrollUnit::Lines,
                        },
                        phase: TerminalScrollPhase::Moved,
                        position: protocol_mouse_position(clamped_point, viewport, event.position),
                        modifiers: protocol_terminal_modifiers(event.modifiers),
                    },
                );
            }
        }
        let Some(point) = terminal_point_from_position(viewport, event.position) else {
            return;
        };
        let side = terminal_selection_side(viewport, event.position);
        let _ =
            self.input_sender
                .send_selection_update(drag.session_id, cell_position(point), side);
        drag.dragged = true;
        self.terminal_selection_drag = Some(drag);
        cx.notify();
    }

    pub(super) fn finish_terminal_selection_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.terminal_selection_drag.take() else {
            return;
        };
        if !drag.dragged {
            let _ = self.input_sender.send_selection_clear(drag.session_id);
        } else if self.copy_on_select {
            // The user dragged out a real selection; mirror it to the system clipboard immediately.
            // Uses the drag's own session (not the active one) so a selection in a background split
            // still copies correctly.
            self.copy_terminal_selection(drag.session_id, cx);
        }
        cx.notify();
    }

    pub(super) fn copy_active_terminal_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        self.copy_terminal_selection(session_id, cx)
    }

    pub(super) fn copy_terminal_selection(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> bool {
        // The daemon owns the selection and extracts its text from the terminal core (whole
        // scrollback span, soft-wrap unwrapped, trailing whitespace trimmed).
        let text = match self
            .client
            .request(ClientRequest::TerminalCopySelection { session_id })
        {
            Ok(DaemonResponse::SelectionText { text, .. }) => text,
            _ => return false,
        };
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    pub(super) fn paste_into_active_terminal(&mut self, cx: &mut Context<Self>) -> bool {
        self.dismiss_completion();
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let paste_events = self
            .session_feed
            .snapshots
            .get(&session_id)
            .is_some_and(|snapshot| snapshot.input_modes.paste_events);
        let enqueued = if paste_events {
            let contents = clipboard_contents_from_item(item, &["*/*".to_owned()]);
            self.enqueue_terminal_clipboard_paste(
                session_id,
                TerminalClipboardSelection::Clipboard,
                contents,
            )
        } else {
            let Some(text) = item.text() else {
                return false;
            };
            self.enqueue_terminal_paste(session_id, text)
        };
        if !enqueued {
            return false;
        }
        let _ = self.input_sender.send_selection_clear(session_id);
        let ime_changed = self.terminal_ime_states.remove(&session_id).is_some();
        if ime_changed {
            cx.notify();
        }
        true
    }

    pub(super) fn next_input_sequence() -> u64 {
        NEXT_INPUT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn enqueue_terminal_input(&self, session_id: SessionId, bytes: Vec<u8>) -> bool {
        let sequence = Self::next_input_sequence();
        self.input_latency.record_input(session_id, sequence);
        if let Err(error) = self.input_sender.send_input(session_id, bytes, sequence) {
            self.input_latency.discard_input(session_id, sequence);
            eprintln!("failed to enqueue terminal input: {error:#}");
            return false;
        }
        true
    }

    pub(super) fn enqueue_terminal_paste(&self, session_id: SessionId, text: String) -> bool {
        let sequence = Self::next_input_sequence();
        self.input_latency.record_input(session_id, sequence);
        if let Err(error) = self.input_sender.send_paste(session_id, text, sequence) {
            self.input_latency.discard_input(session_id, sequence);
            eprintln!("failed to enqueue terminal paste: {error:#}");
            return false;
        }
        true
    }

    /// Inject a snippet into the terminal. The content is pasted verbatim (bracketed-paste safe,
    /// newlines kept as written). When `auto_run` is set, a real carriage return (`\r`) key is sent
    /// AFTER the paste to execute it — inside a bracketed paste the shell would otherwise insert a
    /// newline literally instead of running. Whether to run is an explicit per-snippet flag
    /// (the editor's auto-run checkbox), not inferred from a trailing newline.
    pub(super) fn run_snippet_content(
        &self,
        session_id: SessionId,
        content: &str,
        auto_run: bool,
    ) -> bool {
        if !content.is_empty() && !self.enqueue_terminal_paste(session_id, content.to_owned()) {
            return false;
        }
        if auto_run {
            return self.enqueue_terminal_input(session_id, b"\r".to_vec());
        }
        true
    }
    pub(super) fn enqueue_terminal_clipboard_paste(
        &self,
        session_id: SessionId,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
    ) -> bool {
        let sequence = Self::next_input_sequence();
        self.input_latency.record_input(session_id, sequence);
        if let Err(error) = self
            .input_sender
            .send_paste_clipboard(session_id, selection, contents, sequence)
        {
            self.input_latency.discard_input(session_id, sequence);
            eprintln!("failed to enqueue terminal rich paste: {error:#}");
            return false;
        }
        true
    }

    pub(super) fn select_all_active_terminal(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        // The daemon selects the whole buffer (including scrollback) in the terminal core and
        // republishes; the projected selection arrives with the next snapshot.
        self.client
            .request(ClientRequest::TerminalSelectAll { session_id })
            .is_ok()
    }

    // --- Keybinding action handlers ----------------------------------------------------------
    //
    // These are thin wrappers over existing capabilities, invoked by the configurable keymap.
    // Each returns whether it acted so the caller can stop action propagation.

    pub(crate) fn terminal_ime_state(&self, session_id: SessionId) -> Option<&TerminalImeState> {
        self.terminal_ime_states.get(&session_id)
    }

    pub(crate) fn set_terminal_marked_text(
        &mut self,
        session_id: SessionId,
        text: &str,
        selected_range: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        let length = text.encode_utf16().count();
        if text.is_empty() {
            self.terminal_ime_states.remove(&session_id);
        } else {
            self.dismiss_completion();
            let selected_range = selected_range.unwrap_or(length..length);
            self.terminal_ime_states.insert(
                session_id,
                TerminalImeState {
                    text: text.to_owned(),
                    selected_range: selected_range.start.min(length)
                        ..selected_range.end.min(length),
                },
            );
        }
        cx.notify();
    }

    pub(crate) fn clear_terminal_marked_text(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_ime_states.remove(&session_id).is_some() {
            cx.notify();
        }
    }

    pub(crate) fn commit_terminal_text(
        &mut self,
        session_id: SessionId,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.terminal_ime_states.remove(&session_id);
        let _ = self.input_sender.send_selection_clear(session_id);
        if !text.is_empty() {
            self.completion.wants = true;
            self.enqueue_terminal_input(session_id, terminal_text_bytes(text));
        }
        cx.notify();
    }
}

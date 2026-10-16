use super::*;

impl EggieApp {
    pub(super) fn open_terminal_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        match &self.terminal_search {
            Some(search) if search.session_id == session_id => {}
            _ => {
                let colors = self.colors;
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(
                        window,
                        cx,
                        TextInputStyle {
                            text_color: (colors.text << 8) | 0xff,
                            placeholder_color: (colors.muted << 8) | 0xff,
                            cursor_color: (colors.accent << 8) | 0xff,
                            selection_color: (colors.accent << 8) | 0x55,
                        },
                    );
                    input.set_placeholder(self.language.search_placeholder());
                    // Seed the edit-menu language here rather than relying on `render` to patch it
                    // next frame: `render` now only pushes language on a config change, so a search
                    // opened while the config is stable would otherwise keep the default language.
                    input.set_language(self.language);
                    input
                });
                let subscription = cx.subscribe_in(&input, window, Self::on_search_input_event);
                self.terminal_search = Some(TerminalSearchUi {
                    session_id,
                    input,
                    regex: false,
                    result: None,
                    generation: 0,
                    pending: Default::default(),
                    task: None,
                    _subscription: subscription,
                });
            }
        }
        if let Some(search) = &self.terminal_search {
            let handle = search.input.read(cx).focus_handle();
            handle.focus(window, cx);
        }
        cx.notify();
        true
    }

    /// Route events emitted by the search input to search behavior.
    pub(super) fn on_search_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                self.run_terminal_search(TerminalSearchDirection::Forward, true, cx)
            }
            TextInputEvent::Confirm => {
                self.navigate_terminal_search(TerminalSearchDirection::Forward, cx)
            }
            TextInputEvent::ConfirmReverse => {
                self.navigate_terminal_search(TerminalSearchDirection::Backward, cx)
            }
            TextInputEvent::Cancel => {
                // Close and return focus to the terminal (same path as the X button), so keyboard
                // input keeps working instead of being dropped when the input entity stops rendering.
                self.close_terminal_search(window, cx);
            }
        }
    }

    /// Close the search bar, drop its highlights, and return focus to the terminal.
    pub(super) fn close_terminal_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal_search.take().is_none() {
            return false;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
        true
    }

    /// The current query text, or empty if the search bar is closed.
    pub(super) fn terminal_search_query(&self, cx: &App) -> String {
        self.terminal_search
            .as_ref()
            .map(|search| search.input.read(cx).content().to_owned())
            .unwrap_or_default()
    }

    /// Send the current query to the daemon and apply the result (highlights + counter). `fresh`
    /// restarts the search from the viewport (used when the query text changes); otherwise the
    /// daemon advances from the previous active match in `direction`.
    pub(super) fn run_terminal_search(
        &mut self,
        direction: TerminalSearchDirection,
        fresh: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = &mut self.terminal_search else {
            return;
        };
        let request = TerminalSearchRequest {
            query: search.input.read(cx).content().to_owned(),
            regex: search.regex,
            direction,
            fresh,
        };
        if fresh {
            search.generation = search.generation.wrapping_add(1);
            search.pending.clear();
            search.result = None;
        }
        search.pending.push_back((search.generation, request));
        self.start_next_terminal_search(cx);
        cx.notify();
    }

    fn start_next_terminal_search(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.terminal_search else {
            return;
        };
        if search.task.is_some() {
            return;
        }
        let Some((generation, request)) = search.pending.pop_front() else {
            return;
        };
        let session_id = search.session_id;
        let input_id = search.input.entity_id();
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        search.task = Some(cx.spawn(async move |this, cx| {
            if request.fresh {
                executor.timer(Duration::from_millis(16)).await;
            }
            let current = this
                .update(cx, |app, _| {
                    app.terminal_search.as_ref().is_some_and(|search| {
                        search.input.entity_id() == input_id && search.generation == generation
                    })
                })
                .unwrap_or(false);
            let result = if current {
                Some(
                    executor
                        .spawn(async move {
                            let result = match client.request_with_timeout(
                                ClientRequest::TerminalSearch {
                                    session_id,
                                    request,
                                },
                                Duration::from_secs(5),
                            )? {
                                DaemonResponse::SearchResult { result, .. } => result,
                                response => {
                                    anyhow::bail!("unexpected search response: {response:?}")
                                }
                            };
                            let snapshot = match client.request_with_timeout(
                                ClientRequest::Snapshot { session_id },
                                Duration::from_secs(5),
                            )? {
                                DaemonResponse::Snapshot { snapshot } => snapshot,
                                response => {
                                    anyhow::bail!("unexpected snapshot response: {response:?}")
                                }
                            };
                            anyhow::Ok((result, snapshot))
                        })
                        .await,
                )
            } else {
                None
            };
            let _ = this.update(cx, |app, cx| {
                let Some(search) = &mut app.terminal_search else {
                    return;
                };
                if search.input.entity_id() != input_id {
                    return;
                }
                search.task = None;
                if search.generation == generation
                    && let Some(result) = result
                {
                    match result {
                        Ok((result, snapshot)) => {
                            let held_revision = app
                                .session_feed
                                .snapshots
                                .get(&session_id)
                                .map(|snapshot| snapshot.revision)
                                .unwrap_or(0);
                            if snapshot.revision >= held_revision {
                                app.session_feed.snapshots.insert(session_id, snapshot);
                            }
                            search.result = Some(result);
                        }
                        Err(error) => {
                            app.poll_error = Some(format!("terminal search failed: {error:#}"))
                        }
                    }
                }
                app.start_next_terminal_search(cx);
                cx.notify();
            });
        }));
    }

    /// Toggle regex interpretation of the query and re-run the search.
    pub(super) fn toggle_terminal_search_regex(&mut self, cx: &mut Context<Self>) {
        let Some(search) = &mut self.terminal_search else {
            return;
        };
        search.regex = !search.regex;
        self.run_terminal_search(TerminalSearchDirection::Forward, true, cx);
    }

    /// Advance to the next (Enter) or previous (Shift+Enter) match.
    pub(super) fn navigate_terminal_search(
        &mut self,
        direction: TerminalSearchDirection,
        cx: &mut Context<Self>,
    ) {
        if self.terminal_search_query(cx).is_empty() {
            return;
        }
        self.run_terminal_search(direction, false, cx);
    }

    /// Viewport-relative search highlights for `session_id`, if the search bar targets it.
    pub(super) fn terminal_search_matches(
        &self,
        session_id: SessionId,
    ) -> Option<&TerminalSearchResult> {
        let search = self.terminal_search.as_ref()?;
        if search.session_id != session_id {
            return None;
        }
        search.result.as_ref().filter(|result| {
            self.session_feed
                .snapshots
                .get(&session_id)
                .is_some_and(|snapshot| snapshot.revision == result.revision)
        })
    }

    /// Render the floating search bar overlaid on the top-right of the terminal content region.
    pub(super) fn render_terminal_search_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.terminal_search.as_ref()?;
        let colors = self.colors;
        let query_is_empty = search.input.read(cx).content().is_empty();
        let regex = search.regex;
        let (index, total) = search
            .result
            .as_ref()
            .map(|result| (result.index, result.total))
            .unwrap_or((0, 0));

        // "3/12" when there are matches, "0/0" for a query with no hits, blank for an empty query.
        let counter_text = if query_is_empty {
            String::new()
        } else if total == 0 {
            "0/0".to_owned()
        } else {
            format!("{}/{}", index + 1, total)
        };
        let no_matches = !query_is_empty && total == 0;

        let bar =
            terminal_overlay_surface(format!("terminal-search-{}", search.session_id), colors)
                .absolute()
                .top(px(8.))
                .right(px(12.))
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(34.))
                // Ideal width, but never wider than the container so a narrow split can't overflow it.
                .w(px(340.))
                .max_w(relative(0.9))
                .px_2()
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(colors.muted))
                        .child(icon(IconName::Search)),
                )
                // The reusable text-input component owns focus, selection, caret, and editing.
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w(px(48.))
                        .h_full()
                        .text_size(px(13.))
                        .text_color(rgb(colors.text))
                        .child(search.input.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .min_w(px(38.))
                        .text_size(px(12.))
                        .text_color(rgb(if no_matches { 0xE06C75 } else { colors.muted }))
                        .child(counter_text),
                )
                // Regex toggle: `.*` highlighted when active.
                .child(
                    div()
                        .id("terminal-search-regex")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(24.))
                        .rounded_lg()
                        .text_size(px(12.))
                        .cursor_pointer()
                        .text_color(rgb(if regex { colors.accent } else { colors.muted }))
                        .when(regex, |element| element.bg(rgb(colors.panel_alt)))
                        .hover(move |element| element.bg(rgb(colors.hover)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.toggle_terminal_search_regex(cx);
                        }))
                        .child(".*"),
                )
                .child(icon_button(
                    IconName::ArrowUp,
                    "terminal-search-prev",
                    colors,
                    None,
                    cx.listener(|app, _, _, cx| {
                        cx.stop_propagation();
                        app.navigate_terminal_search(TerminalSearchDirection::Backward, cx);
                    }),
                ))
                .child(icon_button(
                    IconName::ArrowDown,
                    "terminal-search-next",
                    colors,
                    None,
                    cx.listener(|app, _, _, cx| {
                        cx.stop_propagation();
                        app.navigate_terminal_search(TerminalSearchDirection::Forward, cx);
                    }),
                ))
                .child(icon_button(
                    IconName::Close,
                    "terminal-search-close",
                    colors,
                    None,
                    cx.listener(|app, _, window, cx| {
                        cx.stop_propagation();
                        app.close_terminal_search(window, cx);
                    }),
                ));
        Some(bar.into_any_element())
    }

    // Toggle the command palette: open it (building a fresh input) if closed, close it if open.
}

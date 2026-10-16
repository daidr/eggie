use super::*;

impl EggieApp {
    pub(super) fn render_right_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let language = self.language;
        // When the active shell has exited (or there is no session), Info/Commands are disabled and
        // the content is forced to Notes; the user's right_tab choice is preserved and restored once a
        // live session is active again.
        let forced = self.notes_forced();
        let active_tab = if forced {
            RightTab::Notes
        } else {
            self.right_tab
        };
        let tabs = [
            (RightTab::Info, language.info_tab(), IconName::Info),
            (
                RightTab::Commands,
                language.commands_tab(),
                IconName::Terminal,
            ),
            (RightTab::Notes, language.notes_tab(), IconName::StickyNote),
        ]
        .into_iter()
        .fold(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(TAB_BAR_HEIGHT))
                .p(px(TAB_BAR_PADDING))
                .gap(px(TAB_GAP)),
            |tabs, (tab, title, tab_icon)| {
                // Info/Commands are disabled while notes are forced (no live shell).
                let disabled = forced && matches!(tab, RightTab::Info | RightTab::Commands);
                tabs.child(
                    div()
                        .id(format!("right-tab-{title}"))
                        .flex_1()
                        .flex()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .rounded_lg()
                        .when(disabled, |element| {
                            element.text_color(rgb(self.colors.muted)).opacity(0.5)
                        })
                        .when(!disabled, |element| {
                            element
                                .cursor_pointer()
                                .when(active_tab == tab, |element| {
                                    element
                                        .bg(rgb(self.colors.panel_alt))
                                        .text_color(rgb(self.colors.accent))
                                })
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    app.right_tab = tab;
                                    if tab != RightTab::Info {
                                        app.close_port_editor(window, cx);
                                    }
                                    cx.notify();
                                }))
                        })
                        .child(icon(tab_icon))
                        .child(title),
                )
            },
        );
        div()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.right_sidebar_width))
            .h_full()
            .bg(rgb(self.colors.panel))
            .child(tabs)
            .child(if active_tab == RightTab::Notes {
                // Notes fills the bounded region directly (NOT inside the sidebar's outer scroll),
                // so its textarea can own a real height and scroll internally.
                div()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .text_size(px(12.))
                    .child(self.render_right_content(cx))
            } else {
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("right-sidebar-scroll-content")
                            .size_full()
                            .min_h_0()
                            .overflow_y_scroll()
                            .scrollbar_width(px(SIDEBAR_SCROLLBAR_WIDTH))
                            .track_scroll(&self.right_sidebar_scroll_handle)
                            .p_4()
                            .text_size(px(12.))
                            .child(self.render_right_content(cx)),
                    )
                    .child(self.render_right_sidebar_scrollbar())
            })
            .into_any_element()
    }

    pub(super) fn render_right_sidebar_scrollbar(&self) -> AnyElement {
        let scroll_handle = self.right_sidebar_scroll_handle.clone();
        let muted = self.colors.muted;
        canvas(
            |_, _, _| (),
            move |track_bounds, _, window, _| {
                let max_offset = f32::from(scroll_handle.max_offset().y);
                if max_offset <= 0. {
                    return;
                }

                let viewport_height = f32::from(scroll_handle.bounds().size.height);
                let track_height = f32::from(track_bounds.size.height);
                let content_height = viewport_height + max_offset;
                let thumb_height = (track_height * viewport_height / content_height)
                    .max(SIDEBAR_SCROLLBAR_MIN_THUMB_HEIGHT)
                    .min(track_height);
                let progress = (-f32::from(scroll_handle.offset().y) / max_offset).clamp(0., 1.);
                let thumb_top =
                    f32::from(track_bounds.origin.y) + (track_height - thumb_height) * progress;
                let thumb_bounds = Bounds::new(
                    point(track_bounds.origin.x, px(thumb_top)),
                    size(track_bounds.size.width, px(thumb_height)),
                );
                window.paint_quad(quad(
                    thumb_bounds,
                    px(SIDEBAR_SCROLLBAR_THUMB_WIDTH / 2.),
                    rgba((muted << 8) | 0x99),
                    px(0.),
                    rgba(0x00000000),
                    Default::default(),
                ));
            },
        )
        .absolute()
        .top(px(4.))
        .right(px(2.))
        .bottom(px(4.))
        .w(px(SIDEBAR_SCROLLBAR_THUMB_WIDTH))
        .into_any_element()
    }

    pub(super) fn render_right_content(&self, cx: &mut Context<Self>) -> AnyElement {
        // Forced Notes (exited / no session) overrides the selected tab's content.
        let tab = if self.notes_forced() {
            RightTab::Notes
        } else {
            self.right_tab
        };
        match tab {
            RightTab::Info => {
                let session = self.active_session_id().and_then(|id| {
                    self.session_feed
                        .sessions
                        .iter()
                        .find(|session| session.id == id)
                });
                let mut content = div().flex().flex_col().gap_4();
                if let Some(session) = session {
                    content = content
                        .children(self.render_ssh_identity(session))
                        .when(
                            session.remote.is_none()
                                && !self
                                    .session_feed
                                    .session_inspections
                                    .get(&session.id)
                                    .is_some_and(|inspection| {
                                        inspection.processes.iter().any(|process| {
                                            process.pid == session.current_process.pid
                                        })
                                    }),
                            |element| {
                                element.child(process_summary(
                                    &session.current_process,
                                    self.colors,
                                    self.language,
                                ))
                            },
                        )
                        .when(
                            session.remote.is_none() || session.remote_directory().is_some(),
                            |element| {
                                element.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .child(info_row(
                                            self.language.current_directory(),
                                            displayed_directory(session),
                                            self.colors,
                                        ))
                                        // A remote path cannot be opened locally, so the Finder/VS Code
                                        // buttons are omitted for a remote session.
                                        .when(
                                            session.remote.is_none()
                                                && session
                                                    .reported_location
                                                    .as_ref()
                                                    .is_none_or(|location| location.local),
                                            |element| {
                                                element.child(self.render_directory_actions(
                                                    DirectoryField::Current,
                                                    &session.current_directory,
                                                    cx,
                                                ))
                                            },
                                        ),
                                )
                            },
                        )
                        .when(session.remote.is_none(), |element| {
                            element.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(self.collapsible_header(
                                        "initial-directory-details".into(),
                                        self.language.initial_directory(),
                                        !self.initial_directory_expanded,
                                        cx,
                                        |app, _| {
                                            app.initial_directory_expanded =
                                                !app.initial_directory_expanded
                                        },
                                    ))
                                    .when(self.initial_directory_expanded, |element| {
                                        element
                                            .child(div().child(
                                                session.initial_directory.display().to_string(),
                                            ))
                                            .child(self.render_directory_actions(
                                                DirectoryField::Initial,
                                                &session.initial_directory,
                                                cx,
                                            ))
                                    }),
                            )
                        })
                        .when_some(
                            session
                                .remote
                                .as_ref()
                                .and_then(|remote| remote.initial_directory.as_ref()),
                            |element, path| {
                                element.child(info_row(
                                    self.language.initial_directory(),
                                    path.clone(),
                                    self.colors,
                                ))
                            },
                        );
                    if let Some(inspection) = self.session_feed.session_inspections.get(&session.id)
                    {
                        let is_remote = inspection.port_mappings.is_some()
                            || session
                                .reported_location
                                .as_ref()
                                .is_some_and(|location| !location.local);
                        if session.remote.as_ref().is_none_or(|remote| {
                            remote.phase == eggie_protocol::RemoteSessionPhase::Connected
                        }) {
                            content = content.child(process_section(
                                &inspection.processes,
                                self.colors,
                                self.language,
                                is_remote,
                                cx,
                            ));
                        }
                        if !is_remote
                            && let Some(ports) =
                                ports_section(inspection, self.colors, self.language)
                        {
                            content = content.child(ports);
                        }
                        if let Some(connection) = self.active_port_connection(cx)
                            && (session.remote.as_ref().is_none_or(|remote| {
                                remote.phase == eggie_protocol::RemoteSessionPhase::Connected
                            }) || !connection.mappings.is_empty())
                        {
                            content = content.child(self.render_port_section(&connection, cx));
                        }
                    } else if session.remote.as_ref().is_none_or(|remote| {
                        remote.phase == eggie_protocol::RemoteSessionPhase::Connected
                    }) {
                        let status = self
                            .session_feed
                            .session_inspection_errors
                            .get(&session.id)
                            .cloned()
                            .unwrap_or_else(|| self.language.loading_process_info().to_owned());
                        content = content.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(process_section_header(self.colors, self.language))
                                .child(placeholder(status, self.colors)),
                        );
                    }
                }
                content.into_any_element()
            }
            RightTab::Commands => self.render_commands_tab(cx),
            RightTab::Notes => self.render_notes_tab(cx),
        }
    }

    // ===== Commands tab (command history + snippets) =========================================

    pub(super) fn render_commands_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_command_history_section(cx))
            .child(self.render_snippets_section(cx))
            .into_any_element()
    }

    /// A clickable collapsible section header: caret + label. Toggling flips the given predicate via
    /// the click listener the caller wires through \`on_toggle\`.
    pub(super) fn collapsible_header(
        &self,
        id: SharedString,
        label: &'static str,
        collapsed: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .text_color(rgb(self.colors.muted))
            .child(icon_sized(
                if collapsed {
                    IconName::ArrowRight
                } else {
                    IconName::ArrowDown
                },
                12.,
            ))
            .child(section_label(label, self.colors))
            .on_click(cx.listener(move |app, _, _, cx| {
                on_toggle(app, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    pub(super) fn render_command_history_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut section = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.collapsible_header(
                "command-history-header".into(),
                self.language.command_history_label(),
                self.command_history_collapsed,
                cx,
                |app, _| app.command_history_collapsed = !app.command_history_collapsed,
            ));
        if self.command_history_collapsed {
            return section.into_any_element();
        }

        let session_id = self.active_session_id();
        let state = session_id.and_then(|id| self.session_feed.shell_integration.get(&id));
        let query = self
            .command_history_search
            .read(cx)
            .content()
            .to_lowercase();
        section = section.child(
            div()
                .id("command-history-search")
                .w_full()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(self.colors.border))
                .child(self.command_history_search.clone()),
        );
        let matches = |command: &Option<String>| {
            command
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query)
        };
        match state {
            None => {
                section = section.child(placeholder(
                    self.language.shell_integration_unavailable(),
                    self.colors,
                ));
            }
            Some(state) => {
                // Newest first: current (running) command, then history reversed.
                let now_ms = unix_now_ms();
                let mut list = div().flex().flex_col().gap_1();
                let mut any = false;
                if state.phase == eggie_protocol::TerminalSemanticPhase::Output
                    && let Some(current) = &state.current_command
                    && matches(&current.command_line)
                {
                    any = true;
                    list = list.child(self.render_command_row(
                        session_id,
                        "cmd-current".into(),
                        current.id,
                        current.command_line.clone(),
                        current.exit_code,
                        current.started_at_unix_ms,
                        now_ms,
                        cx,
                    ));
                }
                for record in state.history.iter().rev() {
                    if !matches(&record.command_line) {
                        continue;
                    }
                    any = true;
                    list = list.child(self.render_command_row(
                        session_id,
                        {
                            let s: SharedString = format!("cmd-{}", record.id).into();
                            s
                        },
                        record.id,
                        record.command_line.clone(),
                        record.exit_code,
                        record.started_at_unix_ms,
                        now_ms,
                        cx,
                    ));
                }
                if any {
                    section = section.child(list);
                } else {
                    section = section.child(placeholder(
                        if query.is_empty() {
                            self.language.command_history_empty()
                        } else if self.language == Language::SimplifiedChinese {
                            "没有匹配的命令"
                        } else {
                            "No matching commands"
                        },
                        self.colors,
                    ));
                }
            }
        }
        section.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_command_row(
        &self,
        session_id: Option<SessionId>,
        id: SharedString,
        command_id: u64,
        command_line: Option<String>,
        exit_code: Option<i32>,
        started_at_unix_ms: u64,
        now_ms: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = CommandStatus::from_exit_code(exit_code);
        let (dot_color, trailing): (u32, Option<String>) = match status {
            CommandStatus::Running => (
                self.colors.muted,
                Some(self.language.command_running().to_owned()),
            ),
            CommandStatus::Success => (0x3fb950, None),
            CommandStatus::Failure(code) => (0xf85149, Some(code.to_string())),
        };
        let raw = command_line.unwrap_or_default();
        // The paste refill uses the real command (empty if the shell never reported one);
        // the display falls back to a muted placeholder so a text-less record is not a blank row.
        let paste_text = raw.clone();
        let has_text = !raw.trim().is_empty();
        // Command history rows are single-line: collapse any embedded newlines (multi-line
        // commands) to spaces so the row keeps its flex layout and truncates cleanly.
        let command_text = if has_text {
            raw.split(['\n', '\r'])
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            self.language.command_no_text().to_owned()
        };
        let relative = format_relative_time(started_at_unix_ms, now_ms);
        let colors = self.colors;
        let menu_text = paste_text.clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|element| element.bg(rgb(colors.panel_alt)))
            .child(
                div()
                    .flex_none()
                    .size(px(7.))
                    .rounded_full()
                    .bg(rgb(dot_color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family("monospace")
                    .when(!has_text, |element| element.text_color(rgb(colors.muted)))
                    .child(command_text),
            )
            .children(trailing.map(|t| {
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(rgb(dot_color))
                    .child(t)
            }))
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(rgb(colors.muted))
                    .child(relative),
            )
            .on_click(cx.listener(move |app, _, _, _| {
                // Refill only (no trailing newline appended): the command is re-entered, not run.
                if let Some(session_id) = session_id {
                    app.enqueue_terminal_paste(session_id, paste_text.clone());
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |app, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(session_id) = session_id {
                        app.show_command_output_menu(
                            session_id,
                            command_id,
                            menu_text.clone(),
                            window,
                            cx,
                        );
                    }
                }),
            )
            .into_any_element()
    }

    fn show_command_output_menu(
        &mut self,
        session_id: SessionId,
        command_id: u64,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::native_menu::{NativeCommandOutputMenuCommand, prepare_command_output_menu};
        let Some(menu) = prepare_command_output_menu(window, self.language) else {
            return;
        };
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let Some(choice) = menu.show() else {
                return;
            };
            let request = if choice == NativeCommandOutputMenuCommand::Locate {
                ClientRequest::LocateCommandOutput {
                    session_id,
                    command_id,
                }
            } else {
                ClientRequest::ReadCommandOutput {
                    session_id,
                    command_id,
                    max_bytes: 1024 * 1024,
                }
            };
            let response = executor
                .spawn(async move { client.request_with_timeout(request, Duration::from_secs(5)) })
                .await;
            let _ = this.update(cx, |app, cx| {
                match response {
                    Ok(DaemonResponse::TerminalText {
                        text, truncated, ..
                    }) => {
                        let text = if choice == NativeCommandOutputMenuCommand::CopyWithCommand {
                            format!("{command}\n{text}")
                        } else {
                            text
                        };
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        if truncated {
                            app.poll_error = Some(
                                if app.language == Language::SimplifiedChinese {
                                    "输出过长，仅复制前 1 MiB"
                                } else {
                                    "Copied the first 1 MiB of output"
                                }
                                .into(),
                            );
                        }
                    }
                    Ok(DaemonResponse::Ok) => {}
                    Ok(DaemonResponse::Error { message }) => app.poll_error = Some(message),
                    Err(error) => app.poll_error = Some(error.to_string()),
                    _ => app.poll_error = Some("Unexpected command output response".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- Snippets --------------------------------------------------------------------------

    pub(super) fn render_snippets_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .child(self.collapsible_header(
                "snippets-header".into(),
                self.language.snippets_label(),
                self.snippets_collapsed,
                cx,
                |app, _| app.snippets_collapsed = !app.snippets_collapsed,
            ))
            .child(icon_button(
                IconName::Add,
                "snippet-add",
                self.colors,
                Some(self.language.add_snippet().into()),
                cx.listener(|app, _, window, cx| app.open_snippet_editor(None, window, cx)),
            ));
        let mut section = div().flex().flex_col().gap_2().child(header);
        if self.snippets_collapsed {
            return section.into_any_element();
        }
        let snippets = self.snippet_store.read(cx).snippets().to_vec();
        if snippets.is_empty() {
            section = section.child(placeholder(self.language.snippets_empty(), self.colors));
        } else {
            let mut list = div().flex().flex_col().gap_1();
            for snippet in snippets {
                list = list.child(self.render_snippet_row(&snippet, cx));
            }
            section = section.child(list);
        }
        section.into_any_element()
    }

    pub(super) fn render_snippet_row(
        &self,
        snippet: &Snippet,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let id = snippet.id;
        let content = snippet.content.clone();
        let auto_run = snippet.auto_run;
        let short = id.simple().to_string();
        let row_id: SharedString = format!("snippet-{short}").into();
        let name = if snippet.name.trim().is_empty() {
            snippet
                .content
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned()
        } else {
            snippet.name.clone()
        };
        // Left click uses (injects/runs) the snippet; right click opens a native menu
        // (Use / Edit / — / Delete). No inline buttons — the row itself is the target.
        div()
            .id(row_id)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|element| element.bg(rgb(colors.panel_alt)))
            .child(div().flex_1().min_w_0().truncate().child(name))
            .on_click(cx.listener(move |app, _, _, _| {
                if let Some(session_id) = app.active_session_id() {
                    app.run_snippet_content(session_id, &content, auto_run);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |app, _, window, cx| {
                    app.show_snippet_context_menu(id, window, cx);
                }),
            )
            .into_any_element()
    }

    pub(super) fn show_snippet_context_menu(
        &mut self,
        id: uuid::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(menu) = prepare_snippet_menu(window, self.language) else {
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.handle_snippet_menu_command(command, id, window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn handle_snippet_menu_command(
        &mut self,
        command: NativeSnippetMenuCommand,
        id: uuid::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            NativeSnippetMenuCommand::Use => {
                let snippet = self
                    .snippet_store
                    .read(cx)
                    .snippets()
                    .iter()
                    .find(|s| s.id == id)
                    .cloned();
                if let (Some(snippet), Some(session_id)) = (snippet, self.active_session_id()) {
                    self.run_snippet_content(session_id, &snippet.content, snippet.auto_run);
                }
            }
            NativeSnippetMenuCommand::Edit => self.open_snippet_editor(Some(id), window, cx),
            NativeSnippetMenuCommand::Delete => self.confirm_delete_snippet(id, window, cx),
        }
    }

    pub(super) fn open_snippet_editor(
        &mut self,
        editing: Option<uuid::Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = self.language;
        let existing = editing.and_then(|id| {
            self.snippet_store
                .read(cx)
                .snippets()
                .iter()
                .find(|s| s.id == id)
                .cloned()
        });
        let name_seed = existing
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let content_seed = existing
            .as_ref()
            .map(|s| s.content.clone())
            .unwrap_or_default();
        let auto_run_seed = existing.as_ref().map(|s| s.auto_run).unwrap_or(false);
        let title = if editing.is_some() {
            language.edit_snippet()
        } else {
            language.add_snippet()
        };
        let prompt = prompt_for_snippet(
            window,
            title,
            &name_seed,
            &content_seed,
            auto_run_seed,
            language.snippet_name_placeholder(),
            language.snippet_command_label(),
            language.snippet_description(),
            language.auto_run_label(),
            language.save(),
            language.cancel(),
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Some((name, content, auto_run))) = prompt.await else {
                return;
            };
            if name.trim().is_empty() && content.trim().is_empty() {
                return;
            }
            let _ = this.update(cx, |app, cx| {
                match editing {
                    Some(id) => app.snippet_store.update(cx, |store, cx| {
                        store.update(id, name, content, auto_run, cx)
                    }),
                    None => app.snippet_store.update(cx, |store, cx| {
                        store.add(Snippet::new(name, content, auto_run), cx)
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn confirm_delete_snippet(
        &mut self,
        id: uuid::Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = self.language;
        let prompt = window.prompt(
            PromptLevel::Warning,
            language.delete_snippet_confirm_title(),
            Some(language.delete_snippet_confirm_message()),
            &[language.delete(), language.cancel()],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = prompt.await else {
                return;
            };
            if choice != 0 {
                return;
            }
            let _ = this.update(cx, |app, cx| {
                app.snippet_store
                    .update(cx, |store, cx| store.remove(id, cx));
                cx.notify();
            });
        })
        .detach();
    }

    // ===== Notes tab =========================================================================

    pub(super) fn render_notes_tab(&self, _cx: &mut Context<Self>) -> AnyElement {
        let colors = self.colors;
        let Some(editor) = self.notes_editor.as_ref() else {
            return div().into_any_element();
        };
        // The notes tab lives in the sidebar's bounded region (not its outer scroll), so this scroll
        // container has a real height: the multi-line TextInput grows by (wrapped) line count and
        // scrolls INSIDE here. A self-drawn scrollbar (GPUI's overflow_y_scroll paints none) sits on
        // the textarea itself.
        let scroll_handle = editor.scroll_handle.clone();
        let muted = colors.muted;
        div()
            .relative()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .id("notes-scroll")
                    .size_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&editor.scroll_handle)
                    .p_2()
                    .rounded_md()
                    .bg(rgb(colors.panel_alt))
                    .child(editor.input.clone()),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |track_bounds, _, window, _| {
                        let max_offset = f32::from(scroll_handle.max_offset().y);
                        if max_offset <= 0. {
                            return;
                        }
                        let viewport_height = f32::from(scroll_handle.bounds().size.height);
                        let track_height = f32::from(track_bounds.size.height);
                        let content_height = viewport_height + max_offset;
                        let thumb_height = (track_height * viewport_height / content_height)
                            .max(SIDEBAR_SCROLLBAR_MIN_THUMB_HEIGHT)
                            .min(track_height);
                        let progress =
                            (-f32::from(scroll_handle.offset().y) / max_offset).clamp(0., 1.);
                        let thumb_top = f32::from(track_bounds.origin.y)
                            + (track_height - thumb_height) * progress;
                        let thumb_bounds = Bounds::new(
                            point(track_bounds.origin.x, px(thumb_top)),
                            size(track_bounds.size.width, px(thumb_height)),
                        );
                        window.paint_quad(quad(
                            thumb_bounds,
                            px(SIDEBAR_SCROLLBAR_THUMB_WIDTH / 2.),
                            rgba((muted << 8) | 0x99),
                            px(0.),
                            rgba(0x00000000),
                            Default::default(),
                        ));
                    },
                )
                .absolute()
                .top(px(4.))
                .right(px(2.))
                .bottom(px(4.))
                .w(px(SIDEBAR_SCROLLBAR_THUMB_WIDTH)),
            )
            .into_any_element()
    }

    pub(super) fn on_notes_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(editor) = self.notes_editor.as_mut() else {
            return;
        };
        editor.save_epoch = editor.save_epoch.wrapping_add(1);
        let epoch = editor.save_epoch;
        // Debounce: only the last keystroke in a ~500ms burst writes to disk.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |app, cx| {
                let Some(editor) = app.notes_editor.as_ref() else {
                    return;
                };
                if editor.save_epoch != epoch {
                    return; // Superseded by a newer keystroke.
                }
                let text = editor.input.read(cx).content().to_owned();
                app.notes_store
                    .update(cx, |store, cx| store.set_text(text, cx));
            });
        })
        .detach();
    }
}

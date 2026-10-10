use super::*;

impl EggieApp {
    pub(super) fn set_marquee_hover(
        &mut self,
        keys: &[String],
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let mut states = self.sidebar.marquee_states.borrow_mut();
        let mut changed = false;
        for key in keys {
            let state = states.entry(key.clone()).or_default();
            if state.hovered != hovered {
                let now = Instant::now();
                state.from = marquee_offset(*state, now);
                state.hovered = hovered;
                state.ever_hovered |= hovered && state.overflow > 0.5;
                state.to = if hovered { state.overflow } else { 0. };
                state.started_at = Some(now);
                state.duration = marquee_duration((state.to - state.from).abs(), hovered);
                state.revision += 1;
                changed = true;
            }
        }
        drop(states);
        if changed {
            cx.notify();
        }
    }

    pub(super) fn render_marquee(
        &self,
        key: String,
        title: String,
        font_size: f32,
        height: f32,
        background: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self
            .sidebar
            .marquee_states
            .borrow()
            .get(&key)
            .copied()
            .unwrap_or_default();
        let app = cx.entity().downgrade();
        let states = self.sidebar.marquee_states.clone();
        let measured_key = key.clone();
        let measured_title = title.clone();
        let mut viewport = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h(px(height))
            .overflow_hidden()
            .text_size(px(font_size))
            .on_children_prepainted(move |children, window, cx| {
                let Some(bounds) = children.first() else {
                    return;
                };
                let text_width = window
                    .text_system()
                    .layout_line(
                        &measured_title,
                        px(font_size),
                        &[window.text_style().to_run(measured_title.len())],
                        None,
                    )
                    .width;
                let overflow = (f32::from(text_width) - f32::from(bounds.size.width)).max(0.);
                if states
                    .borrow()
                    .get(&measured_key)
                    .is_some_and(|state| (state.overflow - overflow).abs() <= 0.5)
                {
                    return;
                }
                let key = measured_key.clone();
                let app = app.clone();
                cx.defer(move |cx| {
                    app.update(cx, |app, cx| {
                        let mut states = app.sidebar.marquee_states.borrow_mut();
                        let state = states.entry(key).or_default();
                        if (state.overflow - overflow).abs() > 0.5 {
                            let now = Instant::now();
                            state.from = marquee_offset(*state, now).min(overflow);
                            state.overflow = overflow;
                            state.ever_hovered |= state.hovered && overflow > 0.5;
                            state.to = if state.hovered { overflow } else { 0. };
                            state.started_at = Some(now);
                            state.duration =
                                marquee_duration((state.to - state.from).abs(), state.hovered);
                            state.revision += 1;
                            drop(states);
                            cx.notify();
                        }
                    })
                    .ok();
                });
            })
            // This child has the exact viewport width, even when the following text overflows.
            .child(div().w_full().h(px(0.)));
        let text = div()
            .absolute()
            .left_0()
            .top_0()
            .whitespace_nowrap()
            .child(title);
        if state.overflow <= 0.5 {
            return viewport.child(text).into_any_element();
        }
        if !state.ever_hovered {
            return viewport
                .child(text)
                .child(div().absolute().right_0().top_0().bottom_0().w(px(16.)).bg(
                    linear_gradient(
                        270.,
                        linear_color_stop(rgb(background), 0.),
                        linear_color_stop(rgb(background).opacity(0.), 1.),
                    ),
                ))
                .into_any_element();
        }
        let offset = move |p: f32| state.from + (state.to - state.from) * p;
        viewport = viewport.child(text.with_animation(
            format!("marquee-text-{key}-{}", state.revision),
            Animation::new(state.duration).with_easing(gpui::ease_in_out),
            move |text, p| text.ml(px(-offset(p))),
        ));
        let color = rgb(background);
        viewport
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(16.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(color, 0.),
                        linear_color_stop(color.opacity(0.), 1.),
                    ))
                    .with_animation(
                        format!("marquee-left-{key}-{}", state.revision),
                        Animation::new(state.duration).with_easing(gpui::ease_in_out),
                        move |mask, p| mask.opacity(offset(p) / state.overflow),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(16.))
                    .bg(linear_gradient(
                        270.,
                        linear_color_stop(color, 0.),
                        linear_color_stop(color.opacity(0.), 1.),
                    ))
                    .with_animation(
                        format!("marquee-right-{key}-{}", state.revision),
                        Animation::new(state.duration).with_easing(gpui::ease_in_out),
                        move |mask, p| mask.opacity(1. - offset(p) / state.overflow),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn dismiss_sidebar_preview(&mut self, cx: &mut Context<Self>) {
        self.sidebar.hovered_sidebar_session = None;
        self.sidebar.sidebar_preview_revision += 1;
        if let Some(preview) = self.sidebar.sidebar_preview.take() {
            preview
                .window
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .ok();
        }
    }

    pub(super) fn sidebar_preview_hover(
        &mut self,
        session_id: SessionId,
        hovered: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered && cx.has_active_drag() {
            return;
        }
        if hovered {
            self.sidebar.hovered_sidebar_session = Some(session_id);
            self.sidebar.sidebar_preview_revision += 1;
            if self
                .sidebar
                .sidebar_preview
                .as_ref()
                .map(|preview| preview.session_id)
                != Some(session_id)
            {
                if let Some(preview) = self.sidebar.sidebar_preview.take() {
                    preview
                        .window
                        .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                        .ok();
                }
                let revision = self.sidebar.sidebar_preview_revision;
                let executor = cx.background_executor().clone();
                cx.spawn(async move |app, cx| {
                    executor.timer(SIDEBAR_PREVIEW_HOVER_DELAY).await;
                    app.update_in(cx, |app, window, cx| {
                        if app.sidebar.hovered_sidebar_session == Some(session_id)
                            && app.sidebar.sidebar_preview_revision == revision
                        {
                            app.show_sidebar_preview(session_id, window, cx);
                        }
                    })
                    .ok();
                })
                .detach();
            }
        } else if self.sidebar.hovered_sidebar_session == Some(session_id) {
            self.sidebar.hovered_sidebar_session = None;
            self.sidebar.sidebar_preview_revision += 1;
        }
        self.sidebar_preview_pointer(session_id, true, hovered, cx);
    }

    pub(super) fn sidebar_preview_pointer(
        &mut self,
        session_id: SessionId,
        anchor: bool,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.sidebar.sidebar_preview.as_mut() else {
            return;
        };
        if preview.session_id != session_id {
            return;
        }
        if anchor {
            preview.anchor_hovered = hovered;
        } else {
            preview.popup_hovered = hovered;
        }
        preview.leave_revision += 1;
        if hovered {
            preview
                .window
                .update(cx, |popup, _, cx| popup.restore(cx))
                .ok();
        } else if !preview.anchor_hovered && !preview.popup_hovered {
            let revision = preview.leave_revision;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |app, cx| {
                executor.timer(SIDEBAR_PREVIEW_LEAVE_DELAY).await;
                app.update(cx, |app, cx| {
                    if app.sidebar.sidebar_preview.as_ref().is_some_and(|preview| {
                        preview.session_id == session_id
                            && preview.leave_revision == revision
                            && !preview.anchor_hovered
                            && !preview.popup_hovered
                    }) {
                        app.dismiss_sidebar_preview(cx);
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn show_sidebar_preview(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self
            .session_feed
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
        else {
            return;
        };
        let Some(anchor) = self
            .sidebar
            .sidebar_row_bounds
            .borrow()
            .get(&session_id)
            .copied()
        else {
            return;
        };
        self.dismiss_project_popover(cx);
        let title = self
            .workspace_state
            .project_views
            .get(&session.project_id)
            .and_then(|view| {
                view.layout.terminal_custom_title(session_id).or_else(|| {
                    session
                        .program_status
                        .as_ref()
                        .and_then(|status| status.title.as_deref())
                })
            })
            .unwrap_or(&session.title)
            .to_owned();
        let colors = self.colors;
        let language = self.language;
        let dark = self.use_dark;
        let icon = process_icons::card_stack(
            &[self.icon_for_item(ItemKind::Terminal, Some(session_id))],
            dark,
            SIDEBAR_CARD_SIZE,
        );
        let snapshot = self.session_feed.snapshots.get(&session_id).cloned();
        let appearance = self.resolved_session_appearance(session_id, cx);
        let theme = appearance.theme;
        let font_family: SharedString = appearance.font_family.to_string().into();
        let thumbnail_font_size = snapshot.as_ref().map_or(7., |snapshot| {
            (286. / snapshot.size.columns.max(1) as f32 / 0.62)
                .min(116. / snapshot.size.rows.max(1) as f32 / 1.35)
                .clamp(3.5, 9.)
        });
        let renderer = MetalTerminalRenderer::default();
        let inspection = Rc::new(RefCell::new(
            self.session_feed
                .session_inspections
                .get(&session_id)
                .cloned(),
        ));
        let inspection_for_popup = inspection.clone();
        let parent = Window::window_handle(window)
            .downcast::<Self>()
            .expect("sidebar preview belongs to an Eggie window");
        let status = session.program_status.clone();
        let osc_progress = self
            .session_feed
            .progress_updates
            .get(&session_id)
            .and_then(|update| update.progress);
        let status_label = status
            .as_ref()
            .map(|status| program_state_label(status.state, language).to_owned())
            .or_else(|| {
                osc_progress.map(|progress| {
                    let label = match (progress.state, language) {
                        (TerminalProgressState::Normal, Language::English) => "Progress",
                        (TerminalProgressState::Normal, Language::SimplifiedChinese) => "进度",
                        (TerminalProgressState::Error, Language::English) => "Error",
                        (TerminalProgressState::Error, Language::SimplifiedChinese) => "出错",
                        (TerminalProgressState::Indeterminate, Language::English) => "Working",
                        (TerminalProgressState::Indeterminate, Language::SimplifiedChinese) => {
                            "进行中"
                        }
                        (TerminalProgressState::Paused, Language::English) => "Paused",
                        (TerminalProgressState::Paused, Language::SimplifiedChinese) => "已暂停",
                    };
                    progress
                        .percent
                        .map_or_else(|| label.to_owned(), |value| format!("{label} {value}%"))
                })
            })
            .unwrap_or_else(|| match (&session.status, language) {
                (SessionStatus::Running, Language::English) => "Running".to_owned(),
                (SessionStatus::Running, Language::SimplifiedChinese) => "运行中".to_owned(),
                (SessionStatus::Exited { .. }, Language::English) => "Exited".to_owned(),
                (SessionStatus::Exited { .. }, Language::SimplifiedChinese) => "已退出".to_owned(),
            });
        let status_tint = status
            .as_ref()
            .map(|status| match status.state {
                TerminalProgramState::Blocked => self.interface_theme.palette[3],
                TerminalProgramState::Error => self.interface_theme.palette[1],
                TerminalProgramState::Working => colors.accent,
                TerminalProgramState::Done => self.interface_theme.palette[2],
                TerminalProgramState::Idle => colors.muted,
            })
            .or_else(|| {
                osc_progress.map(|progress| match progress.state {
                    TerminalProgressState::Error => self.interface_theme.palette[1],
                    TerminalProgressState::Paused => self.interface_theme.palette[3],
                    _ => colors.accent,
                })
            });
        let mut status_detail = status
            .as_ref()
            .and_then(|status| status.message.clone())
            .or_else(|| session.active_command.clone())
            .unwrap_or_else(|| session.current_directory.display().to_string());
        if let Some(progress) = status.as_ref().and_then(|status| status.progress) {
            status_detail.push_str(&format!(" · {progress}%"));
        }
        let memory_label = if language == Language::English {
            "Process memory"
        } else {
            "进程内存"
        };
        let popup = PopupWindow::open(
            cx,
            window,
            PopupWindowOptions {
                anchor,
                content_size: size(px(318.), px(236.)),
                placement: PopupPlacement::Beside,
                background: colors.panel,
                border: colors.border,
            },
            move |_, _| {
                let memory = inspection_for_popup
                    .borrow()
                    .as_ref()
                    .and_then(|inspection| {
                        inspection
                            .processes
                            .iter()
                            .filter_map(|process| process.memory_bytes)
                            .reduce(u64::saturating_add)
                    });
                let thumbnail: AnyElement = if let Some(snapshot) = &snapshot {
                    div()
                        .size_full()
                        .bg(rgba(terminal_background(snapshot, theme)))
                        .font_family(font_family.clone())
                        .text_size(px(thumbnail_font_size))
                        .child(
                            renderer.render(
                                snapshot.clone(),
                                TerminalRenderOptions::new(
                                    theme,
                                    colors.accent,
                                    0.,
                                    None,
                                    None,
                                    None,
                                    InputLatencyTracker::default(),
                                )
                                .with_cursor_visible(false),
                            ),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size_full()
                        .bg(rgb(theme.background))
                        .text_color(rgb(theme.foreground))
                        .text_size(px(11.))
                        .child(if language == Language::English {
                            "Waiting for terminal output"
                        } else {
                            "等待终端画面"
                        })
                        .into_any_element()
                };
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .p_3()
                    .gap_2()
                    .text_color(rgb(colors.text))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when_some(icon.clone(), |row, icon| {
                                row.child(
                                    img(icon)
                                        .flex_none()
                                        .w(px(process_icons::stack_width(1, SIDEBAR_CARD_SIZE)))
                                        .h(px(SIDEBAR_CARD_SIZE + 5.)),
                                )
                            })
                            .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .when_some(status_tint, |tag, tint| {
                                        tag.bg(rgba((tint << 8) | 0x28))
                                    })
                                    .text_size(px(10.))
                                    .text_color(rgb(status_tint.unwrap_or(colors.muted)))
                                    .child(status_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(status_detail.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(120.))
                            .overflow_hidden()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(colors.border))
                            .child(thumbnail),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(session.current_process.name.clone())
                            .child(format!("{memory_label} {}", format_memory_usage(memory))),
                    )
                    .into_any_element()
            },
            move |hovered, _, cx| {
                cx.defer(move |cx| {
                    parent
                        .update(cx, |app, _, cx| {
                            app.sidebar_preview_pointer(session_id, false, hovered, cx);
                        })
                        .ok();
                });
            },
        );
        let Ok(popup) = popup else {
            return;
        };
        self.sidebar.sidebar_preview = Some(SidebarPreviewState {
            session_id,
            window: popup,
            anchor_hovered: true,
            popup_hovered: false,
            leave_revision: 0,
        });
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |app, cx| {
            let result = executor
                .spawn(async move { client.request(ClientRequest::InspectSession { session_id }) })
                .await;
            app.update(cx, |app, cx| {
                if !app
                    .sidebar
                    .sidebar_preview
                    .as_ref()
                    .is_some_and(|preview| preview.session_id == session_id)
                {
                    return;
                }
                if let Ok(DaemonResponse::SessionInspection {
                    inspection: mut current,
                }) = result
                {
                    app.fill_missing_process_metrics(&mut current);
                    *inspection.borrow_mut() = Some(current.clone());
                    app.session_feed
                        .session_inspections
                        .insert(session_id, current);
                    popup.update(cx, |_, _, cx| cx.notify()).ok();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn dismiss_project_popover(&mut self, cx: &mut Context<Self>) {
        if let Some(popover) = self.sidebar.project_popover.take() {
            popover
                .window
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .ok();
        }
    }

    pub(super) fn project_popover_hover(
        &mut self,
        project_id: ProjectId,
        over_anchor: bool,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered && cx.has_active_drag() {
            return;
        }
        if hovered && over_anchor {
            if !self
                .workspace_state
                .collapsed_projects
                .contains(&project_id)
            {
                return;
            }
            if self
                .sidebar
                .project_popover
                .as_ref()
                .map(|popover| popover.project_id)
                != Some(project_id)
            {
                self.show_project_popover(project_id, window, cx);
            }
        }
        let Some(popover) = self.sidebar.project_popover.as_mut() else {
            return;
        };
        if popover.project_id != project_id {
            return;
        }
        if over_anchor {
            popover.anchor_hovered = hovered;
        } else {
            popover.popup_hovered = hovered;
        }
        popover.leave_revision += 1;
        if hovered {
            popover
                .window
                .update(cx, |popup, _, cx| popup.restore(cx))
                .ok();
        } else if !popover.anchor_hovered && !popover.popup_hovered {
            let revision = popover.leave_revision;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |app, cx| {
                executor.timer(PROJECT_POPOVER_LEAVE_DELAY).await;
                app.update(cx, |app, cx| {
                    if app.sidebar.project_popover.as_ref().is_some_and(|popover| {
                        popover.project_id == project_id
                            && popover.leave_revision == revision
                            && !popover.anchor_hovered
                            && !popover.popup_hovered
                    }) {
                        app.dismiss_project_popover(cx);
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn show_project_popover(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_project_popover(cx);
        let Some(view) = self.workspace_state.project_views.get(&project_id) else {
            return;
        };
        let Some(anchor) = self
            .sidebar
            .project_row_bounds
            .borrow()
            .get(&project_id)
            .copied()
        else {
            return;
        };
        let mut tabs = Vec::new();
        for group_id in view.layout.group_ids() {
            let Some(group) = view.layout.find_group(group_id) else {
                continue;
            };
            for item in &group.items {
                let status = item.session_id.and_then(|id| {
                    self.session_feed
                        .sessions
                        .iter()
                        .find(|session| session.id == id)
                        .and_then(|session| session.program_status.as_ref())
                });
                let title = item
                    .custom_title
                    .as_deref()
                    .or_else(|| status.and_then(|status| status.title.as_deref()))
                    .unwrap_or_else(|| item.display_title())
                    .to_owned();
                tabs.push(ProjectPopoverTab {
                    group_id,
                    item_id: item.id,
                    title,
                    subtitle: status.map(|status| {
                        status.message.clone().unwrap_or_else(|| {
                            program_state_label(status.state, self.language).into()
                        })
                    }),
                    icon: self.icon_for_item(item.kind, item.session_id),
                    active: self.workspace_state.active_project == project_id
                        && view.active_group_id == group_id
                        && group.active_item_id == Some(item.id),
                });
            }
        }
        if tabs.is_empty() {
            return;
        }
        let colors = self.colors;
        let dark = self.use_dark;
        let parent = Window::window_handle(window)
            .downcast::<Self>()
            .expect("project popover belongs to an Eggie window");
        // Row padding is 4 px on each side; the outer border also consumes 1 px per side.
        let content_height = tabs.len().min(8) as f32 * PROJECT_POPOVER_ROW_HEIGHT + 10.;
        let popup = PopupWindow::open(
            cx,
            window,
            PopupWindowOptions {
                anchor,
                content_size: size(px(255.), px(content_height)),
                placement: PopupPlacement::Beside,
                background: colors.panel,
                border: colors.border,
            },
            move |_, _| {
                let mut list = div()
                    .id("project-popover-tabs")
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(rgb(colors.text));
                for tab in &tabs {
                    let group_id = tab.group_id;
                    let item_id = tab.item_id;
                    let icon = process_icons::card_stack(&[tab.icon], dark, SIDEBAR_CARD_SIZE)
                        .expect("single process card");
                    let mut title = div().flex_1().min_w_0().truncate().child(tab.title.clone());
                    if let Some(subtitle) = &tab.subtitle {
                        title = div()
                            .flex_1()
                            .min_w_0()
                            .flex_col()
                            .child(div().truncate().child(tab.title.clone()))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(10.))
                                    .text_color(rgb(colors.muted))
                                    .child(subtitle.clone()),
                            );
                    }
                    list = list.child(
                        div()
                            .id(format!("project-popover-tab-{item_id}"))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .h(px(PROJECT_POPOVER_ROW_HEIGHT))
                            .mx_1()
                            .px_2()
                            .rounded_md()
                            .when(tab.active, |row| row.bg(rgb(colors.panel_alt)))
                            .hover(move |row| row.bg(rgb(colors.hover)))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, popup_window, cx| {
                                popup_window.remove_window();
                                // The popup can close as the pointer leaves its parent window.
                                // Commit selection from the press event, then switch once both
                                // windows have returned to the app.
                                cx.defer(move |cx| {
                                    if let Err(error) =
                                        parent.update(cx, |app, parent_window, cx| {
                                            app.sidebar.project_popover = None;
                                            app.activate_project(project_id, cx);
                                            app.activate_item(group_id, item_id, parent_window, cx);
                                            parent_window.activate_window();
                                        })
                                    {
                                        eprintln!(
                                            "failed to activate project popover tab: {error:#}"
                                        );
                                    }
                                });
                                cx.stop_propagation();
                            })
                            .child(
                                img(icon)
                                    .flex_none()
                                    .w(px(process_icons::stack_width(1, SIDEBAR_CARD_SIZE)))
                                    .h(px(SIDEBAR_CARD_SIZE + 5.)),
                            )
                            .child(title),
                    );
                }
                list.into_any_element()
            },
            move |hovered, _, cx| {
                cx.defer(move |cx| {
                    if let Err(error) = parent.update(cx, |app, parent_window, cx| {
                        app.project_popover_hover(project_id, false, hovered, parent_window, cx);
                    }) {
                        eprintln!("failed to update project popover hover: {error:#}");
                    }
                });
            },
        );
        match popup {
            Ok(window) => {
                self.sidebar.project_popover = Some(ProjectPopoverState {
                    project_id,
                    window,
                    anchor_hovered: true,
                    popup_hovered: false,
                    leave_revision: 0,
                });
            }
            Err(error) => eprintln!("failed to open project popover: {error:#}"),
        }
    }
}

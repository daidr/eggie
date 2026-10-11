use super::*;

impl EggieApp {
    pub(super) fn popup_tabs_from_manager(
        project: ProjectId,
        local: WindowId,
        manager: &WindowManager,
        language: Language,
    ) -> Vec<ProjectPopoverTab> {
        let mut tabs = Vec::new();
        for window in manager.state.windows.values() {
            let Some(view) = window.views.get(&project) else {
                continue;
            };
            for group_id in view.layout.group_ids() {
                let group = view.layout.find_group(group_id).unwrap();
                for item in &group.items {
                    let session = item
                        .session_id
                        .and_then(|id| manager.sessions.iter().find(|s| s.id == id));
                    let status = session.and_then(|s| s.program_status.as_ref());
                    let title = item
                        .custom_title
                        .as_deref()
                        .or_else(|| status.and_then(|s| s.title.as_deref()))
                        .unwrap_or_else(|| item.display_title())
                        .to_owned();
                    let subtitle = if window.id == local {
                        status.map(|s| {
                            s.message
                                .clone()
                                .unwrap_or_else(|| program_state_label(s.state, language).into())
                        })
                    } else {
                        Some(manager.window_label(window.id))
                    };
                    tabs.push(ProjectPopoverTab {
                        location: eggie_domain::ItemLocation {
                            window_id: window.id,
                            project_id: project,
                            group_id,
                            item_id: item.id,
                        },
                        session_id: item.session_id,
                        item_id: item.id,
                        title,
                        subtitle,
                        icon: session
                            .map(process_icons::for_session)
                            .unwrap_or(ProcessIcon::Terminal),
                        active: window.id == local
                            && window.active_project == project
                            && view.active_group_id == group_id
                            && group.active_item_id == Some(item.id),
                    });
                }
            }
        }
        // Background terminals route through the same global navigator as the sidebar.
        for (&id, tab) in &manager.state.background {
            if tab.project_id != project {
                continue;
            }
            let session = manager.sessions.iter().find(|s| s.id == id);
            let location = eggie_domain::ItemLocation {
                window_id: local,
                project_id: project,
                group_id: manager.state.windows[&local].views[&project].active_group_id,
                item_id: tab.item.id,
            };
            tabs.push(ProjectPopoverTab {
                location,
                session_id: Some(id),
                item_id: tab.item.id,
                title: tab.item.display_title().into(),
                subtitle: Some(
                    if language == Language::SimplifiedChinese {
                        "后台运行"
                    } else {
                        "Background"
                    }
                    .into(),
                ),
                icon: session
                    .map(process_icons::for_session)
                    .unwrap_or(ProcessIcon::Terminal),
                active: false,
            });
        }
        tabs
    }

    pub(super) fn project_popup_tabs(
        &self,
        project: ProjectId,
        cx: &App,
    ) -> Vec<ProjectPopoverTab> {
        Self::popup_tabs_from_manager(
            project,
            self.window_id,
            self.window_manager.read(cx),
            self.language,
        )
    }

    pub(super) fn select_popup_terminal(
        &mut self,
        location: eggie_domain::ItemLocation,
        session: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = session {
            self.navigate_terminal(id, window, cx);
        } else if location.window_id == self.window_id {
            self.activate_project(location.project_id, cx);
            self.activate_item(location.group_id, location.item_id, window, cx);
        } else if let Some(handle) = self
            .window_manager
            .read(cx)
            .handles
            .get(&location.window_id)
            .copied()
        {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |app, window, cx| {
                    app.pull_window_state(cx);
                    app.activate_project(location.project_id, cx);
                    app.activate_item(location.group_id, location.item_id, window, cx);
                    window.activate_window();
                });
            });
        }
        window.activate_window();
    }

    pub(super) fn project_popup_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.keystroke.key == "escape"
            && (self.sidebar.project_popover.is_some() || self.sidebar.sidebar_preview.is_some())
        {
            self.dismiss_project_popover(cx);
            self.dismiss_sidebar_preview(cx);
            return true;
        }
        let Some(popover) = self.sidebar.project_popover.as_ref() else {
            return false;
        };
        let tabs = self.project_popup_tabs(popover.project_id, cx);
        if tabs.is_empty() {
            self.dismiss_project_popover(cx);
            return false;
        }
        let index = project_popover_selection_index(&tabs, popover.keyboard_target.get());
        match event.keystroke.key.as_str() {
            "up" | "down" => {
                let next = index.map_or(0, |index| {
                    if event.keystroke.key == "up" {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(tabs.len() - 1)
                    }
                });
                popover.keyboard_target.set(Some(tabs[next].item_id));
                popover.scroll.scroll_to_item(next);
                let _ = popover.window.update(cx, |_, _, cx| cx.notify());
                true
            }
            "enter" => {
                let tab = &tabs[index.unwrap_or(0)];
                let (location, session) = (tab.location, tab.session_id);
                self.dismiss_project_popover(cx);
                self.select_popup_terminal(location, session, window, cx);
                true
            }
            _ => false,
        }
    }
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
            // The absolute text must use the viewport's line box, not the inherited 1.618em.
            .line_height(px(height))
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered && (cx.has_active_drag() || self.sidebar.project_selector.is_some()) {
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
                if self.sidebar.sidebar_preview.is_some() {
                    // An open preview follows the pointer; only the first preview waits.
                    self.show_sidebar_preview(session_id, window, cx);
                } else {
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
            let content_revision = preview.content_revision;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |app, cx| {
                executor.timer(SIDEBAR_PREVIEW_LEAVE_DELAY).await;
                app.update(cx, |app, cx| {
                    if app.sidebar.sidebar_preview.as_ref().is_some_and(|preview| {
                        preview.is_current_target(session_id, content_revision)
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
        let colors = self.colors;
        let language = self.language;
        let dark = self.use_dark;
        let icon = process_icons::card_stack(
            &[self.icon_for_item(ItemKind::Terminal, Some(session_id))],
            dark,
            SIDEBAR_CARD_SIZE,
        );
        let snapshot = Rc::new(RefCell::new(
            self.session_feed.snapshots.get(&session_id).cloned(),
        ));
        let snapshot_for_popup = snapshot.clone();
        let appearance = self.resolved_session_appearance(session_id, cx);
        let theme = appearance.theme;
        let font_family: SharedString = appearance.font_family.to_string().into();
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
        let manager = self.window_manager.clone();
        let palette = self.interface_theme.palette;
        let memory_label = if language == Language::English {
            "Process memory"
        } else {
            "进程内存"
        };
        let content_revision = self.sidebar.sidebar_preview_revision;
        let options = PopupWindowOptions {
            anchor,
            content_size: size(px(318.), px(236.)),
            placement: PopupPlacement::Beside,
            background: colors.panel,
            border: colors.border,
        };
        let content = move |_: &mut Window, cx: &mut Context<PopupWindow>| {
            let registry = manager.read(cx);
            let session = registry
                .sessions
                .iter()
                .find(|current| current.id == session_id)
                .unwrap_or(&session);
            let title = registry
                .state
                .locate(session_id)
                .and_then(|location| registry.state.item(location))
                .and_then(|item| item.custom_title.as_deref())
                .or_else(|| {
                    session
                        .program_status
                        .as_ref()
                        .and_then(|status| status.title.as_deref())
                })
                .unwrap_or(&session.title)
                .to_owned();
            let status = session.program_status.clone();
            let osc_progress = registry
                .progress
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
                            (TerminalProgressState::Paused, Language::SimplifiedChinese) => {
                                "已暂停"
                            }
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
                    (SessionStatus::Exited { .. }, Language::SimplifiedChinese) => {
                        "已退出".to_owned()
                    }
                });
            let status_tint = status
                .as_ref()
                .map(|status| match status.state {
                    TerminalProgramState::Blocked => palette[3],
                    TerminalProgramState::Error => palette[1],
                    TerminalProgramState::Working => colors.accent,
                    TerminalProgramState::Done => palette[2],
                    TerminalProgramState::Idle => colors.muted,
                })
                .or_else(|| {
                    osc_progress.map(|progress| match progress.state {
                        TerminalProgressState::Error => palette[1],
                        TerminalProgressState::Paused => palette[3],
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
            let snapshot = snapshot_for_popup.borrow().clone();
            let thumbnail_font_size = snapshot.as_ref().map_or(7., |snapshot| {
                (286. / snapshot.size.columns.max(1) as f32 / 0.62)
                    .min(116. / snapshot.size.rows.max(1) as f32 / 1.35)
                    .clamp(3.5, 9.)
            });
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
                        .mask_corners(rgb(colors.panel))
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
        };
        let on_hover = move |hovered, _: &mut Window, cx: &mut App| {
            cx.defer(move |cx| {
                parent
                    .update(cx, |app, _, cx| {
                        if app.sidebar.sidebar_preview.as_ref().is_some_and(|preview| {
                            preview.is_current_target(session_id, content_revision)
                        }) {
                            app.sidebar_preview_pointer(session_id, false, hovered, cx);
                        }
                    })
                    .ok();
            });
        };
        let popup = if let Some(preview) = &self.sidebar.sidebar_preview {
            let handle = preview.window;
            handle
                .update(cx, |popup, popup_window, cx| {
                    popup.retarget(window, options, content, on_hover, popup_window, cx);
                })
                .map(|_| handle)
        } else {
            PopupWindow::open(cx, window, options, content, on_hover)
        };
        let Ok(popup) = popup else {
            self.sidebar.sidebar_preview = None;
            return;
        };
        self.sidebar.sidebar_preview = Some(SidebarPreviewState {
            session_id,
            content_revision,
            window: popup,
            anchor_hovered: true,
            popup_hovered: false,
            leave_revision: 0,
        });
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |app, cx| {
            let mut tick = 0;
            loop {
                let live = app
                    .update(cx, |app, _| {
                        app.sidebar.sidebar_preview.as_ref().is_some_and(|preview| {
                            preview.window.window_id() == popup.window_id()
                                && preview.is_current_target(session_id, content_revision)
                        })
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
                let client = client.clone();
                let (frame, metrics) = executor
                    .spawn(async move {
                        let frame = client.request(ClientRequest::Snapshot { session_id });
                        let metrics = (tick % 3 == 0)
                            .then(|| client.request(ClientRequest::InspectSession { session_id }));
                        (frame, metrics)
                    })
                    .await;
                let live = app
                    .update(cx, |app, cx| {
                        if !app.sidebar.sidebar_preview.as_ref().is_some_and(|preview| {
                            preview.window.window_id() == popup.window_id()
                                && preview.is_current_target(session_id, content_revision)
                        }) {
                            return false;
                        }
                        if let Ok(DaemonResponse::Snapshot { snapshot: frame }) = frame {
                            *snapshot.borrow_mut() = Some(frame);
                        }
                        if let Some(Ok(DaemonResponse::SessionInspection {
                            inspection: mut current,
                        })) = metrics
                        {
                            app.fill_missing_process_metrics(&mut current);
                            *inspection.borrow_mut() = Some(current.clone());
                            app.session_feed
                                .session_inspections
                                .insert(session_id, current);
                        }
                        popup.update(cx, |_, _, cx| cx.notify()).ok();
                        true
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
                tick += 1;
                executor.timer(Duration::from_millis(300)).await;
            }
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered && (cx.has_active_drag() || self.sidebar.project_selector.is_some()) {
            return;
        }
        if over_anchor {
            self.sidebar.hovered_project = hovered.then_some(project_id);
            self.sidebar.project_hover_revision += 1;
            if hovered
                && self.sidebar.project_popover.as_ref().map(|p| p.project_id) != Some(project_id)
            {
                let revision = self.sidebar.project_hover_revision;
                cx.spawn(async move |app, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(320))
                        .await;
                    let _ = app.update_in(cx, |app, window, cx| {
                        if app.sidebar.hovered_project == Some(project_id)
                            && app.sidebar.project_hover_revision == revision
                            && app.workspace_state.collapsed_projects.contains(&project_id)
                            && !cx.has_active_drag()
                        {
                            app.show_project_popover(project_id, window, cx);
                        }
                    });
                })
                .detach();
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
                        let pressed = app
                            .sidebar
                            .project_popover
                            .as_ref()
                            .and_then(|p| p.window.read(cx).ok())
                            .is_some_and(|p| p.pointer_pressed());
                        if !pressed {
                            app.dismiss_project_popover(cx);
                        }
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
        let Some(anchor) = self
            .sidebar
            .project_row_bounds
            .borrow()
            .get(&project_id)
            .copied()
        else {
            return;
        };
        let tabs = self.project_popup_tabs(project_id, cx);
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
        let keyboard_target = Rc::new(std::cell::Cell::new(None));
        let keyboard_target_for_popup = keyboard_target.clone();
        let scroll = ScrollHandle::new();
        let scroll_for_popup = scroll.clone();
        let manager = self.window_manager.clone();
        let current_window = self.window_id;
        let language = self.language;
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
            move |_, cx| {
                let tabs = Self::popup_tabs_from_manager(
                    project_id,
                    current_window,
                    manager.read(cx),
                    language,
                );
                let mut list = div()
                    .id("project-popover-tabs")
                    .track_scroll(&scroll_for_popup)
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(rgb(colors.text));
                for tab in &tabs {
                    let location = tab.location;
                    let session_id = tab.session_id;
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
                            .border_1()
                            .border_color(rgba(0))
                            .when(tab.active, |row| row.bg(rgb(colors.panel_alt)))
                            .when(keyboard_target_for_popup.get() == Some(item_id), |row| {
                                row.border_color(rgb(colors.accent))
                            })
                            .hover(move |row| row.bg(rgb(colors.hover)))
                            .cursor_pointer()
                            .on_click(move |_, popup_window, cx| {
                                popup_window.remove_window();
                                cx.defer(move |cx| {
                                    let _ = parent.update(cx, |app, parent_window, cx| {
                                        app.sidebar.project_popover = None;
                                        app.select_popup_terminal(
                                            location,
                                            session_id,
                                            parent_window,
                                            cx,
                                        );
                                    });
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
                    keyboard_target,
                    scroll,
                });
            }
            Err(error) => eprintln!("failed to open project popover: {error:#}"),
        }
    }
}

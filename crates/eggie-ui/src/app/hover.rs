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
            && (self.sidebar.project_popover.is_some() || self.sidebar.terminal_preview.is_some())
        {
            self.dismiss_project_popover(cx);
            self.dismiss_terminal_preview(cx);
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

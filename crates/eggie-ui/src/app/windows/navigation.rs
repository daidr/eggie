use super::*;

impl EggieApp {
    pub(in crate::app) fn render_terminal_section_label(&self, label: String) -> AnyElement {
        div()
            .pl_1()
            .pt_1()
            .pb_1()
            .text_size(px(10.))
            .text_color(rgb(self.colors.muted))
            .child(label)
            .into_any_element()
    }

    pub(in crate::app) fn render_nonlocal_terminals(
        &self,
        project: ProjectId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let manager = self.window_manager.read(cx);
        let mut entries = Vec::new();
        for window in manager
            .state
            .windows
            .values()
            .filter(|w| w.id != self.window_id)
        {
            if let Some(view) = window.views.get(&project) {
                for item in view.layout.terminal_items() {
                    if let Some(session) = item.session_id {
                        entries.push((session, item.display_title().to_owned(), Some(window.id)));
                    }
                }
            }
        }
        for (&id, tab) in &manager.state.background {
            if tab.project_id == project {
                entries.push((id, tab.item.display_title().to_owned(), None));
            }
        }
        // Keep each window's tabs together, in stable window-number order; background comes last.
        entries.sort_by_key(|(_, _, location)| {
            location
                .map(|id| manager.state.windows[&id].number)
                .unwrap_or(u32::MAX)
        });
        let mut rows = Vec::new();
        let mut previous_location = None;
        for (session, title, location) in entries {
            if previous_location != Some(location) {
                let label = match location {
                    Some(id) => format!(
                        "{} {}",
                        if self.language == Language::SimplifiedChinese {
                            "窗口"
                        } else {
                            "Window"
                        },
                        manager.state.windows[&id].number,
                    ),
                    None => if self.language == Language::SimplifiedChinese {
                        "后台"
                    } else {
                        "Background"
                    }
                    .into(),
                };
                rows.push(self.render_terminal_section_label(label));
                previous_location = Some(location);
            }
            let live = manager.sessions.iter().find(|s| s.id == session);
            let title = if live.is_none() {
                format!(
                    "{title} · {}",
                    if self.language == Language::SimplifiedChinese {
                        "已结束"
                    } else {
                        "Ended"
                    }
                )
            } else {
                title
            };
            rows.push(
                div()
                    .id(format!("external-terminal-{session}"))
                    .pl_1()
                    .pr_2()
                    .mb_1()
                    .h(px(SIDEBAR_SESSION_ROW_HEIGHT))
                    .rounded_md()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(rgb(self.colors.muted))
                    .cursor_pointer()
                    .hover(|e| e.bg(rgb(self.colors.hover)))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.navigate_terminal(session, window, cx)
                    }))
                    .child(self.render_item_icon(
                        ItemKind::Terminal,
                        Some(session),
                        format!("external-icon-{session}"),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .line_height(px(17.))
                            .child(title),
                    )
                    .child(
                        div()
                            .id(format!("bring-terminal-{session}"))
                            .px_1()
                            .py_1()
                            .rounded_md()
                            .text_size(px(10.))
                            .hover(|e| {
                                e.bg(rgb(self.colors.panel_alt))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |app, _, window, cx| {
                                cx.stop_propagation();
                                app.move_terminal_here(session, window, cx);
                            }))
                            .child(if self.language == Language::SimplifiedChinese {
                                "移入"
                            } else {
                                "Move here"
                            }),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    pub(in crate::app) fn render_ended_terminals(
        &self,
        project: ProjectId,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let manager = self.window_manager.read(cx);
        let mut rows = Vec::new();
        if let Some(view) = self.workspace_state.project_views.get(&project) {
            for item in view.layout.terminal_items().filter(|item| {
                item.session_id
                    .is_some_and(|id| !manager.sessions.iter().any(|s| s.id == id))
            }) {
                let session = item.session_id.unwrap();
                rows.push(
                    div()
                        .id(format!("ended-terminal-{session}"))
                        .pl_1()
                        .pr_2()
                        .mb_1()
                        .h(px(SIDEBAR_SESSION_ROW_HEIGHT))
                        .rounded_md()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .gap_2()
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|e| e.bg(rgb(self.colors.hover)))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            app.navigate_terminal(session, window, cx)
                        }))
                        .child(self.render_item_icon(
                            ItemKind::Terminal,
                            Some(session),
                            format!("ended-icon-{session}"),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .line_height(px(17.))
                                .text_color(rgb(self.colors.muted))
                                .child(format!(
                                    "{} · {}",
                                    item.display_title(),
                                    if self.language == Language::SimplifiedChinese {
                                        "已结束"
                                    } else {
                                        "Ended"
                                    }
                                )),
                        )
                        .into_any_element(),
                );
            }
        }
        rows
    }
    pub(in crate::app) fn navigate_terminal(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.publish_window_state(cx);
        let manager = self.window_manager.read(cx);
        if let Some(location) = manager.state.locate(session_id) {
            if location.window_id == self.window_id {
                self.project_picker = false;
                self.activate_session(location.project_id, session_id, window, cx);
            } else if let Some(handle) = manager.handles.get(&location.window_id).copied() {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |app, window, cx| {
                        app.pull_window_state(cx);
                        app.project_picker = false;
                        window.activate_window();
                        app.activate_session(location.project_id, session_id, window, cx);
                    });
                });
            }
        } else if let Some(project_id) = manager
            .state
            .background
            .get(&session_id)
            .map(|tab| tab.project_id)
        {
            self.restore_terminal_here(session_id, project_id, window, cx);
        }
    }

    pub(in crate::app) fn restore_terminal_here(
        &mut self,
        session_id: SessionId,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace_state.project_views.contains_key(&project_id) {
            return;
        }
        self.publish_window_state(cx);
        let target = self.target_for_project(project_id, cx);
        self.window_manager
            .update(cx, |manager, cx| manager.restore(session_id, target, cx));
        self.pull_window_state(cx);
        self.project_picker = false;
        self.activate_session(project_id, session_id, window, cx);
    }

    pub(in crate::app) fn move_terminal_here(
        &mut self,
        session: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.publish_window_state(cx);
        let source = self.window_manager.read(cx).state.locate(session);
        if let Some(source) = source {
            let target = self.target_for_project(source.project_id, cx);
            self.window_manager
                .update(cx, |manager, cx| manager.move_item(source, target, cx));
            self.pull_window_state(cx);
            self.project_picker = false;
            self.activate_session(source.project_id, session, window, cx);
        } else {
            self.navigate_terminal(session, window, cx);
        }
    }
}

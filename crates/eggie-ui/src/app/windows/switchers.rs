use super::*;

impl EggieApp {
    pub(in crate::app) fn project_picker_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.project_picker {
            return false;
        }
        let projects = &self.workspace_state.projects;
        let index = projects
            .iter()
            .position(|p| p.id == self.workspace_state.active_project)
            .unwrap_or(0);
        match event.keystroke.key.as_str() {
            "up" | "down" => {
                let next = if event.keystroke.key == "up" {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(projects.len().saturating_sub(1))
                };
                if let Some(project) = projects.get(next) {
                    self.workspace_state.active_project = project.id;
                    cx.notify();
                }
                true
            }
            "enter" => {
                self.project_picker = false;
                if self.workspace_state.workspace().layout.item_count() == 0 {
                    self.create_terminal_in_project(
                        self.workspace_state.active_project,
                        window,
                        cx,
                    );
                } else {
                    cx.notify();
                }
                true
            }
            "escape" => {
                self.project_picker = false;
                cx.notify();
                true
            }
            _ => false,
        }
    }
    pub(in crate::app) fn filtered_terminal_entries(
        &self,
        cx: &App,
    ) -> Vec<(SessionId, String, String)> {
        let query = self.command_palette_query(cx);
        let manager = self.window_manager.read(cx);
        let mut entries = Vec::new();
        for project in &self.workspace_state.projects {
            for window in manager.state.windows.values() {
                if let Some(view) = window.views.get(&project.id) {
                    for item in view.layout.terminal_items() {
                        if let Some(id) = item.session_id {
                            let label = format!("{} · {}", project.name, item.display_title());
                            let location = self.window_label(window.id, cx);
                            if command_matches(&format!("{label} {location}"), &query) {
                                entries.push((id, label, location));
                            }
                        }
                    }
                }
            }
            for (&id, tab) in &manager.state.background {
                if tab.project_id == project.id {
                    let label = format!("{} · {}", project.name, tab.item.display_title());
                    if command_matches(&label, &query) {
                        entries.push((
                            id,
                            label,
                            if self.language == Language::SimplifiedChinese {
                                "后台".into()
                            } else {
                                "Background".into()
                            },
                        ));
                    }
                }
            }
        }
        entries
    }

    pub(in crate::app) fn filtered_window_entries(&self, cx: &App) -> Vec<(WindowId, String)> {
        let query = self.command_palette_query(cx);
        let moving = self
            .command_palette
            .as_ref()
            .is_some_and(|p| matches!(p.mode, PaletteMode::MoveToWindow(_)));
        self.window_manager
            .read(cx)
            .state
            .windows
            .keys()
            .copied()
            .filter(|id| !moving || *id != self.window_id)
            .map(|id| (id, self.window_label(id, cx)))
            .filter(|(_, label)| command_matches(label, &query))
            .collect()
    }

    pub(in crate::app) fn render_project_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .min_w_0()
            .px_4()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .child(if self.language == Language::SimplifiedChinese {
                "选择项目"
            } else {
                "Choose a project"
            })
            .child(
                div()
                    .id("window-project-list")
                    .w_full()
                    .max_w(px(340.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .max_h(px(440.))
                    .overflow_y_scroll()
                    .children(self.workspace_state.projects.iter().map(|project| {
                        let id = project.id;
                        div()
                            .id(format!("window-project-{id}"))
                            .w_full()
                            .px_4()
                            .py_3()
                            .rounded_xl()
                            .bg(rgb(self.colors.panel_alt))
                            .when(id == self.workspace_state.active_project, |e| {
                                e.border_1().border_color(rgb(self.colors.accent))
                            })
                            .cursor_pointer()
                            .hover(|e| e.bg(rgb(self.colors.hover)))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.project_picker = false;
                                app.activate_project(id, cx);
                                if app
                                    .workspace_state
                                    .workspace()
                                    .layout
                                    .terminal_items()
                                    .next()
                                    .is_none()
                                {
                                    app.create_terminal_in_project(id, window, cx);
                                } else {
                                    cx.notify();
                                }
                            }))
                            .child(project.name.clone())
                    })),
            )
            .into_any_element()
    }
}

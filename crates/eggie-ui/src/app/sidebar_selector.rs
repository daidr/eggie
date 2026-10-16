use super::*;

impl EggieApp {
    pub(super) fn dismiss_project_selector(&mut self, cx: &mut Context<Self>) {
        if let Some(selector) = self.sidebar.project_selector.take() {
            let _ = selector
                .window
                .update(cx, |popup, window, cx| popup.dismiss(window, cx));
            cx.notify();
        }
    }

    pub(super) fn select_sidebar_project(
        &mut self,
        project: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_project_selector(cx);
        self.dismiss_project_popover(cx);
        self.dismiss_terminal_preview(cx);
        self.activate_project(project, cx);
        self.sidebar
            .project_drag
            .scroll
            .set_offset(point(px(0.), px(0.)));
        self.focus_handle.focus(window, cx);
    }

    pub(super) fn project_selector_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(selector) = &self.sidebar.project_selector else {
            return false;
        };
        let projects = &self.workspace_state.projects;
        let selected = selector.keyboard_target.get();
        let index = projects
            .iter()
            .position(|project| project.id == selected)
            .unwrap_or(0);
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_project_selector(cx),
            "enter" => self.select_sidebar_project(selected, window, cx),
            "up" | "down" | "home" | "end" => {
                let next = match event.keystroke.key.as_str() {
                    "up" => index.saturating_sub(1),
                    "down" => (index + 1).min(projects.len().saturating_sub(1)),
                    "home" => 0,
                    _ => projects.len().saturating_sub(1),
                };
                if let Some(project) = projects.get(next) {
                    selector.keyboard_target.set(project.id);
                    selector.scroll.scroll_to_item(next);
                    let _ = selector.window.update(cx, |_, _, cx| cx.notify());
                }
            }
            _ => {} // Menu keystrokes must never reach the terminal underneath.
        }
        true
    }

    pub(super) fn toggle_project_selector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar.project_selector.is_some() {
            self.dismiss_project_selector(cx);
            return;
        }
        let Some(anchor) = self.sidebar.project_selector_bounds.get() else {
            return;
        };
        self.dismiss_project_popover(cx);
        self.dismiss_terminal_preview(cx);
        self.focus_handle.focus(window, cx);
        let parent = Window::window_handle(window)
            .downcast::<Self>()
            .expect("project selector owner");
        let projects = self.project_store.clone();
        let settings = self.settings.clone();
        let selected = self.workspace_state.active_project;
        let keyboard_target = Rc::new(std::cell::Cell::new(selected));
        let target = keyboard_target.clone();
        let scroll = ScrollHandle::new();
        if let Some(index) = self
            .workspace_state
            .projects
            .iter()
            .position(|project| project.id == selected)
        {
            scroll.scroll_to_item(index);
        }
        let popup_scroll = scroll.clone();
        let height = (self.workspace_state.projects.len().min(9) + 1) as f32 * 32. + 11.;
        let colors = self.colors;
        let result = PopupWindow::open(
            cx,
            window,
            PopupWindowOptions {
                anchor,
                content_size: size(anchor.size.width.max(px(180.)), px(height)),
                placement: PopupPlacement::Below,
                background: colors.panel,
                border: colors.border,
            },
            move |_, cx| {
                let language = settings.read(cx).config().language;
                let mut list = div()
                    .id("sidebar-project-options")
                    .size_full()
                    .flex()
                    .flex_col()
                    .py_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&popup_scroll)
                    .text_size(px(12.))
                    .text_color(rgb(colors.text));
                for project in projects.read(cx).projects() {
                    let id = project.id;
                    let icons = parent.read(cx).ok().and_then(|app| {
                        app.workspace_state.project_views.get(&id).and_then(|view| {
                            app.render_project_icon_stack(
                                id,
                                view,
                                ProjectIconLayout::SelectorOption,
                                cx,
                            )
                        })
                    });
                    list = list.child(
                        div()
                            .id(format!("sidebar-project-option-{id}"))
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_0()
                            .h(px(32.))
                            .mx_1()
                            .px_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba(0))
                            .when(id == selected, |row| row.bg(rgb(colors.panel_alt)))
                            .when(id == target.get(), |row| {
                                row.border_color(rgb(colors.accent))
                            })
                            .hover(move |row| row.bg(rgb(colors.hover)))
                            .cursor_pointer()
                            .on_click(move |_, popup, cx| {
                                popup.remove_window();
                                cx.defer(move |cx| {
                                    let _ = parent.update(cx, |app, window, cx| {
                                        app.sidebar.project_selector = None;
                                        app.select_sidebar_project(id, window, cx);
                                    });
                                });
                                cx.stop_propagation();
                            })
                            .children(icons)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .w(px(14.))
                                    .text_color(rgb(colors.accent))
                                    .child(if id == selected { "✓" } else { "" }),
                            ),
                    );
                }
                list.child(div().mx_2().h(px(1.)).flex_none().bg(rgb(colors.border)))
                    .child(
                        div()
                            .id("sidebar-project-option-add")
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .h(px(32.))
                            .mx_1()
                            .px_2()
                            .rounded_md()
                            .text_color(rgb(colors.muted))
                            .cursor_pointer()
                            .hover(move |row| row.bg(rgb(colors.hover)))
                            .on_click(move |_, popup, cx| {
                                popup.remove_window();
                                cx.defer(move |cx| {
                                    let _ = parent.update(cx, |app, window, cx| {
                                        app.sidebar.project_selector = None;
                                        app.prompt_add_project(window, cx);
                                    });
                                });
                            })
                            .child(icon_sized(IconName::Add, 14.))
                            .child(language.add_project_tooltip()),
                    )
                    .into_any_element()
            },
            |_, _, _| {},
        );
        match result {
            Ok(window) => {
                self.sidebar.project_selector = Some(ProjectSelectorState {
                    window,
                    keyboard_target,
                    scroll,
                });
                cx.notify();
            }
            Err(error) => eprintln!("failed to open project selector: {error:#}"),
        }
    }

    pub(super) fn render_single_project_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.workspace_state.active_project_data();
        let id = project.id;
        let view = self.workspace_state.workspace();
        let bounds = self.sidebar.project_selector_bounds.clone();
        let header = div().flex().items_center().mx_2().mb_3().child(
            div()
                .flex_1()
                .min_w_0()
                .on_children_prepainted(move |children, _, _| bounds.set(children.first().copied()))
                .child(
                    div()
                        .id("sidebar-project-selector")
                        .flex()
                        .items_center()
                        .min_w_0()
                        .h(px(34.))
                        .pl_1()
                        .pr_2()
                        .rounded_lg()
                        .bg(rgb(self.colors.panel_alt))
                        .border_1()
                        .border_color(rgb(self.colors.border))
                        .tooltip(tooltip_text(self.language.select_project(), self.colors))
                        .cursor_pointer()
                        .hover(|row| row.bg(rgb(self.colors.hover)))
                        .on_click(
                            cx.listener(|app, _, window, cx| {
                                app.toggle_project_selector(window, cx)
                            }),
                        )
                        .on_mouse_down_out(
                            cx.listener(|app, _, _, cx| app.dismiss_project_selector(cx)),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |app, _, window, cx| {
                                app.dismiss_project_selector(cx);
                                app.show_project_context_menu(id, window, cx);
                            }),
                        )
                        .children(self.render_project_icon_stack(
                            id,
                            view,
                            ProjectIconLayout::SelectorTrigger,
                            cx,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(project.name.clone()),
                        )
                        .child(icon_sized(IconName::ArrowDown, 12.)),
                ),
        );
        let empty = view.layout.terminal_items().next().is_none()
            && self
                .window_manager
                .read(cx)
                .state
                .project_session_ids(id)
                .is_empty();
        div()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.left_sidebar_width))
            .h_full()
            .overflow_hidden()
            .bg(rgb(self.colors.panel))
            .child(self.render_window_drag_region(cx))
            .child(header)
            .child(
                div()
                    .id("single-project-terminal-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar.project_drag.scroll)
                    .child(self.render_sidebar_project_items(id, view, false, cx))
                    .when(empty, |list| {
                        list.child(
                            div()
                                .mx_2()
                                .py_3()
                                .flex()
                                .flex_col()
                                .text_size(px(12.))
                                .text_color(rgb(self.colors.muted))
                                .child(
                                    div()
                                        .id("single-project-create-terminal")
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .px_2()
                                        .py_2()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|row| row.bg(rgb(self.colors.hover)))
                                        .on_click(cx.listener(move |app, _, window, cx| {
                                            app.create_terminal_in_project(id, window, cx)
                                        }))
                                        .child(icon_sized(IconName::Add, 14.))
                                        .child(self.language.new_terminal_tooltip()),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

use super::*;

impl EggieApp {
    pub(in crate::app) fn render_terminal_status_tag(
        &self,
        label: &'static str,
        tint: u32,
    ) -> AnyElement {
        div()
            .flex_none()
            .h(px(16.))
            .px_1()
            .rounded_full()
            .bg(rgba((tint << 8) | 0x30))
            .text_size(px(10.))
            .text_color(rgb(self.colors.text))
            .child(label)
            .into_any_element()
    }

    pub(in crate::app) fn render_exited_terminal_tag(&self) -> AnyElement {
        self.render_terminal_status_tag(
            if self.language == Language::SimplifiedChinese {
                "已结束"
            } else {
                "Exited"
            },
            self.colors.muted,
        )
    }

    pub(super) fn start_project_drag(
        &mut self,
        id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_project_popover(cx);
        self.dismiss_sidebar_preview(cx);
        self.sidebar.project_drag.source = Some(id);
        self.sidebar.project_drag.target = None;
        self.sidebar.project_drag.bounds = None;
        self.sidebar.project_drag.revision += 1;
        let revision = self.sidebar.project_drag.revision;
        let app = cx.entity().downgrade();
        cx.spawn_in(window, async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let keep_running = cx
                    .update(|window, cx| {
                        app.update(cx, |app, cx| {
                            if app.sidebar.project_drag.revision != revision
                                || app.sidebar.project_drag.source.is_none()
                            {
                                return false;
                            }
                            if !cx.has_active_drag() {
                                app.clear_project_drag(cx);
                                return false;
                            }
                            if let Some(bounds) = app.sidebar.project_drag.bounds {
                                let pointer = window.mouse_position();
                                let delta = project_drag::edge_scroll_delta(bounds, pointer);
                                let scroll = &app.sidebar.project_drag.scroll;
                                let offset = scroll.offset();
                                let next_y =
                                    (offset.y + delta).clamp(-scroll.max_offset().y, px(0.));
                                if next_y != offset.y {
                                    scroll.set_offset(point(offset.x, next_y));
                                    cx.notify();
                                }
                                app.update_project_drop_target(pointer, bounds, cx);
                            }
                            true
                        })
                        .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(super) fn clear_project_drag(&mut self, cx: &mut Context<Self>) {
        if self.sidebar.project_drag.source.take().is_some() {
            self.sidebar.project_drag.target = None;
            self.sidebar.project_drag.bounds = None;
            self.sidebar.project_drag.revision += 1;
            cx.notify();
        }
    }

    pub(super) fn update_project_drop_target(
        &mut self,
        pointer: Point<Pixels>,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.sidebar.project_drag.bounds = Some(bounds);
        let target = bounds.contains(&pointer).then(|| {
            let rows = self.sidebar.project_row_bounds.borrow();
            project_drag::insertion_target(
                self.workspace_state.projects.iter().filter_map(|project| {
                    rows.get(&project.id).map(|bounds| (project.id, *bounds))
                }),
                pointer.y,
            )
        });
        if self.sidebar.project_drag.target != target {
            self.sidebar.project_drag.target = target;
            cx.notify();
        }
    }

    pub(super) fn drop_project(
        &mut self,
        project: &DraggedProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if project.window_id != self.window_id {
            return;
        }
        if let Some(bounds) = self.sidebar.project_drag.bounds {
            self.update_project_drop_target(window.mouse_position(), bounds, cx);
        }
        let target = self.sidebar.project_drag.target;
        self.clear_project_drag(cx);
        if let Some(target) = target {
            self.project_store.update(cx, |store, cx| {
                store.move_before(project.id, target.before, cx);
            });
            self.reconcile_project_views(cx);
            cx.notify();
        }
    }

    pub(super) fn sidebar_sessions_in_layout_order<'a>(
        &'a self,
        project_id: ProjectId,
        view: &'a WindowProjectView,
    ) -> Vec<&'a SessionSummary> {
        // Reading-order position of each session within the layout tree.
        let mut layout_rank: HashMap<SessionId, usize> = HashMap::new();
        for item in view.layout.terminal_items() {
            if let Some(session_id) = item.session_id {
                let next_rank = layout_rank.len();
                layout_rank.entry(session_id).or_insert(next_rank);
            }
        }
        let orphan_rank = layout_rank.len();

        let mut sessions: Vec<&SessionSummary> = self
            .session_feed
            .sessions
            .iter()
            .filter(|session| session.project_id == project_id)
            .collect();

        // Stable sort keeps orphan sessions (rank = len) in their original relative order, appended
        // after everything the layout knows about.
        sessions
            .sort_by_key(|session| layout_rank.get(&session.id).copied().unwrap_or(orphan_rank));
        sessions
    }

    pub(super) fn render_directory_actions(
        &self,
        field: DirectoryField,
        path: &Path,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let language = self.language;
        let opener = self.settings.read(cx).config().open_directory_with;
        let slug = field.slug();

        let copy_value = path.display().to_string();
        let copy_button = div()
            .id(SharedString::from(format!("copy-path-{slug}")))
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(24.))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(colors.border))
            .text_color(rgb(colors.muted))
            .cursor_pointer()
            .hover(move |element| {
                element
                    .bg(rgb(colors.panel_alt))
                    .text_color(rgb(colors.text))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_value.clone()));
            }))
            .child(icon_sized(IconName::Copy, 14.))
            .child(div().text_size(px(12.)).child(language.copy_path()));

        let open_path = path.to_path_buf();
        // Left half: launch the persisted opener on the directory.
        let open_action = div()
            .id(SharedString::from(format!("open-with-{slug}")))
            .flex()
            .flex_1()
            .items_center()
            .gap_1()
            .h_full()
            .px_2()
            .cursor_pointer()
            .hover(move |element| element.bg(rgb(colors.panel_alt)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, _| {
                open_path_with(&open_path, opener);
            }))
            .child(icon_sized(icon_for_open_with(opener), 14.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.))
                    .child(open_with_label(language, opener)),
            );

        // Right half: a caret that toggles this row's opener dropdown.
        let caret = div()
            .id(SharedString::from(format!("open-with-caret-{slug}")))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(22.))
            .h_full()
            .border_l_1()
            .border_color(rgb(colors.border))
            .cursor_pointer()
            .hover(move |element| element.bg(rgb(colors.panel_alt)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |app, _, _, cx| {
                app.open_with_dropdown = if app.open_with_dropdown == Some(field) {
                    None
                } else {
                    Some(field)
                };
                cx.notify();
            }))
            .child(icon_sized(IconName::ArrowDown, 12.));

        let control = cx.entity().downgrade();
        let mut open_button = div()
            // Capture the split-button's bounds so the dropdown can anchor just below it.
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let control = control.clone();
                cx.defer(move |cx| {
                    control
                        .update(cx, |app, cx| {
                            if app.open_with_dropdown_bounds.get(&field) != Some(&bounds) {
                                app.open_with_dropdown_bounds.insert(field, bounds);
                                if app.open_with_dropdown == Some(field) {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(24.))
                    .rounded_lg()
                    .overflow_hidden()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .text_color(rgb(colors.muted))
                    .child(open_action)
                    .child(caret),
            );
        if let Some(dropdown) = self.render_open_with_dropdown(field, cx) {
            open_button = open_button.child(dropdown);
        }

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(copy_button)
            .child(open_button)
            .into_any_element()
    }

    /// A button that opens a prompt for a port forward spec, for a remote session. The spec is a
    /// single line: `L 8080 localhost:80` for local, `R 8080 localhost:80` for remote, or
    /// `D 1080` for dynamic.
    pub(super) fn render_forward_port_button(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let language = self.language;
        div()
            .id(format!("forward-port-{session_id}"))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(px(28.))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(colors.border))
            .text_size(px(12.))
            .text_color(rgb(colors.muted))
            .cursor_pointer()
            .hover(move |element| {
                element
                    .bg(rgb(colors.panel_alt))
                    .text_color(rgb(colors.text))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |app, _, window, cx| {
                app.prompt_forward_port(session_id, window, cx);
            }))
            .child(language.forward_port())
            .into_any_element()
    }

    /// The dropdown listing the available [`OpenWith`] apps for `field`, anchored below its split
    /// button. Selecting an option persists it as the new default opener. Same anchored + deferred +
    /// `.occlude()` overlay pattern as the settings dropdowns.
    pub(super) fn render_open_with_dropdown(
        &self,
        field: DirectoryField,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.open_with_dropdown != Some(field) {
            return None;
        }
        let colors = self.colors;
        let language = self.language;
        let selected = self.settings.read(cx).config().open_directory_with;
        let slug = field.slug();

        let mut list = div().flex().flex_col().py_1();
        for option in OpenWith::ALL {
            let is_selected = option == selected;
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "open-with-{slug}-{}",
                        option.slug()
                    )))
                    .flex()
                    .items_center()
                    .gap_2()
                    .mx_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(is_selected, |element| {
                        element
                            .bg(rgb(colors.panel_alt))
                            .text_color(rgb(colors.accent))
                    })
                    .hover(move |element| element.bg(rgb(colors.panel_alt)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_open_directory_with(option, cx);
                        app.open_with_dropdown = None;
                        cx.notify();
                    }))
                    .child(icon_sized(icon_for_open_with(option), 14.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(open_with_label(language, option)),
                    ),
            );
        }

        let popover = div()
            .flex()
            .flex_col()
            .w(px(160.))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            // Floating overlay: block clicks on blank areas from bubbling through (see
            // [[gpui-overlay-occlude]]).
            .occlude()
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |app, _, cx| {
                    if app.open_with_dropdown == Some(field) {
                        app.open_with_dropdown = None;
                        cx.notify();
                    }
                });
            }))
            .child(list);

        // Anchor the dropdown just below the split button (top-left corner to the button's
        // bottom-left), falling back to a window-snapped corner until bounds are captured once.
        let anchored_dropdown = match self.open_with_dropdown_bounds.get(&field).copied() {
            Some(bounds) => anchored()
                .anchor(Anchor::TopLeft)
                .position(point(bounds.left(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(popover),
            None => anchored()
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(popover),
        };

        Some(deferred(anchored_dropdown).priority(2).into_any_element())
    }

    /// Persist a new default directory opener. No-op (and no save) if it is already selected.
    pub(super) fn set_open_directory_with(&mut self, opener: OpenWith, cx: &mut Context<Self>) {
        if self.settings.read(cx).config().open_directory_with == opener {
            return;
        }
        self.settings.update(cx, |settings, cx| {
            settings.update(|config| config.open_directory_with = opener, cx);
        });
    }

    pub(super) fn render_project_icon_stack(
        &self,
        project_id: ProjectId,
        view: &WindowProjectView,
        layout: ProjectIconLayout,
        cx: &App,
    ) -> Option<AnyElement> {
        let mut desired: Vec<_> = view
            .layout
            .items()
            .take(PROJECT_MAX_ICON_COUNT)
            .map(|item| ProjectIconCard {
                id: Some(item.id),
                icon: self.icon_for_item(item.kind, item.session_id),
            })
            .collect();
        for window in self
            .window_manager
            .read(cx)
            .state
            .windows
            .values()
            .filter(|w| w.id != self.window_id)
        {
            if let Some(view) = window.views.get(&project_id) {
                desired.extend(view.layout.terminal_items().map(|item| ProjectIconCard {
                    id: Some(item.id),
                    icon: self.icon_for_item(item.kind, item.session_id),
                }));
            }
        }
        desired.extend(
            self.window_manager
                .read(cx)
                .state
                .background
                .values()
                .filter(|tab| tab.project_id == project_id)
                .map(|tab| ProjectIconCard {
                    id: Some(tab.item.id),
                    icon: self.icon_for_item(tab.item.kind, tab.item.session_id),
                }),
        );
        desired.truncate(PROJECT_MAX_ICON_COUNT);
        if desired.is_empty() {
            desired.push(ProjectIconCard {
                id: None,
                icon: ProcessIcon::ProjectPlaceholder,
            });
        }
        let state = {
            let now = Instant::now();
            let mut states = self.sidebar.project_icon_states.borrow_mut();
            let state = states
                .entry(project_id)
                .or_insert_with(|| ProjectIconState::new(desired.clone(), now));
            state.retarget(desired, now);
            state.clone()
        };
        let animating = state.started_at.elapsed() < PROJECT_ICON_TRANSITION;
        let image_width = process_icons::stack_width(PROJECT_MAX_ICON_COUNT, PROJECT_CARD_SIZE);
        let (image_left, slot_width) = match layout {
            ProjectIconLayout::Sidebar => {
                // Keep the first card at the same left edge as a single icon.
                let left = image_width - process_icons::stack_width(1, PROJECT_CARD_SIZE);
                (left, image_width + 16.)
            }
            ProjectIconLayout::SelectorTrigger => {
                // Let the label follow the actual stack, with a small, consistent gap.
                let width = process_icons::stack_width(state.icons.len(), PROJECT_CARD_SIZE) + 6.;
                (0., width)
            }
            ProjectIconLayout::SelectorOption => (0., image_width + 6.),
        };
        let card_height = PROJECT_CARD_SIZE + 5.;
        if !animating {
            let icons = state.icons.iter().map(|card| card.icon).collect::<Vec<_>>();
            let image = process_icons::card_stack_frame(&icons, self.use_dark, PROJECT_CARD_SIZE)?;
            let content_width = process_icons::stack_width(icons.len(), PROJECT_CARD_SIZE);
            return Some(
                div()
                    .flex_none()
                    .w(px(slot_width))
                    .h(px(card_height))
                    .child(
                        img(image)
                            .ml(px(image_left))
                            .w(px(content_width))
                            .h(px(card_height)),
                    )
                    .into_any_element(),
            );
        }
        let dark = self.use_dark;
        Some(
            div()
                .relative()
                .flex_none()
                .overflow_hidden()
                .w(px(slot_width))
                .h(px(card_height))
                .with_animation(
                    format!("project-icons-{project_id}-{}", state.revision),
                    Animation::new(PROJECT_ICON_TRANSITION),
                    move |element, progress| {
                        // All presentations use the state clock, including during a retarget.
                        let frame = state.frame(if progress >= 1. {
                            state.started_at + PROJECT_ICON_TRANSITION
                        } else {
                            Instant::now()
                        });
                        let width = if matches!(layout, ProjectIconLayout::SelectorTrigger) {
                            frame.width + 6.
                        } else {
                            slot_width
                        };
                        let image = process_icons::card_layers_frame(
                            &frame.layers(),
                            dark,
                            PROJECT_CARD_SIZE,
                            image_width,
                        );
                        element.w(px(width)).child(
                            img(image)
                                .absolute()
                                .left(px(image_left))
                                .top_0()
                                .w(px(image_width))
                                .h(px(card_height)),
                        )
                    },
                )
                .into_any_element(),
        )
    }

    pub(super) fn sidebar_status_row_transition(
        &self,
        session_id: SessionId,
        expanded: bool,
    ) -> Option<(f32, f32, u64)> {
        let now = Instant::now();
        let target_height = if expanded {
            SIDEBAR_STATUS_ROW_HEIGHT
        } else {
            SIDEBAR_SESSION_ROW_HEIGHT
        };
        let mut states = self.sidebar.sidebar_status_rows.borrow_mut();
        let state = states.entry(session_id).or_insert(SidebarStatusRowState {
            expanded,
            from_height: target_height,
            revision: 0,
            started_at: now,
        });
        if state.expanded != expanded {
            state.from_height = state.current_height(now);
            state.expanded = expanded;
            state.revision += 1;
            state.started_at = now;
        }
        (state.started_at.elapsed() < SIDEBAR_STATUS_TRANSITION
            && (state.from_height - target_height).abs() > f32::EPSILON)
            .then_some((state.from_height, target_height, state.revision))
    }

    pub(super) fn render_sidebar_project_items(
        &self,
        project_id: ProjectId,
        view: &WindowProjectView,
        nested: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_active_project = project_id == self.workspace_state.active_project;
        let nonlocal_rows = self.render_nonlocal_terminals(project_id, cx);
        let mut terminal_list = div()
            .flex()
            .flex_col()
            .min_w_0()
            .mb_2()
            .when(nested, |list| {
                // The list owns its hierarchy inset so every terminal row shares one left edge.
                list.ml(px(28.))
                    .mr_3()
                    .pl(px(12.))
                    .border_l_1()
                    .border_color(rgba((self.colors.border << 8) | 0x80))
            })
            .when(!nested, |list| list.mx_2());
        if !nonlocal_rows.is_empty() && view.layout.terminal_items().next().is_some() {
            terminal_list = terminal_list.child(self.render_terminal_section_label(
                if self.language == Language::SimplifiedChinese {
                    "此窗口".into()
                } else {
                    "This window".into()
                },
            ));
        }
        for session in self.sidebar_sessions_in_layout_order(project_id, view) {
            let active = is_active_project && self.active_session_id() == Some(session.id);
            let session_id = session.id;
            // Prefer the tab's custom title (if any) so the sidebar stays consistent with
            // the tab bar; fall back to the session's process title.
            let tab_title = view
                .layout
                .terminal_display_title(session_id)
                .unwrap_or(session.title.as_str());
            let exited = matches!(session.status, SessionStatus::Exited { .. });
            let program_status = session.program_status.as_ref();
            let display_title = view
                .layout
                .terminal_custom_title(session_id)
                .or_else(|| program_status.and_then(|status| status.title.as_deref()))
                .unwrap_or(tab_title);
            let title_key = format!("sidebar-title-{session_id}");
            let subtitle_key = format!("sidebar-subtitle-{session_id}");
            let marquee_hovered = self
                .sidebar
                .marquee_states
                .borrow()
                .get(&title_key)
                .is_some_and(|state| state.hovered);
            let label_background = if marquee_hovered {
                self.colors.hover
            } else if active {
                self.colors.panel_alt
            } else {
                self.colors.panel
            };
            let label: AnyElement = if exited {
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(self.render_marquee(
                        title_key.clone(),
                        display_title.to_owned(),
                        12.,
                        17.,
                        label_background,
                        cx,
                    ))
                    .child(self.render_exited_terminal_tag())
                    .into_any_element()
            } else if let Some(status) = program_status {
                let state_label = program_state_label(status.state, self.language);
                let mut subtitle = match (&status.title, &status.message) {
                    (Some(title), Some(message)) if title != display_title => {
                        format!("{title}: {message}")
                    }
                    (_, Some(message)) => message.clone(),
                    (Some(title), None) if title != display_title => title.clone(),
                    _ => state_label.to_owned(),
                };
                if let Some(progress) = status.progress {
                    subtitle.push_str(&format!(" · {progress}%"));
                }
                let tag = matches!(
                    status.state,
                    TerminalProgramState::Blocked
                        | TerminalProgramState::Done
                        | TerminalProgramState::Error
                )
                .then_some((status.state, state_label));
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(self.render_marquee(
                                title_key.clone(),
                                display_title.to_owned(),
                                12.,
                                17.,
                                label_background,
                                cx,
                            ))
                            .when_some(tag, |row, (state, label)| {
                                let tint = match state {
                                    TerminalProgramState::Blocked => {
                                        self.interface_theme.palette[3]
                                    }
                                    TerminalProgramState::Error => self.interface_theme.palette[1],
                                    _ => self.interface_theme.palette[2],
                                };
                                row.child(self.render_terminal_status_tag(label, tint))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .w_full()
                            .min_w_0()
                            .text_color(rgb(self.colors.muted))
                            .child(self.render_marquee(
                                subtitle_key.clone(),
                                subtitle,
                                11.,
                                15.,
                                label_background,
                                cx,
                            )),
                    )
                    .into_any_element()
            } else {
                self.render_marquee(
                    title_key.clone(),
                    display_title.to_owned(),
                    12.,
                    17.,
                    label_background,
                    cx,
                )
            };
            let height_transition =
                self.sidebar_status_row_transition(session_id, !exited && program_status.is_some());
            let row = div()
                .id(format!("sidebar-session-{session_id}"))
                .flex()
                .items_center()
                .min_w_0()
                .overflow_hidden()
                .gap_2()
                .pl_1()
                .pr_2()
                .rounded_md()
                .mb_1()
                .text_size(px(12.))
                .text_color(rgb(if active {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(active, |element| element.bg(rgb(self.colors.panel_alt)))
                .when_some(self.bell_flash_tint(session_id), |element, tint| {
                    element.bg(rgba(tint))
                })
                .hover({
                    let colors = self.colors;
                    move |element| element.bg(rgb(colors.hover))
                })
                .on_hover(cx.listener(move |app, hovered: &bool, window, cx| {
                    app.set_marquee_hover(
                        &[
                            format!("sidebar-title-{session_id}"),
                            format!("sidebar-subtitle-{session_id}"),
                        ],
                        *hovered,
                        cx,
                    );
                    app.sidebar_preview_hover(session_id, *hovered, window, cx);
                }))
                .cursor_pointer()
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.dismiss_sidebar_preview(cx);
                    app.activate_session(project_id, session_id, window, cx)
                }))
                .child(self.render_item_icon(
                    ItemKind::Terminal,
                    Some(session_id),
                    format!("sidebar-progress-{session_id}"),
                ))
                .child(label);
            let row_bounds = self.sidebar.sidebar_row_bounds.clone();
            terminal_list = terminal_list.child(
                div()
                    .on_children_prepainted(move |children, _, _| {
                        if let Some(bounds) = children.first().copied() {
                            row_bounds.borrow_mut().insert(session_id, bounds);
                        }
                    })
                    .child(if let Some((from, to, revision)) = height_transition {
                        row.with_animation(
                            format!("sidebar-status-height-{session_id}-{revision}"),
                            Animation::new(SIDEBAR_STATUS_TRANSITION)
                                .with_easing(gpui::ease_in_out),
                            move |element, progress| element.h(px(from + (to - from) * progress)),
                        )
                        .into_any_element()
                    } else {
                        row.h(px(if !exited && program_status.is_some() {
                            SIDEBAR_STATUS_ROW_HEIGHT
                        } else {
                            SIDEBAR_SESSION_ROW_HEIGHT
                        }))
                        .into_any_element()
                    }),
            );
        }
        terminal_list = terminal_list
            .children(self.render_ended_terminals(project_id, cx))
            .children(nonlocal_rows);
        terminal_list.into_any_element()
    }

    pub(super) fn render_left_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.settings.read(cx).config().sidebar_layout == SidebarLayout::SingleProject {
            return self.render_single_project_sidebar(cx);
        }
        let panel = div()
            .flex()
            .flex_none()
            .flex_col()
            .w(px(self.left_sidebar_width))
            .h_full()
            .overflow_hidden()
            .bg(rgb(self.colors.panel))
            .child(self.render_window_drag_region(cx))
            .child(
                div().flex().flex_col().px_3().pb_3().gap_3().child(
                    div()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(section_label(self.language.projects_label(), self.colors))
                        .child(div().flex().items_center().gap_1().child(icon_button(
                            IconName::Add,
                            "add-project",
                            self.colors,
                            Some(self.language.add_project_tooltip().into()),
                            cx.listener(|app, _, window, cx| app.prompt_add_project(window, cx)),
                        )))
                        .children(self.render_update_popover(cx)),
                ),
            );
        let mut project_list = div()
            .id("sidebar-project-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .py(px(3.))
            .overflow_y_scroll()
            .track_scroll(&self.sidebar.project_drag.scroll)
            .on_drag_move::<DraggedProject>(cx.listener(
                |app, event: &DragMoveEvent<DraggedProject>, window, cx| {
                    let project = event
                        .dragged_item()
                        .downcast_ref::<DraggedProject>()
                        .unwrap();
                    if project.window_id == app.window_id {
                        cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                        app.update_project_drop_target(event.event.position, event.bounds, cx);
                    }
                },
            ))
            .on_drop(cx.listener(|app, project: &DraggedProject, window, cx| {
                app.drop_project(project, window, cx);
            }));
        for project in &self.workspace_state.projects {
            let project_id = project.id;
            let Some(view) = self.workspace_state.project_views.get(&project_id) else {
                continue;
            };
            let collapsed = self
                .workspace_state
                .collapsed_projects
                .contains(&project_id);
            let is_active_project = project_id == self.workspace_state.active_project;
            let has_terminals = view.layout.terminal_items().next().is_some()
                || !self
                    .window_manager
                    .read(cx)
                    .state
                    .project_session_ids(project_id)
                    .is_empty();
            let has_tabs = view.layout.items().next().is_some() || has_terminals;
            let row_bounds = self.sidebar.project_row_bounds.clone();
            let mut icons: Vec<_> = view
                .layout
                .items()
                .take(PROJECT_MAX_ICON_COUNT)
                .map(|item| self.icon_for_item(item.kind, item.session_id))
                .collect();
            if icons.is_empty() {
                icons.push(ProcessIcon::ProjectPlaceholder);
            }
            let dragged_project = DraggedProject {
                id: project_id,
                window_id: self.window_id,
                title: project.name.clone().into(),
                icon: process_icons::card_stack(&icons, self.use_dark, PROJECT_CARD_SIZE)
                    .expect("project cards always contain an icon"),
                icon_width: process_icons::stack_width(icons.len(), PROJECT_CARD_SIZE),
                icon_height: PROJECT_CARD_SIZE + 5.,
                width: self.left_sidebar_width - 24.,
                colors: self.colors,
            };
            let drag_app = cx.entity().downgrade();
            let mut project_block = div().relative().flex().flex_col().flex_none().child(
                div()
                    .on_children_prepainted(move |children, _, _| {
                        if let Some(bounds) = children.first().copied() {
                            row_bounds.borrow_mut().insert(project_id, bounds);
                        }
                    })
                    .child(
                        div()
                            .id(format!("project-{project_id}"))
                            .flex()
                            .items_center()
                            .min_w_0()
                            .overflow_hidden()
                            .gap_0()
                            .mx_3()
                            .pr_2()
                            .py_1()
                            .rounded_md()
                            .mb_1()
                            .when(is_active_project, |element| {
                                element.bg(rgb(self.colors.panel_alt))
                            })
                            .hover({
                                let colors = self.colors;
                                move |element| element.bg(rgb(colors.hover))
                            })
                            .cursor_pointer()
                            .on_drag(dragged_project, move |project, _, window, cx| {
                                drag_app
                                    .update(cx, |app, cx| {
                                        app.start_project_drag(project.id, window, cx);
                                    })
                                    .ok();
                                cx.new(|_| project.clone())
                            })
                            .when(collapsed && has_tabs, |row| {
                                row.on_hover(cx.listener(move |app, hovered: &bool, window, cx| {
                                    app.project_popover_hover(
                                        project_id, true, *hovered, window, cx,
                                    );
                                }))
                            })
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.activate_project(project_id, cx);
                                app.focus_handle.focus(window, cx);
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |app, _, window, cx| {
                                    app.show_project_context_menu(project_id, window, cx)
                                }),
                            )
                            .children(self.render_project_icon_stack(
                                project_id,
                                view,
                                ProjectIconLayout::Sidebar,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(project.name.clone()),
                            )
                            .when(has_tabs, |row| {
                                row.child(
                                    div()
                                        .id(format!("project-add-{project_id}"))
                                        .w(px(22.))
                                        .h(px(24.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .text_color(rgb(self.colors.muted))
                                        .hover(|e| e.bg(rgb(self.colors.hover)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation()
                                        })
                                        .on_click(cx.listener(move |app, _, window, cx| {
                                            cx.stop_propagation();
                                            app.create_terminal_in_project(project_id, window, cx);
                                        }))
                                        .child(icon_sized(IconName::Add, 14.)),
                                )
                            })
                            .child(
                                div()
                                    .id(format!("project-end-icon-{project_id}"))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .w(px(24.))
                                    .h(px(24.))
                                    .ml_2()
                                    .rounded_md()
                                    .text_color(rgb(self.colors.muted))
                                    .when(has_terminals || has_tabs, |e| {
                                        e.on_click(cx.listener(move |app, _, _, cx| {
                                            cx.stop_propagation();
                                            app.toggle_project_collapsed(project_id, cx);
                                        }))
                                    })
                                    .when(!has_terminals && !has_tabs, |e| {
                                        e.on_click(cx.listener(move |app, _, window, cx| {
                                            cx.stop_propagation();
                                            app.create_terminal_in_project(project_id, window, cx);
                                        }))
                                    })
                                    .when(!has_terminals, |end| {
                                        end.cursor_pointer().hover({
                                            let colors = self.colors;
                                            move |end| {
                                                end.bg(rgb(colors.panel_alt))
                                                    .text_color(rgb(colors.accent))
                                            }
                                        })
                                    })
                                    .child(icon_sized(
                                        if !has_terminals {
                                            IconName::Add
                                        } else if collapsed {
                                            IconName::ArrowRight
                                        } else {
                                            IconName::ArrowDown
                                        },
                                        14.,
                                    )),
                            ),
                    ),
            );
            if !collapsed && has_terminals {
                project_block = project_block
                    .child(self.render_sidebar_project_items(project_id, view, true, cx));
            }
            project_list = project_list.child(
                project_block
                    .when(
                        self.sidebar.project_drag.source == Some(project_id),
                        |block| block.opacity(0.4),
                    )
                    .when(
                        self.sidebar.project_drag.target
                            == Some(ProjectDropTarget {
                                before: Some(project_id),
                            }),
                        |block| {
                            block.child(
                                div()
                                    .absolute()
                                    .top(px(-2.))
                                    .left_3()
                                    .right_3()
                                    .h(px(2.))
                                    .rounded_full()
                                    .bg(rgb(self.colors.accent)),
                            )
                        },
                    ),
            );
        }
        project_list = project_list.child(div().relative().flex_1().min_h(px(16.)).when(
            self.sidebar.project_drag.target == Some(ProjectDropTarget { before: None }),
            |end| {
                end.child(
                    div()
                        .absolute()
                        .top(px(-2.))
                        .left_3()
                        .right_3()
                        .h(px(2.))
                        .rounded_full()
                        .bg(rgb(self.colors.accent)),
                )
            },
        ));
        panel.child(project_list).into_any_element()
    }
}

use super::*;

impl EggieApp {
    pub(super) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.is_some() {
            self.close_command_palette(window, cx);
            return;
        }
        self.open_palette(PaletteMode::Commands, window, cx);
    }

    /// Open the profile picker: same modal as the command palette, listing profiles. Replaces an open
    /// command palette (the picker is reached *from* it, so the first modal must not linger).
    pub(super) fn open_profile_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_palette(PaletteMode::Profiles, window, cx);
    }

    /// Build the palette modal in `mode` and focus its filter input.
    pub(super) fn open_palette(
        &mut self,
        mode: PaletteMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let colors = self.colors;
        let placeholder = match mode {
            PaletteMode::Commands => self.language.command_palette_placeholder(),
            PaletteMode::Profiles => self.language.profile_picker_placeholder(),
            PaletteMode::ProjectDefaultProfile(_) => {
                self.language.project_default_profile_placeholder()
            }
        };
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
            input.set_placeholder(placeholder);
            input
        });
        let subscription = cx.subscribe_in(&input, window, Self::on_palette_input_event);
        let handle = input.read(cx).focus_handle();
        self.command_palette = Some(CommandPaletteUi {
            input,
            selected_index: 0,
            scroll_handle: ScrollHandle::new(),
            mode,
            _subscription: subscription,
        });
        handle.focus(window, cx);
        cx.notify();
    }

    /// Close the palette and return focus to the terminal so keyboard input keeps working.
    pub(super) fn close_command_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.command_palette.take().is_none() {
            return false;
        }
        self.focus_handle.focus(window, cx);
        cx.notify();
        true
    }

    /// Route events emitted by the palette's filter input.
    pub(super) fn on_palette_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                // A new query reshuffles the filtered list; reset the selection to the top.
                if let Some(palette) = &mut self.command_palette {
                    palette.selected_index = 0;
                    palette.scroll_handle.set_offset(point(px(0.), px(0.)));
                }
                cx.notify();
            }
            TextInputEvent::Confirm | TextInputEvent::ConfirmReverse => {
                self.run_selected_palette_command(window, cx);
            }
            TextInputEvent::Cancel => {
                self.close_command_palette(window, cx);
            }
        }
    }

    /// The palette's current filter query (empty if the palette is closed).
    pub(super) fn command_palette_query(&self, cx: &App) -> String {
        self.command_palette
            .as_ref()
            .map(|palette| palette.input.read(cx).content().trim().to_owned())
            .unwrap_or_default()
    }

    /// The palette entries matching the current query, in display order.
    pub(super) fn filtered_palette_entries(&self, cx: &App) -> Vec<PaletteEntry> {
        let settings = self.settings.read(cx);
        let config = settings.config();
        let query = self.command_palette_query(cx);
        palette_entries(config, &settings.lua_config().registered)
            .into_iter()
            .filter(|entry| command_matches(&entry.label, &query))
            .collect()
    }

    /// The profiles matching the current query, in store order (built-in Default first).
    pub(super) fn filtered_profiles(&self, cx: &App) -> Vec<Profile> {
        let query = self.command_palette_query(cx);
        self.profile_store
            .read(cx)
            .profiles()
            .iter()
            .filter(|profile| command_matches(&profile.name, &query))
            .cloned()
            .collect()
    }

    /// How many rows the open palette currently shows (0 when closed).
    pub(super) fn palette_row_count(&self, cx: &App) -> usize {
        match self.command_palette.as_ref().map(|palette| palette.mode) {
            Some(PaletteMode::Commands) => self.filtered_palette_entries(cx).len(),
            Some(PaletteMode::Profiles) | Some(PaletteMode::ProjectDefaultProfile(_)) => {
                self.filtered_profiles(cx).len()
            }
            None => 0,
        }
    }

    /// The open palette's rows, rendered uniformly regardless of mode: commands show their shortcut
    /// on the right, profiles their badge dot on the left and a "project default" marker on the right.
    pub(super) fn palette_rows(&self, cx: &App) -> Vec<PaletteRow> {
        match self.command_palette.as_ref().map(|palette| palette.mode) {
            Some(PaletteMode::Commands) => self
                .filtered_palette_entries(cx)
                .into_iter()
                .map(|entry| PaletteRow {
                    label: entry.label.to_owned(),
                    trailing: entry.keystroke,
                    badge_color: None,
                })
                .collect(),
            Some(mode @ (PaletteMode::Profiles | PaletteMode::ProjectDefaultProfile(_))) => {
                // Mark the default of whichever project the picker is about: the *named* project when
                // setting its default (it may not be the active one — the menu came from its sidebar
                // row), otherwise the active project, whose default a launch would use.
                let configured = match mode {
                    PaletteMode::ProjectDefaultProfile(project_id) => self
                        .workspace_state
                        .projects
                        .iter()
                        .find(|project| project.id == project_id)
                        .and_then(|project| project.default_profile_id),
                    _ => {
                        self.workspace_state
                            .active_project_data()
                            .default_profile_id
                    }
                };
                // An unset project default means "use the built-in Default", so mark that row instead
                // of leaving no row marked.
                let marked = self.profile_store.read(cx).resolve(configured).id;
                self.filtered_profiles(cx)
                    .into_iter()
                    .map(|profile| PaletteRow {
                        trailing: (profile.id == marked)
                            .then(|| self.language.profile_picker_project_default().to_owned()),
                        label: profile.name,
                        badge_color: profile.badge_color,
                    })
                    .collect()
            }
            None => Vec::new(),
        }
    }

    /// Move the palette selection by `delta`, clamped to the filtered list, and keep it in view.
    pub(super) fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.palette_row_count(cx);
        let Some(palette) = &mut self.command_palette else {
            return;
        };
        if count == 0 {
            palette.selected_index = 0;
            return;
        }
        let last = count - 1;
        let current = palette.selected_index.min(last) as isize;
        let next = (current + delta).clamp(0, last as isize) as usize;
        palette.selected_index = next;
        palette.scroll_handle.scroll_to_item(next);
        cx.notify();
    }

    /// Confirm the palette's current selection: dispatch the chosen command, or — in profile mode —
    /// create a terminal from the chosen profile.
    pub(super) fn run_selected_palette_command(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = &self.command_palette else {
            return;
        };
        // Clamp rather than read `selected_index` raw: the list can shrink under an open palette
        // without a query change or arrow key (a profile deleted in another window re-renders this one
        // through the store observer). Render already clamps the *highlight*, so reading the raw index
        // here would make Enter a silent no-op on a visibly highlighted row.
        let mode = palette.mode;
        let index = palette.selected_index;
        match mode {
            PaletteMode::Commands => {
                let mut entries = self.filtered_palette_entries(cx);
                if entries.is_empty() {
                    return;
                }
                let index = index.min(entries.len() - 1);
                let action = entries.swap_remove(index).action;
                // Order matters — the dispatch path is captured against the focused node, so focus
                // must be back on the workspace root first for window-level handlers to receive it.
                self.close_command_palette(window, cx);
                window.dispatch_action(action, cx);
            }
            PaletteMode::Profiles => {
                let mut profiles = self.filtered_profiles(cx);
                if profiles.is_empty() {
                    return;
                }
                let profile = profiles.swap_remove(index.min(profiles.len() - 1));
                // Close first so the new terminal receives focus, not the modal.
                self.close_command_palette(window, cx);
                let group_id = self.workspace_state.workspace().active_group_id;
                self.new_terminal_with_profile(group_id, None, None, profile, window, cx);
            }
            PaletteMode::ProjectDefaultProfile(project_id) => {
                let mut profiles = self.filtered_profiles(cx);
                if profiles.is_empty() {
                    return;
                }
                let profile = profiles.swap_remove(index.min(profiles.len() - 1));
                self.close_command_palette(window, cx);
                // Store the built-in Default as `None` rather than as its id: "unset" and "explicitly
                // the built-in Default" resolve identically, and `None` is the one that keeps following
                // the built-in if a future release ever changes what that is.
                let selected =
                    (profile.id != eggie_domain::DEFAULT_PROFILE_ID).then_some(profile.id);
                self.project_store.update(cx, |store, cx| {
                    store.set_default_profile(project_id, selected, cx);
                });
            }
        }
    }

    /// Render the command palette as a centered modal over the whole workspace, or `None` when it is
    /// closed. A dimmed backdrop catches clicks (and, via `.occlude()`, all other pointer events) to
    /// close it; the panel holds the filter input above a scrollable, filterable list. Serves both
    /// palette modes — the rows and the empty-state text differ, the chrome does not.
    pub(super) fn render_command_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let palette = self.command_palette.as_ref()?;
        let colors = self.colors;
        let rows = self.palette_rows(cx);
        let selected = palette.selected_index.min(rows.len().saturating_sub(1));
        let empty_text = match palette.mode {
            PaletteMode::Commands => self.language.command_palette_empty(),
            PaletteMode::Profiles | PaletteMode::ProjectDefaultProfile(_) => {
                self.language.profile_picker_empty()
            }
        };

        let mut list = div()
            .id("command-palette-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&palette.scroll_handle)
            .py_1();
        if rows.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .px_3()
                    .text_color(rgb(colors.muted))
                    .child(empty_text),
            );
        } else {
            for (index, entry) in rows.into_iter().enumerate() {
                let is_selected = index == selected;
                let mut row = div()
                    .id(("command-palette-row", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .flex_none()
                    .h(px(34.))
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .when(is_selected, |element| {
                        element
                            .bg(rgb(colors.panel_alt))
                            .text_color(rgb(colors.accent))
                    })
                    .hover(|element| element.bg(rgb(colors.panel_alt)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if let Some(palette) = &mut app.command_palette {
                            palette.selected_index = index;
                        }
                        app.run_selected_palette_command(window, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_1()
                            .min_w_0()
                            // A profile's accent dot, so its identity is recognizable before launch.
                            .children(entry.badge_color.map(|color| {
                                div().flex_none().size(px(8.)).rounded_full().bg(rgb(color))
                            }))
                            .child(div().flex_1().min_w_0().truncate().child(entry.label)),
                    );
                if let Some(keystroke) = entry.trailing {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(rgb(colors.muted))
                            .child(keystroke),
                    );
                }
                list = list.child(row);
            }
        }

        let panel = div()
            .id("command-palette-panel")
            .flex()
            .flex_col()
            .w(px(560.))
            .max_w(relative(0.9))
            .max_h(relative(0.5))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            // Swallow presses inside the panel so they don't bubble to the backdrop's close handler.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            // ↑/↓ move the selection. The text input never binds these keys, so they bubble here
            // uncontested; every other key falls through to the input.
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "up" => {
                        app.move_palette_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        app.move_palette_selection(1, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_none()
                    .h(px(40.))
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(colors.border))
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(colors.muted))
                            .child(icon(IconName::Search)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .text_size(px(14.))
                            .text_color(rgb(colors.text))
                            .child(palette.input.clone()),
                    ),
            )
            .child(list);

        let backdrop = div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            // Push the panel down from the top edge so it reads as a palette, not a dialog.
            .pt(px(96.))
            // Block every pointer event for the terminal content behind the modal.
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, cx| {
                    app.close_command_palette(window, cx);
                }),
            )
            .child(panel);
        Some(backdrop.into_any_element())
    }
}

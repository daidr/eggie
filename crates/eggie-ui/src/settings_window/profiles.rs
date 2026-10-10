use super::*;

impl SettingsWindow {
    pub(super) fn profile_input_list(&self) -> [&Entity<TextInput>; 8] {
        [
            &self.profile_inputs.name,
            &self.profile_inputs.shell_program,
            &self.profile_inputs.shell_args,
            &self.profile_inputs.cwd,
            &self.profile_inputs.env,
            &self.profile_inputs.remote_host,
            &self.profile_inputs.remote_user,
            &self.profile_inputs.remote_port,
        ]
    }

    /// Write an appearance override on the selected profile. `None` clears it back to "follow the
    /// global setting".
    pub(super) fn apply_profile_selector_choice(
        &mut self,
        kind: SelectorKind,
        choice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.selected_profile else {
            return;
        };
        self.profile_store.update(cx, |store, cx| {
            store.update(
                id,
                // Exhaustive on purpose: a catch-all arm would let a newly added profile selector
                // compile and then silently write nothing.
                |profile| match kind {
                    SelectorKind::ProfileTerminalDark => {
                        profile.appearance.terminal_dark_theme = choice
                    }
                    SelectorKind::ProfileTerminalLight => {
                        profile.appearance.terminal_light_theme = choice
                    }
                    SelectorKind::ProfileFont => profile.appearance.font_family = choice,
                    // Global selectors write to AppSettings; they never reach here.
                    SelectorKind::InterfaceDark
                    | SelectorKind::InterfaceLight
                    | SelectorKind::TerminalDark
                    | SelectorKind::TerminalLight
                    | SelectorKind::Font
                    | SelectorKind::FontBold
                    | SelectorKind::FontItalic
                    | SelectorKind::FontBoldItalic => {}
                },
                cx,
            );
        });
    }

    /// The profile the Profiles pane is editing, resolving a stale selection to the built-in Default.
    ///
    /// Never returns `None` once the store exists: the store's own invariant guarantees a built-in
    /// Default, and every stale selection (deleted in this window, or in another one) lands there
    /// rather than leaving the pane blank.
    pub(super) fn selected_profile_id(&self, cx: &App) -> eggie_domain::ProfileId {
        self.profile_store
            .read(cx)
            .resolve(self.selected_profile)
            .id
    }

    /// Re-seed the profile inputs from the store when the selection has moved.
    ///
    /// Called from `render`, which is what makes it correct across *both* ways the selection can
    /// change: a click in this window, and a profile deleted elsewhere. The second works because
    /// `render` reads the profile store, so GPUI tracks that entity for this window and every
    /// `ProfileStore` mutation (each ends in `persist`, which notifies) invalidates this view — the
    /// now-stale id then resolves to the built-in Default here.
    ///
    /// Guarded on `profile_inputs_for`, deliberately *not* on a "currently seeding" flag: GPUI's
    /// `cx.emit` queues the event in `pending_effects` and delivers it after this call returns
    /// (`vendor/zed-gpui/.../app/context.rs`), so any flag cleared at the end of this function would
    /// already be false when `Changed` arrives. That deferral is also why the write-through compares
    /// against `seeded_profile_text` rather than trying to suppress the echo here.
    pub(super) fn sync_profile_inputs(&mut self, cx: &mut Context<Self>) {
        let id = self.selected_profile_id(cx);
        if self.profile_inputs_for == Some(id) {
            return;
        }
        let Some(profile) = self.profile_store.read(cx).profile(id).cloned() else {
            return;
        };
        let language = self.settings.read(cx).config().language;
        let fields = crate::profile_editor::ProfileFields::from_profile(&profile);
        // Record the owner *before* seeding. `set_content` emits `Changed`, and although delivery is
        // deferred, it must find the inputs already attributed to this profile — otherwise the
        // write-through would attribute the seeded text to whichever profile was selected before.
        self.profile_inputs_for = Some(id);
        self.selected_profile = Some(id);
        let seeds: [(&Entity<TextInput>, String, &'static str); 8] = [
            (&self.profile_inputs.name, fields.name, ""),
            (
                &self.profile_inputs.shell_program,
                fields.shell_program,
                language.shell_program_placeholder(),
            ),
            (
                &self.profile_inputs.shell_args,
                fields.shell_args,
                language.shell_args_placeholder(),
            ),
            (
                &self.profile_inputs.cwd,
                fields.cwd,
                language.profile_cwd_placeholder(),
            ),
            (
                &self.profile_inputs.env,
                fields.env,
                language.profile_env_placeholder(),
            ),
            (
                &self.profile_inputs.remote_host,
                fields.remote_host,
                language.profile_remote_host_placeholder(),
            ),
            (&self.profile_inputs.remote_user, fields.remote_user, ""),
            (&self.profile_inputs.remote_port, fields.remote_port, ""),
        ];
        for (input, value, placeholder) in seeds {
            input.update(cx, |input, cx| {
                input.set_placeholder(placeholder);
                input.set_content(value, cx);
            });
        }
        // Snapshot what was just seeded. Read back from the inputs rather than reusing `fields`, so the
        // record is exactly what the write-through will read — if `set_content` ever normalized its
        // argument, comparing against the pre-normalization string would make every open look edited.
        self.seeded_profile_text = Some(self.profile_text(cx));
    }

    /// The current text of all eight profile inputs, in `profile_input_list` order.
    pub(super) fn profile_text(&self, cx: &App) -> [String; 8] {
        self.profile_input_list()
            .map(|input| input.read(cx).content().to_owned())
    }

    /// Write every profile input back to the selected profile.
    ///
    /// Reads all eight inputs on each edit rather than tracking which one fired: the alternative is
    /// eight near-identical handlers, and this one runs only on keystrokes in a settings pane.
    ///
    /// Ignores events that arrive while the inputs belong to a different profile than the current
    /// selection — that is a seed in flight, and applying it would copy one profile's values onto
    /// another.
    pub(super) fn on_profile_input_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            event,
            TextInputEvent::Changed | TextInputEvent::Confirm | TextInputEvent::ConfirmReverse
        ) {
            return;
        }
        let Some(id) = self.selected_profile else {
            return;
        };
        // The inputs must already be attributed to this profile. A mismatch means `sync_profile_inputs`
        // is mid-flight (its deferred `Changed` events are arriving), and the text belongs to neither
        // profile yet.
        if self.profile_inputs_for != Some(id) {
            return;
        }
        // Nothing to write unless the *text* differs from what seeding put there.
        //
        // Compared against the seeded text, not against the stored profile. `set_content` emits
        // `Changed` whose delivery GPUI defers past the render that seeded it, so this handler always
        // runs once per selection on text the user never touched — and comparing the *parsed* result to
        // the stored struct answers "changed" in exactly the case that must not be written, because the
        // text↔struct round trip is lossy for input the parser refuses. Verified against a real
        // `profiles.json`: with the struct comparison alone, merely opening this pane deleted a
        // hand-edited `"BAD KEY"` env entry from disk. Raw text is immune — seeded text equals itself.
        let current = self.profile_text(cx);
        let Some(seeded) = self.seeded_profile_text.clone() else {
            // Not seeded yet, so there is no baseline to diff against and nothing can be attributed to
            // the user. `sync_profile_inputs` always seeds before the pane can be interacted with.
            return;
        };
        if seeded == current {
            return;
        }

        let Some(stored) = self.profile_store.read(cx).profile(id).cloned() else {
            return;
        };
        // Apply only the fields whose *own* text changed, field by field.
        //
        // Rebuilding the whole struct from all eight inputs is wrong, even behind the guard above:
        // these inputs are seeded once per selection, so another surface writing the same profile
        // leaves them stale — and a whole-struct write then reverts that surface's edit. Reproduced
        // live: the Advanced → Shell pane sets `shell_program` to `/bin/bash`, then typing a single
        // character into this pane's *Name* field wrote `shell_program: ""` back over it, because the
        // Profiles pane's shell input still held the value from when it was seeded. Diffing per field
        // makes a stale input inert: it matches its own seed, so it is never applied.
        let mut edited = stored.clone();
        let changed = |index: usize| seeded[index] != current[index];
        // Indices follow `profile_input_list` order; `profile_text` builds `current` from it.
        if changed(0) {
            // An all-whitespace name would render as a blank row in the picker with nothing to click;
            // keep the stored name until the user types something real.
            let name = current[0].trim();
            if !name.is_empty() {
                edited.name = name.to_owned();
            }
        }
        if changed(1) {
            edited.shell_program = current[1].trim().to_owned();
        }
        if changed(2) {
            edited.shell_args = crate::profile_editor::parse_args(&current[2]);
        }
        if changed(3) {
            // Empty means "inherit" (active terminal's cwd, then the project root), which is `None` —
            // not `Some("")`, which would be a path that never exists.
            let cwd = current[3].trim();
            edited.cwd = (!cwd.is_empty()).then(|| std::path::PathBuf::from(cwd));
        }
        if changed(4) {
            let (mut env, _rejected) = crate::profile_editor::parse_env(&current[4]);
            // Carry back the entries the editor cannot show (a value containing a newline). They are
            // valid POSIX values, just not expressible one-per-line — without this, editing any other
            // variable would delete them. Parsed entries win on a key collision: that key is visible in
            // the editor, so what the user sees is what they get.
            for (key, value) in crate::profile_editor::unrepresentable_env(&stored.env) {
                env.entry(key).or_insert(value);
            }
            edited.env = env;
        }
        // The three remote fields build one value, so any of them moving re-derives it from all three.
        if changed(5) || changed(6) || changed(7) {
            edited.remote =
                crate::profile_editor::parse_remote(&current[5], &current[6], &current[7]);
        }
        // Baseline moves only now, and only for text that was actually consumed above. Set before the
        // no-op return below so a keystroke that parses to the same profile (typing then deleting a
        // character) still updates the baseline instead of re-diffing against an ever-older seed.
        self.seeded_profile_text = Some(current);
        // A keystroke can leave the parsed profile identical (whitespace, a refused env line); don't
        // churn the file for it.
        if edited == stored {
            return;
        }
        self.profile_store.update(cx, |store, cx| {
            store.update(id, |profile| *profile = edited, cx);
        });
        cx.notify();
    }

    /// Create a profile, select it, and focus its name field so it can be renamed immediately.
    pub(super) fn add_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.settings.read(cx).config().language;
        let profile = eggie_domain::Profile::new(language.new_profile_name().to_owned());
        let id = profile.id;
        self.profile_store
            .update(cx, |store, cx| store.add(profile, cx));
        self.select_profile(id, cx);
        // The name is a placeholder, so put the caret in it rather than making the user hunt for it.
        self.profile_inputs
            .name
            .read(cx)
            .focus_handle()
            .focus(window, cx);
    }

    /// Point the pane at `id`. The inputs are re-seeded by the next `render` (see
    /// [`Self::sync_profile_inputs`]), which is also the path a selection change from another window
    /// travels, so there is only one seeding code path.
    pub(super) fn select_profile(&mut self, id: eggie_domain::ProfileId, cx: &mut Context<Self>) {
        if self.selected_profile == Some(id) {
            return;
        }
        self.selected_profile = Some(id);
        self.open_selector = None;
        self.badge_picker_open = false;
        cx.notify();
    }

    /// Confirm, then delete the profile. The built-in Default is not deletable and never reaches here.
    ///
    /// Projects naming it as their default are cleaned up too. That is belt-and-braces —
    /// `ProfileStore::resolve` already sends a stale id to Default — but it keeps a dangling id off
    /// disk, where a future profile could otherwise inherit it (import, restore from backup) and
    /// silently become some project's default.
    pub(super) fn prompt_delete_profile(
        &mut self,
        id: eggie_domain::ProfileId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.profile_store.read(cx).profile(id) else {
            return;
        };
        if profile.builtin {
            return;
        }
        let language = self.settings.read(cx).config().language;
        let message = language.delete_profile_message(&profile.name);
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            language.delete_profile_title(),
            Some(&message),
            &[language.delete(), language.cancel()],
            cx,
        );
        let settings_window = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            if answer.await.ok() != Some(0) {
                return;
            }
            settings_window
                .update(cx, |settings, cx| {
                    let removed = settings
                        .profile_store
                        .update(cx, |store, cx| store.remove(id, cx));
                    if !removed {
                        return;
                    }
                    // Clear the deleted id from any project that named it as their default. The
                    // "anything changed" return is deliberately ignored: the store already skips the
                    // write and the notify when no project referenced this profile, and there is
                    // nothing further for this callsite to do either way.
                    settings.project_store.update(cx, |store, cx| {
                        let _ = store.forget_profile(id, cx);
                    });
                    // Hand the pane to the built-in Default rather than leaving it on a dead id.
                    let default = settings.profile_store.read(cx).default_profile().id;
                    settings.select_profile(default, cx);
                })
                .ok();
        })
        .detach();
    }

    pub(super) fn render_profiles_section(
        &self,
        language: Language,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let selected_id = self.selected_profile_id(cx);
        let store = self.profile_store.read(cx);
        let Some(profile) = store.profile(selected_id).cloned() else {
            return Vec::new();
        };
        let entries: Vec<(eggie_domain::ProfileId, String, bool, Option<u32>)> = store
            .profiles()
            .iter()
            .map(|profile| {
                (
                    profile.id,
                    profile.name.clone(),
                    profile.builtin,
                    profile.badge_color,
                )
            })
            .collect();

        let mut list_rows = Vec::with_capacity(entries.len());
        for (id, name, builtin, badge_color) in entries {
            let selected = id == selected_id;
            list_rows.push(
                div()
                    .id(("settings-profile-row", id.as_u128() as usize))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(34.))
                    .px_3()
                    .cursor_pointer()
                    .when(selected, |element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.accent))
                    })
                    .when(!selected, |element| {
                        element.hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    })
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.select_profile(id, cx);
                    }))
                    // The badge dot doubles as the list's leading marker, so the color a tab will
                    // carry is visible while picking the profile, not only after launching it.
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .when_some(badge_color, |element, color| element.bg(rgb(color)))
                            .when(badge_color.is_none(), |element| {
                                element.border_1().border_color(rgb(self.colors.border))
                            }),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .when(builtin, |element| {
                        element.child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(self.colors.muted))
                                .child(language.builtin_profile_marker()),
                        )
                    })
                    .into_any_element(),
            );
        }

        // The list's caption is omitted for the same reason as the General pane's first section: the
        // pane title right above it already reads "Profiles". The add/delete buttons sit *below* the
        // card, matching how macOS Settings places the controls for a list (Login Items, Users &
        // Groups) rather than leaving a button row floating in empty space above it.
        let list = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .child(settings_card(list_rows, self.colors))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .px_1()
                    .child(
                        div()
                            .id("settings-profile-add")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_color(rgb(self.colors.muted))
                            .hover(|element| {
                                element
                                    .bg(rgb(self.colors.panel_alt))
                                    .text_color(rgb(self.colors.text))
                            })
                            .tooltip(tooltip_text(
                                SharedString::from(language.add_profile_tooltip()),
                                self.colors,
                            ))
                            .on_click(cx.listener(|settings, _, window, cx| {
                                settings.add_profile(window, cx);
                            }))
                            .child(icon(IconName::Add)),
                    )
                    // Deleting the built-in Default is not offered at all: it is the fallback
                    // every stale profile reference resolves to, so the control is absent
                    // rather than present-and-inert.
                    .when(!profile.builtin, |element| {
                        element.child(
                            div()
                                .id("settings-profile-delete")
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(24.))
                                .rounded_md()
                                .cursor_pointer()
                                .text_color(rgb(self.colors.muted))
                                .hover(|element| {
                                    element
                                        .bg(rgb(self.colors.panel_alt))
                                        .text_color(rgb(self.colors.text))
                                })
                                .tooltip(tooltip_text(
                                    SharedString::from(language.delete_profile_tooltip()),
                                    self.colors,
                                ))
                                .on_click(cx.listener(move |settings, _, window, cx| {
                                    settings.prompt_delete_profile(selected_id, window, cx);
                                }))
                                .child(icon(IconName::Close)),
                        )
                    }),
            )
            .into_any_element();

        // --- The selected profile's fields ---
        let name_row = settings_row(
            language.profile_name_row(),
            language.profile_name_description(),
            self.render_profile_input("settings-profile-name", &self.profile_inputs.name, 220.),
            self.colors,
        );

        let launch_rows = vec![
            settings_row(
                language.shell_program_row(),
                language.shell_program_description(),
                self.render_profile_input(
                    "settings-profile-shell-program",
                    &self.profile_inputs.shell_program,
                    220.,
                ),
                self.colors,
            ),
            settings_row(
                language.shell_args_row(),
                language.shell_args_description(),
                self.render_profile_input(
                    "settings-profile-shell-args",
                    &self.profile_inputs.shell_args,
                    220.,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_cwd_row(),
                language.profile_cwd_description(),
                self.render_profile_input("settings-profile-cwd", &self.profile_inputs.cwd, 220.),
                self.colors,
            ),
            settings_row(
                language.shell_integration_path_row(),
                language.shell_integration_path_description(),
                self.render_profile_path_control(profile.shell_integration_path, language, cx),
                self.colors,
            ),
            // The env editor is multi-line, so it gets a full-width row of its own rather than a
            // squeezed control column.
            self.render_profile_env_row(language, cx),
        ];

        let remote_target = crate::profile_editor::parse_remote(
            self.profile_inputs.remote_host.read(cx).content(),
            self.profile_inputs.remote_user.read(cx).content(),
            self.profile_inputs.remote_port.read(cx).content(),
        );
        // Surface the same judgement the launch path makes: an unusable target is dropped there, so
        // saying so here is the difference between "my profile connects" and a silent local shell.
        let remote_warning = remote_target
            .as_ref()
            .is_some_and(|target| !target.is_usable())
            .then(|| language.profile_remote_invalid());
        let remote_rows = vec![
            self.render_profile_remote_host_row(language, remote_warning),
            settings_row(
                language.profile_remote_user_row(),
                language.profile_remote_user_description(),
                self.render_profile_input(
                    "settings-profile-remote-user",
                    &self.profile_inputs.remote_user,
                    160.,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_remote_port_row(),
                language.profile_remote_port_description(),
                self.render_profile_input(
                    "settings-profile-remote-port",
                    &self.profile_inputs.remote_port,
                    80.,
                ),
                self.colors,
            ),
        ];

        let appearance_rows = vec![
            settings_row(
                language.profile_terminal_dark_theme_row(),
                language.profile_terminal_dark_theme_description(),
                self.render_selector(
                    "settings-profile-terminal-dark-theme",
                    profile
                        .appearance
                        .terminal_dark_theme
                        .clone()
                        .unwrap_or_else(|| language.profile_appearance_inherit().to_owned()),
                    None,
                    SelectorKind::ProfileTerminalDark,
                    window,
                    cx,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_terminal_light_theme_row(),
                language.profile_terminal_light_theme_description(),
                self.render_selector(
                    "settings-profile-terminal-light-theme",
                    profile
                        .appearance
                        .terminal_light_theme
                        .clone()
                        .unwrap_or_else(|| language.profile_appearance_inherit().to_owned()),
                    None,
                    SelectorKind::ProfileTerminalLight,
                    window,
                    cx,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_font_row(),
                language.profile_font_description(),
                self.render_selector(
                    "settings-profile-font",
                    profile
                        .appearance
                        .font_family
                        .clone()
                        .unwrap_or_else(|| language.profile_appearance_inherit().to_owned()),
                    // Render the trigger in the chosen font, as the global font selector does.
                    profile.appearance.font_family.clone(),
                    SelectorKind::ProfileFont,
                    window,
                    cx,
                ),
                self.colors,
            ),
            settings_row(
                language.profile_font_size_row(),
                language.profile_font_size_description(),
                self.render_profile_font_size_control(&profile, language, cx),
                self.colors,
            ),
            settings_row(
                language.profile_badge_row(),
                language.profile_badge_description(),
                self.render_profile_badge_control(&profile, language, cx),
                self.colors,
            ),
        ];

        vec![
            list,
            settings_section(
                Some(language.profile_name_row()),
                vec![name_row],
                self.colors,
            ),
            settings_section(
                Some(language.profile_launch_section()),
                launch_rows,
                self.colors,
            ),
            settings_section(
                Some(language.profile_remote_section()),
                remote_rows,
                self.colors,
            ),
            settings_section(
                Some(language.profile_appearance_section()),
                appearance_rows,
                self.colors,
            ),
        ]
    }

    /// A bordered single-line input for a profile field, `width` px wide.
    pub(super) fn render_profile_input(
        &self,
        id: &'static str,
        input: &Entity<TextInput>,
        width: f32,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .w(px(width))
            .h(px(30.))
            .px_2()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel_alt))
            .text_size(px(12.))
            .text_color(rgb(self.colors.text))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(input.clone()),
            )
            .into_any_element()
    }

    /// The remote host row, with the "this target is ignored" warning appended when it applies.
    pub(super) fn render_profile_remote_host_row(
        &self,
        language: Language,
        warning: Option<&'static str>,
    ) -> AnyElement {
        let control = self.render_profile_input(
            "settings-profile-remote-host",
            &self.profile_inputs.remote_host,
            220.,
        );
        let colors = self.colors;
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_6()
            .min_h(px(64.))
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap_1()
                    .child(language.profile_remote_host_row())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(language.profile_remote_host_description()),
                    )
                    .when_some(warning, |element, warning| {
                        // Tinted with the bell/attention color so a rejected target reads as a problem
                        // rather than as more description.
                        element.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(WARNING_TEXT))
                                .child(warning),
                        )
                    }),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }

    /// The multi-line environment editor, plus a count of ignored lines when the text has any.
    pub(super) fn render_profile_env_row(
        &self,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        // Recompute from the live text rather than from `profile.env`: the stored map has already had
        // the bad lines dropped, so it cannot say how many there were.
        let (_, rejected) =
            crate::profile_editor::parse_env(self.profile_inputs.env.read(cx).content());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(language.profile_env_row())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(language.profile_env_description()),
                    ),
            )
            .child(
                div()
                    .id("settings-profile-env")
                    .w_full()
                    .h(px(96.))
                    .px_2()
                    .py_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(colors.border))
                    .bg(rgb(colors.panel_alt))
                    .text_size(px(12.))
                    .text_color(rgb(colors.text))
                    .overflow_hidden()
                    .child(self.profile_inputs.env.clone()),
            )
            .when(rejected > 0, |element| {
                element.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(WARNING_TEXT))
                        .child(language.profile_env_rejected(rejected)),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_profile_path_control(
        &self,
        enabled: bool,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = vec![
            (
                SharedString::from("on"),
                SharedString::from(language.cursor_blink_on()),
                enabled,
            ),
            (
                SharedString::from("off"),
                SharedString::from(language.cursor_blink_off()),
                !enabled,
            ),
        ];
        self.render_segmented(
            "settings-profile-path",
            options,
            move |settings, index, cx| {
                let Some(id) = settings.selected_profile else {
                    return;
                };
                settings.profile_store.update(cx, |store, cx| {
                    store.update(
                        id,
                        |profile| profile.shell_integration_path = index == 0,
                        cx,
                    );
                });
            },
            cx,
        )
    }

    /// The per-profile font-size stepper. Steps from the *resolved* size, so the first click moves
    /// relative to what the terminal currently shows rather than jumping to a fixed number; a
    /// dedicated entry clears the override back to the global size.
    pub(super) fn render_profile_font_size_control(
        &self,
        profile: &eggie_domain::Profile,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let global = self.settings.read(cx).config().font_size;
        let override_size = profile.appearance.font_size;
        let effective = override_size.unwrap_or(global);
        // An unset override displays the inherited value in parentheses, so "follow global" is visible
        // as a state rather than looking identical to an override that happens to match.
        let display = match override_size {
            Some(size) => format!("{size:.0}"),
            None => format!("({global:.0})"),
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.render_stepper(
                "settings-profile-font-size-dec",
                "settings-profile-font-size-inc",
                display,
                effective > MIN_FONT_SIZE,
                effective < MAX_FONT_SIZE,
                56.,
                move |settings, dir, cx| {
                    let Some(id) = settings.selected_profile else {
                        return;
                    };
                    let next = (effective + f32::from(dir)).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
                    settings.profile_store.update(cx, |store, cx| {
                        store.update(id, |profile| profile.appearance.font_size = Some(next), cx);
                    });
                },
                cx,
            ))
            .when(override_size.is_some(), |element| {
                element.child(
                    div()
                        .id("settings-profile-font-size-clear")
                        .flex()
                        .items_center()
                        .h(px(30.))
                        .px_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(rgb(self.colors.muted))
                        .hover(|element| {
                            element
                                .bg(rgb(self.colors.panel_alt))
                                .text_color(rgb(self.colors.text))
                        })
                        .on_click(cx.listener(|settings, _, _, cx| {
                            let Some(id) = settings.selected_profile else {
                                return;
                            };
                            settings.profile_store.update(cx, |store, cx| {
                                store.update(id, |profile| profile.appearance.font_size = None, cx);
                            });
                        }))
                        .child(language.profile_appearance_inherit()),
                )
            })
            .into_any_element()
    }

    /// The badge color picker: a swatch that opens a small palette popover.
    pub(super) fn render_profile_badge_control(
        &self,
        profile: &eggie_domain::Profile,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = profile.badge_color;
        let dropdown_position = self
            .badge_picker_bounds
            .map(|bounds| point(bounds.left(), bounds.bottom() + px(SELECTOR_DROPDOWN_GAP)));
        let settings_entity = cx.entity().downgrade();
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let settings_entity = settings_entity.clone();
                cx.defer(move |cx| {
                    settings_entity
                        .update(cx, |settings, cx| {
                            if settings.badge_picker_bounds != Some(bounds) {
                                settings.badge_picker_bounds = Some(bounds);
                                if settings.badge_picker_open {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .relative()
            .child(
                div()
                    .id("settings-profile-badge")
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(self.colors.panel_alt))
                    .cursor_pointer()
                    .hover(|element| element.border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.badge_picker_open = !settings.badge_picker_open;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_none()
                            .size(px(12.))
                            .rounded_full()
                            .when_some(current, |element, color| element.bg(rgb(color)))
                            .when(current.is_none(), |element| {
                                element.border_1().border_color(rgb(self.colors.border))
                            }),
                    )
                    .child(div().text_size(px(12.)).child(match current {
                        Some(color) => format!("#{color:06X}"),
                        None => language.profile_badge_none().to_owned(),
                    })),
            )
            .when(self.badge_picker_open, |element| {
                let mut popover = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    popover = popover.position(position);
                }
                element.child(
                    deferred(popover.child(self.render_badge_palette(language, cx))).priority(2),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_badge_palette(
        &self,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut swatches = div().flex().flex_wrap().gap_2().max_w(px(180.));
        for (index, color) in BADGE_COLORS.iter().copied().enumerate() {
            swatches = swatches.child(
                div()
                    .id(("settings-badge-swatch", index))
                    .size(px(20.))
                    .rounded_full()
                    .bg(rgb(color))
                    .cursor_pointer()
                    .hover(|element| element.border_2().border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        settings.set_badge_color(Some(color), cx);
                    })),
            );
        }
        div()
            .id("settings-badge-palette")
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .shadow_lg()
            // Floating overlay: block events for the rows painted behind it, so a click in the
            // popover's padding cannot bubble through and hit a control underneath.
            .occlude()
            .on_mouse_down_out(cx.listener(|settings, _, window, cx| {
                cx.defer_in(window, |settings, _, cx| {
                    if settings.badge_picker_open {
                        settings.badge_picker_open = false;
                        cx.notify();
                    }
                });
                let _ = settings;
            }))
            .child(swatches)
            .child(
                div()
                    .id("settings-badge-clear")
                    .flex()
                    .items_center()
                    .h(px(26.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(rgb(self.colors.muted))
                    .hover(|element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.text))
                    })
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.set_badge_color(None, cx);
                    }))
                    .child(language.profile_badge_none()),
            )
            .into_any_element()
    }

    pub(super) fn set_badge_color(&mut self, color: Option<u32>, cx: &mut Context<Self>) {
        let Some(id) = self.selected_profile else {
            return;
        };
        self.profile_store.update(cx, |store, cx| {
            store.update(id, |profile| profile.badge_color = color, cx);
        });
        self.badge_picker_open = false;
        cx.notify();
    }
}

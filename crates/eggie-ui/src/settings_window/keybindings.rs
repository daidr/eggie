use super::*;

impl SettingsWindow {
    pub(super) fn start_recording(&mut self, id: String, cx: &mut Context<Self>) {
        self.recording = Some(id);
        self.recording_conflict = None;
        let weak = cx.weak_entity();
        self.recording_subscription = Some(cx.intercept_keystrokes(move |event, _window, cx| {
            cx.stop_propagation();
            let keystroke = event.keystroke.clone();
            weak.update(cx, |this, cx| this.on_recorded_keystroke(keystroke, cx))
                .ok();
        }));
        cx.notify();
    }

    pub(super) fn on_recorded_keystroke(
        &mut self,
        keystroke: gpui::Keystroke,
        cx: &mut Context<Self>,
    ) {
        // Escape cancels recording.
        if keystroke.key == "escape" && !keystroke.modifiers.modified() {
            self.cancel_recording(cx);
            return;
        }
        // Ignore the intermediate state where only modifiers are held.
        if crate::keybindings::is_bare_modifier(&keystroke.key) {
            return;
        }
        // Require at least one modifier so a bare letter cannot swallow terminal input.
        if !keystroke.modifiers.modified() {
            return;
        }
        let Some(id) = self.recording.clone() else {
            return;
        };
        let canonical = crate::keybindings::canonical_keystroke(&keystroke);
        // Reject a combination already bound to a different action.
        if let Some(other_id) = self.find_binding_conflict(&canonical, &id, cx) {
            let language = self.settings.read(cx).config().language;
            let other_label = crate::keybindings::spec_by_id(other_id)
                .map(|spec| (spec.label)(language))
                .unwrap_or(other_id);
            self.recording_conflict = Some(other_label.to_owned());
            cx.notify();
            return;
        }
        self.set_binding(id, canonical, cx);
        self.cancel_recording(cx);
    }

    pub(super) fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.recording = None;
        self.recording_subscription = None;
        self.recording_conflict = None;
        cx.notify();
    }

    pub(super) fn find_binding_conflict(
        &self,
        candidate: &str,
        exclude_id: &str,
        cx: &Context<Self>,
    ) -> Option<&'static str> {
        let config = self.settings.read(cx).config();
        crate::keybindings::find_conflict(config, candidate, exclude_id)
    }

    /// Persist an override for `id`. If the recorded keystroke equals the default, drop the
    /// override instead of storing a redundant entry.
    pub(super) fn set_binding(&mut self, id: String, canonical: String, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |config| {
                    if crate::keybindings::default_keystroke(&id) == Some(canonical.as_str()) {
                        config.keybindings.remove(&id);
                    } else {
                        config.keybindings.insert(id.clone(), canonical.clone());
                    }
                },
                cx,
            )
        });
    }

    pub(super) fn reset_binding(&mut self, id: String, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |config| {
                    config.keybindings.remove(&id);
                },
                cx,
            )
        });
    }

    pub(super) fn reset_all_bindings(&mut self, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|config| config.keybindings.clear(), cx)
        });
    }

    pub(super) fn render_keybindings_section(
        &self,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let config = self.settings.read(cx).config().clone();
        let has_overrides = !config.keybindings.is_empty();
        let colors = self.colors;

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(language.keybindings_hint()),
            )
            .when(has_overrides, |element| {
                element.child(
                    div()
                        .id("reset-all-keybindings")
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(28.))
                        .px_3()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(colors.border))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .hover(|element| element.bg(rgb(colors.panel_alt)))
                        .child(language.reset_all())
                        .on_click(cx.listener(|settings, _, _, cx| {
                            settings.reset_all_bindings(cx);
                        })),
                )
            })
            .into_any_element();

        let rows: Vec<AnyElement> = crate::keybindings::ACTION_SPECS
            .iter()
            .map(|spec| self.render_keybinding_row(spec, &config, language, cx))
            .collect();

        vec![
            header,
            // Same reason as the General pane: the pane title already reads "Keyboard Shortcuts".
            settings_section(None, rows, colors),
        ]
    }

    pub(super) fn render_keybinding_row(
        &self,
        spec: &'static crate::keybindings::ActionSpec,
        config: &AppSettings,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = self.colors;
        let id = spec.id;
        let label = (spec.label)(language);
        let effective =
            crate::keybindings::effective_keystroke(config, id).unwrap_or(spec.default_keystroke);
        let lua = &self.settings.read(cx).lua_config().registered;
        let lua_managed = lua.bindings.iter().any(|operation| match operation {
            eggie_config::BindingOp::Bind { key, action, .. } => key == effective || action == id,
            eggie_config::BindingOp::Unbind { key } => key == effective,
        });
        let is_overridden = config.keybindings.contains_key(id);
        let is_recording = self.recording.as_deref() == Some(id);
        let conflict = if is_recording {
            self.recording_conflict.clone()
        } else {
            None
        };

        let cell_text = if is_recording {
            if let Some(other) = conflict.as_deref() {
                language.keybind_conflict(other)
            } else {
                language.recording_prompt().to_owned()
            }
        } else {
            let shortcut = crate::command_palette::display_binding(config, lua, id)
                .unwrap_or_else(|| "Unbound".to_owned());
            if lua_managed {
                format!("{shortcut} · init.lua")
            } else {
                shortcut
            }
        };
        let cell_text_color = if conflict.is_some() {
            0xff6b6b
        } else if is_recording {
            colors.accent
        } else {
            colors.text
        };
        let cell_border = if is_recording {
            colors.accent
        } else {
            colors.border
        };

        let recorded_id = id.to_string();
        let shortcut_cell = div()
            .id(SharedString::from(format!("keybind-{id}")))
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.))
            .min_w(px(84.))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(cell_border))
            .text_size(px(13.))
            .text_color(rgb(cell_text_color))
            .when(!lua_managed, |element| element.cursor_pointer())
            .when(!is_recording, |element| {
                element.hover(|element| element.bg(rgb(colors.panel_alt)))
            })
            .when(is_recording, |element| element.bg(rgb(colors.panel_alt)))
            .child(cell_text)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |settings, _, _, cx| {
                if lua_managed {
                    return;
                }
                if settings.recording.as_deref() == Some(recorded_id.as_str()) {
                    settings.cancel_recording(cx);
                } else {
                    settings.start_recording(recorded_id.clone(), cx);
                }
            }));

        let reset_id = id.to_string();
        let reset_button = div()
            .flex_none()
            .when(is_overridden && !lua_managed, |element| {
                element.child(
                    div()
                        .id(SharedString::from(format!("reset-keybind-{id}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(28.))
                        .px_2()
                        .rounded_md()
                        .text_size(px(11.))
                        .text_color(rgb(colors.muted))
                        .cursor_pointer()
                        .hover(|element| element.bg(rgb(colors.panel_alt)))
                        .child(language.reset_to_default())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |settings, _, _, cx| {
                            settings.reset_binding(reset_id.clone(), cx);
                        })),
                )
            });

        self.search_anchor(label)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .min_h(px(48.))
                    .px_4()
                    .py_2()
                    .child(div().flex_1().min_w_0().child(label))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(reset_button)
                            .child(shortcut_cell),
                    ),
            )
            .into_any_element()
    }
}

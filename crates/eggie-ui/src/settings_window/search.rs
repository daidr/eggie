use super::*;

#[derive(Clone, Copy)]
pub(super) struct SettingSearchEntry {
    section: SettingsSection,
    label: &'static str,
    alternate: &'static str,
    keywords: &'static str,
}

fn entries(language: Language) -> Vec<SettingSearchEntry> {
    let other = if language == Language::English {
        Language::SimplifiedChinese
    } else {
        Language::English
    };
    let mut result = Vec::new();
    macro_rules! entries {
        ($section:ident; $($label:ident),*) => {
            $(result.push(SettingSearchEntry { section: SettingsSection::$section,
                label: language.$label(), alternate: other.$label(), keywords: stringify!($label) });)*
        };
    }
    entries!(General; language_row, sidebar_layout_row, scrollback_lines_row);
    entries!(Appearance; theme_row, terminal_theme_source_row, minimum_contrast_row, font_row, font_bold_row, font_italic_row, font_bold_italic_row, synthetic_bold_row, synthetic_italic_row, synthetic_bold_italic_row, ligatures_row, shaping_break_row, font_thicken_row, font_thicken_strength_row, font_size_row, cursor_shape_row, cursor_blink_row, horizontal_padding_row, vertical_padding_row);
    entries!(Advanced; completed_timeout_row, inactive_timeout_row, terminal_suggest, detect_urls_row, copy_on_select_row, bell_row, osc_clipboard_read_row, shell_program_row, shell_args_row, shell_integration_path_row);
    entries!(Profiles; profile_name_row, shell_program_row, shell_args_row, profile_cwd_row, shell_integration_path_row, profile_remote_user_row, profile_remote_port_row, profile_terminal_dark_theme_row, profile_terminal_light_theme_row, profile_font_row, profile_font_size_row, profile_badge_row);
    entries!(About; auto_check_updates_row, update_channel_row);
    entries!(Appearance; interface_light_theme_row, interface_dark_theme_row);
    entries!(Profiles; profile_env_row, profile_remote_host_row);
    entries!(Appearance; terminal_light_theme_row, terminal_dark_theme_row);
    for spec in crate::keybindings::ACTION_SPECS {
        result.push(SettingSearchEntry {
            section: SettingsSection::Keybindings,
            label: (spec.label)(language),
            alternate: (spec.label)(other),
            keywords: spec.id,
        });
    }
    for kind in MetricAdjustmentKind::ALL {
        result.push(SettingSearchEntry {
            section: SettingsSection::Appearance,
            label: SettingsWindow::metric_adjustment_labels(language, kind).0,
            alternate: SettingsWindow::metric_adjustment_labels(other, kind).0,
            keywords: kind.slug(),
        });
    }
    let zh = language == Language::SimplifiedChinese;
    result.push(SettingSearchEntry {
        section: SettingsSection::Advanced,
        label: if zh {
            "进程结束后"
        } else {
            "When the process exits"
        },
        alternate: if zh {
            "When the process exits"
        } else {
            "进程结束后"
        },
        keywords: "terminal exit behavior",
    });
    result.push(SettingSearchEntry {
        section: SettingsSection::Advanced,
        label: if zh { "粘贴预览" } else { "Paste preview" },
        alternate: if zh { "Paste preview" } else { "粘贴预览" },
        keywords: "paste multiline",
    });
    result.push(SettingSearchEntry {
        section: SettingsSection::Keybindings,
        label: language.keybindings_section(),
        alternate: other.keybindings_section(),
        keywords: "keyboard shortcuts 快捷键",
    });
    result
}

fn matches(entry: &SettingSearchEntry, query: &str) -> bool {
    let text = format!(
        "{} {} {}",
        entry.label,
        entry.alternate,
        entry.keywords.replace('_', " ")
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|part| text.contains(&part.to_lowercase()))
}

impl SettingsWindow {
    pub(super) fn search_anchor(&self, label: &'static str) -> gpui::Stateful<gpui::Div> {
        let target = self.search_target.clone();
        let anchor = gpui::ScrollAnchor::for_handle(self.settings_scroll_handle.clone());
        let tracked = anchor.clone();
        div()
            .w_full()
            .flex_none()
            .min_w_0()
            .on_children_prepainted(move |_, window, cx| {
                let requested = *target.borrow() == Some(label);
                if requested {
                    target.borrow_mut().take();
                    anchor.scroll_to(window, cx);
                    window.on_next_frame(|window, _| window.refresh());
                }
            })
            .id(format!("setting-anchor-{label}"))
            .anchor_scroll(Some(tracked))
    }

    pub(super) fn setting_row(
        &self,
        label: &'static str,
        description: Option<&'static str>,
        control: AnyElement,
        colors: UiColors,
    ) -> AnyElement {
        self.search_anchor(label)
            .child(settings_row(label, description, control, colors))
            .into_any_element()
    }

    fn reveal_setting(
        &mut self,
        entry: SettingSearchEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_selector = None;
        self.bell_dropdown_open = false;
        self.clear_theme_preview(cx);
        if self.recording.is_some() {
            self.cancel_recording(cx);
        }
        self.selected_section = entry.section;
        self.settings_search_input
            .update(cx, |input, cx| input.set_content(String::new(), cx));
        let language = self.settings.read(cx).config().language;
        let target = if entry.section == SettingsSection::Appearance
            && matches!(
                entry.keywords,
                "terminal_light_theme_row" | "terminal_dark_theme_row"
            )
            && self.settings.read(cx).config().terminal_theme_source != TerminalThemeSource::Custom
        {
            language.terminal_theme_source_row()
        } else if entry.section == SettingsSection::Profiles
            && entry.keywords.starts_with("profile_remote_")
            && self
                .profile_store
                .read(cx)
                .resolve(self.selected_profile)
                .remote
                .is_none()
        {
            if language == Language::SimplifiedChinese {
                "连接方式"
            } else {
                "Connection"
            }
        } else {
            entry.label
        };
        *self.search_target.borrow_mut() = Some(target);
        self.settings_scroll_handle
            .set_offset(point(px(0.), px(0.)));
        window.blur();
        cx.notify();
    }

    pub(super) fn on_settings_search(
        &mut self,
        _: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Confirm => {
                let query = self.settings_search_input.read(cx).content();
                if let Some(entry) = entries(self.settings.read(cx).config().language)
                    .into_iter()
                    .filter(|entry| matches(entry, query))
                    .nth(self.settings_search_selected)
                {
                    self.reveal_setting(entry, window, cx);
                }
            }
            TextInputEvent::Cancel => {
                self.settings_search_input
                    .update(cx, |input, cx| input.set_content(String::new(), cx));
            }
            TextInputEvent::Changed => self.settings_search_selected = 0,
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn navigate_settings_search(
        &mut self,
        keystroke: gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if keystroke.modifiers.modified()
            || !matches!(keystroke.key.as_str(), "up" | "down" | "enter" | "escape")
            || !self
                .settings_search_input
                .read(cx)
                .focus_handle()
                .is_focused(window)
        {
            return false;
        }
        let query = self.settings_search_input.read(cx).content();
        if query.trim().is_empty() {
            return false;
        }
        if matches!(keystroke.key.as_str(), "enter" | "escape") {
            let input = self.settings_search_input.clone();
            let event = if keystroke.key == "enter" {
                TextInputEvent::Confirm
            } else {
                TextInputEvent::Cancel
            };
            self.on_settings_search(&input, &event, window, cx);
            return true;
        }
        let items: Vec<_> = entries(self.settings.read(cx).config().language)
            .into_iter()
            .filter(|entry| matches(entry, query))
            .collect();
        self.settings_search_selected = if keystroke.key == "down" {
            (self.settings_search_selected + 1).min(items.len().saturating_sub(1))
        } else {
            self.settings_search_selected.saturating_sub(1)
        };
        *self.search_target.borrow_mut() = items
            .get(self.settings_search_selected)
            .map(|entry| entry.label);
        cx.notify();
        true
    }

    pub(super) fn render_settings_results(
        &self,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let query = self.settings_search_input.read(cx).content();
        let items: Vec<_> = entries(language)
            .into_iter()
            .filter(|entry| matches(entry, query))
            .collect();
        if items.is_empty() {
            return vec![
                div()
                    .text_color(rgb(self.colors.muted))
                    .child(if language == Language::SimplifiedChinese {
                        "没有匹配的设置"
                    } else {
                        "No matching settings"
                    })
                    .into_any_element(),
            ];
        }
        items
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                let selected = self.settings_search_selected == index;
                let target = self.search_target.clone();
                let anchor = gpui::ScrollAnchor::for_handle(self.settings_scroll_handle.clone());
                let tracked = anchor.clone();
                let section = match entry.section {
                    SettingsSection::General => language.general_section(),
                    SettingsSection::Appearance => language.appearance_section(),
                    SettingsSection::Advanced => language.advanced_section(),
                    SettingsSection::Profiles => language.profiles_section(),
                    SettingsSection::Keybindings => language.keybindings_section(),
                    SettingsSection::About => language.about_eggie(),
                };
                div()
                    .on_children_prepainted(move |_, window, cx| {
                        let requested = selected && *target.borrow() == Some(entry.label);
                        if requested {
                            target.borrow_mut().take();
                            anchor.scroll_to(window, cx);
                            window.on_next_frame(|window, _| window.refresh());
                        }
                    })
                    .id(format!("settings-result-{index}"))
                    .anchor_scroll(Some(tracked))
                    .rounded_lg()
                    .p_3()
                    .when(selected, |row| row.bg(rgb(self.colors.panel_alt)))
                    .cursor_pointer()
                    .hover(|row| row.bg(rgb(self.colors.panel_alt)))
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        settings.reveal_setting(entry, window, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_3()
                            .child(entry.label)
                            .child(div().text_color(rgb(self.colors.muted)).child(section)),
                    )
                    .into_any_element()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_search_matches_both_languages_and_each_query_word() {
        let entry = entries(Language::SimplifiedChinese)
            .into_iter()
            .find(|entry| entry.keywords == "font_size_row")
            .unwrap();
        assert!(matches(&entry, "字号"));
        assert!(matches(&entry, "FONT size"));
        assert!(!matches(&entry, "font unknown"));
    }
}

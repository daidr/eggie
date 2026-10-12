use super::*;

impl SettingsWindow {
    // Construct only the visible section; hidden controls otherwise allocate on every draw.
    pub(super) fn render_current_section(
        &mut self,
        config: &AppSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let language = config.language;
        match self.selected_section {
            SettingsSection::General => {
                let language_control = self.render_language_control(language, cx);
                let sidebar_layout_control =
                    self.render_sidebar_layout_control(config.sidebar_layout, cx);
                let scrollback_control =
                    self.render_scrollback_control(config.scrollback_lines, cx);
                vec![
                    settings_section(
                        // The pane title above already reads
                        // "General"; a caption here would
                        // repeat it on the very next line.
                        None,
                        vec![
                            settings_row(
                                language.language_row(),
                                None,
                                language_control,
                                self.colors,
                            ),
                            settings_row(
                                language.sidebar_layout_row(),
                                None,
                                sidebar_layout_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.terminal_behavior_section()),
                        vec![settings_row(
                            language.scrollback_lines_row(),
                            Some(language.scrollback_lines_description()),
                            scrollback_control,
                            self.colors,
                        )],
                        self.colors,
                    ),
                ]
            }
            SettingsSection::Appearance => {
                let mode_control = self.render_mode_control(config.theme_mode, cx);
                let interface_light_gallery = self.render_theme_gallery(
                    SelectorKind::InterfaceLight,
                    language.interface_light_theme_row(),
                    &config.interface_light_theme,
                    window,
                    cx,
                );
                let interface_dark_gallery = self.render_theme_gallery(
                    SelectorKind::InterfaceDark,
                    language.interface_dark_theme_row(),
                    &config.interface_dark_theme,
                    window,
                    cx,
                );
                let terminal_theme_source_control =
                    self.render_terminal_theme_source_control(config.terminal_theme_source, cx);
                // Only built when the terminal uses its own pair: under `FollowInterface` these two rows are
                // omitted entirely rather than shown disabled, so the panel states what is actually in effect.
                let terminal_theme_galleries =
                    (config.terminal_theme_source == TerminalThemeSource::Custom).then(|| {
                        (
                            self.render_theme_gallery(
                                SelectorKind::TerminalLight,
                                language.terminal_light_theme_row(),
                                &config.terminal_light_theme,
                                window,
                                cx,
                            ),
                            self.render_theme_gallery(
                                SelectorKind::TerminalDark,
                                language.terminal_dark_theme_row(),
                                &config.terminal_dark_theme,
                                window,
                                cx,
                            ),
                        )
                    });
                let font_selector = self.render_selector(
                    "font-selector",
                    config.font_family.clone(),
                    Some(config.font_family.clone()),
                    SelectorKind::Font,
                    window,
                    cx,
                );
                // Per-style family selectors. An empty stored value means "fall back to the regular family";
                // show a localized hint and preview in the regular family in that case.
                let style_family_selector =
                    |this: &Self,
                     id: &'static str,
                     stored: &str,
                     kind: SelectorKind,
                     window: &Window,
                     cx: &mut Context<Self>| {
                        let has_value = !stored.trim().is_empty();
                        let display = if has_value {
                            stored.to_owned()
                        } else {
                            language.font_family_use_regular().to_owned()
                        };
                        let preview_family = if has_value {
                            stored.to_owned()
                        } else {
                            config.font_family.clone()
                        };
                        this.render_selector(id, display, Some(preview_family), kind, window, cx)
                    };
                let font_bold_selector = style_family_selector(
                    self,
                    "font-bold-selector",
                    &config.font_family_bold,
                    SelectorKind::FontBold,
                    window,
                    cx,
                );
                let font_italic_selector = style_family_selector(
                    self,
                    "font-italic-selector",
                    &config.font_family_italic,
                    SelectorKind::FontItalic,
                    window,
                    cx,
                );
                let font_bold_italic_selector = style_family_selector(
                    self,
                    "font-bold-italic-selector",
                    &config.font_family_bold_italic,
                    SelectorKind::FontBoldItalic,
                    window,
                    cx,
                );
                let synthetic_bold_control = self.render_synthetic_style_control(
                    SyntheticStyleKind::Bold,
                    config.font_synthetic_style.bold,
                    cx,
                );
                let synthetic_italic_control = self.render_synthetic_style_control(
                    SyntheticStyleKind::Italic,
                    config.font_synthetic_style.italic,
                    cx,
                );
                let synthetic_bold_italic_control = self.render_synthetic_style_control(
                    SyntheticStyleKind::BoldItalic,
                    config.font_synthetic_style.bold_italic,
                    cx,
                );
                let ligatures_control = self.render_ligatures_control(config.ligatures, cx);
                let shaping_break_control =
                    self.render_shaping_break_control(config.font_shaping_break_cursor, cx);
                let font_thicken_control =
                    self.render_font_thicken_control(config.font_thicken, cx);
                let font_thicken_strength_control =
                    self.render_font_thicken_strength_control(config.font_thicken_strength, cx);
                let font_size_control = self.render_font_size_control(config.font_size, cx);
                let cursor_shape_control =
                    self.render_cursor_shape_control(config.cursor_shape, cx);
                let cursor_blink_control =
                    self.render_cursor_blink_control(config.cursor_blink, cx);
                let minimum_contrast_control =
                    self.render_minimum_contrast_control(config.minimum_contrast, cx);
                // The Theme section, assembled here rather than inline because two of its rows are conditional
                // and the section list sits several levels deep in the element tree.
                //
                // Order tells the story of the layering: the interface pair first (it decides the chrome, and
                // the terminal follows it by default), then the terminal's own source and — only when it has
                // one — its pair.
                let mut theme_rows = vec![
                    settings_row(language.theme_row(), None, mode_control, self.colors),
                    interface_light_gallery,
                    interface_dark_gallery,
                    settings_row(
                        language.terminal_theme_source_row(),
                        None,
                        terminal_theme_source_control,
                        self.colors,
                    ),
                ];
                if let Some((terminal_light_gallery, terminal_dark_gallery)) =
                    terminal_theme_galleries
                {
                    theme_rows.push(terminal_light_gallery);
                    theme_rows.push(terminal_dark_gallery);
                }
                theme_rows.push(settings_row(
                    language.minimum_contrast_row(),
                    Some(language.minimum_contrast_description()),
                    minimum_contrast_control,
                    self.colors,
                ));
                let horizontal_padding_control = self.render_terminal_padding_control(
                    PaddingAxis::Horizontal,
                    config.terminal_padding_x,
                    cx,
                );
                let vertical_padding_control = self.render_terminal_padding_control(
                    PaddingAxis::Vertical,
                    config.terminal_padding_y,
                    cx,
                );
                // Build the nine font-metric adjustment rows up front (label + description + stepper).
                let metric_rows: Vec<AnyElement> = MetricAdjustmentKind::ALL
                    .into_iter()
                    .map(|kind| {
                        let (label, description) = Self::metric_adjustment_labels(language, kind);
                        settings_row(
                            label,
                            Some(description),
                            self.render_metric_adjustment_control(kind, cx),
                            self.colors,
                        )
                    })
                    .collect();
                vec![
                    settings_section(Some(language.theme_section()), theme_rows, self.colors),
                    settings_section(
                        Some(language.terminal_text_section()),
                        vec![
                            settings_row(
                                language.font_row(),
                                Some(language.font_description()),
                                font_selector,
                                self.colors,
                            ),
                            settings_row(
                                language.font_bold_row(),
                                None,
                                font_bold_selector,
                                self.colors,
                            ),
                            settings_row(
                                language.font_italic_row(),
                                None,
                                font_italic_selector,
                                self.colors,
                            ),
                            settings_row(
                                language.font_bold_italic_row(),
                                None,
                                font_bold_italic_selector,
                                self.colors,
                            ),
                            settings_row(
                                language.synthetic_bold_row(),
                                Some(language.synthetic_bold_description()),
                                synthetic_bold_control,
                                self.colors,
                            ),
                            settings_row(
                                language.synthetic_italic_row(),
                                Some(language.synthetic_italic_description()),
                                synthetic_italic_control,
                                self.colors,
                            ),
                            settings_row(
                                language.synthetic_bold_italic_row(),
                                Some(language.synthetic_bold_italic_description()),
                                synthetic_bold_italic_control,
                                self.colors,
                            ),
                            settings_row(
                                language.ligatures_row(),
                                Some(language.ligatures_description()),
                                ligatures_control,
                                self.colors,
                            ),
                            settings_row(
                                language.shaping_break_row(),
                                None,
                                shaping_break_control,
                                self.colors,
                            ),
                            settings_row(
                                language.font_thicken_row(),
                                None,
                                font_thicken_control,
                                self.colors,
                            ),
                            settings_row(
                                language.font_thicken_strength_row(),
                                None,
                                font_thicken_strength_control,
                                self.colors,
                            ),
                            settings_row(
                                language.font_size_row(),
                                None,
                                font_size_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.cursor_section()),
                        vec![
                            settings_row(
                                language.cursor_shape_row(),
                                Some(language.cursor_shape_description()),
                                cursor_shape_control,
                                self.colors,
                            ),
                            settings_row(
                                language.cursor_blink_row(),
                                None,
                                cursor_blink_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.terminal_layout_section()),
                        vec![
                            settings_row(
                                language.horizontal_padding_row(),
                                None,
                                horizontal_padding_control,
                                self.colors,
                            ),
                            settings_row(
                                language.vertical_padding_row(),
                                None,
                                vertical_padding_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.font_metrics_section()),
                        metric_rows,
                        self.colors,
                    ),
                ]
            }
            SettingsSection::Advanced => {
                let bell_control = self.render_bell_control(config.bell_mode, window, cx);
                let progress_complete_timeout_control = self.render_progress_timeout_control(
                    ProgressTimeoutKind::Complete,
                    config.progress_complete_timeout_secs,
                    1,
                    cx,
                );
                let progress_stale_timeout_control = self.render_progress_timeout_control(
                    ProgressTimeoutKind::Stale,
                    config.progress_stale_timeout_secs,
                    5,
                    cx,
                );
                let osc_clipboard_read_control =
                    self.render_osc_clipboard_read_control(config.allow_osc_clipboard_read, cx);
                let completion_control = self.render_segmented(
                    "terminal-suggestions",
                    vec![
                        (
                            "off".into(),
                            language.disabled().into(),
                            !config.terminal_suggestions,
                        ),
                        (
                            "on".into(),
                            language.enabled().into(),
                            config.terminal_suggestions,
                        ),
                    ],
                    |window, index, cx| {
                        window.settings.update(cx, |store, cx| {
                            store.update(|c| c.terminal_suggestions = index == 1, cx)
                        });
                    },
                    cx,
                );
                let detect_urls_control = self.render_detect_urls_control(config.detect_urls, cx);
                let copy_on_select_control =
                    self.render_copy_on_select_control(config.copy_on_select, cx);
                let paste_preview_control =
                    self.render_paste_preview_control(config.paste_preview, cx);
                let terminal_exit_control =
                    self.render_terminal_exit_control(config.terminal_exit_behavior, cx);
                let shell_program_control = self.render_shell_input_control(
                    "settings-shell-program",
                    &self.shell_program_input,
                );
                let shell_args_control =
                    self.render_shell_input_control("settings-shell-args", &self.shell_args_input);
                let shell_integration_path_control = self.render_shell_integration_path_control(
                    // Read from the Default profile, which owns this field — `config` keeps only the
                    // downgrade mirror, so reading it would show a stale value after an edit elsewhere.
                    self.profile_store
                        .read(cx)
                        .default_profile()
                        .shell_integration_path,
                    cx,
                );
                vec![
                    settings_section(
                        Some(language.progress_indicators_section()),
                        vec![
                            settings_row(
                                language.completed_timeout_row(),
                                None,
                                progress_complete_timeout_control,
                                self.colors,
                            ),
                            settings_row(
                                language.inactive_timeout_row(),
                                None,
                                progress_stale_timeout_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.terminal_behavior_section()),
                        vec![
                            settings_row(
                                if language == Language::SimplifiedChinese {
                                    "进程结束后"
                                } else {
                                    "When the process exits"
                                },
                                None,
                                terminal_exit_control,
                                self.colors,
                            ),
                            settings_row(
                                language.terminal_suggest(),
                                None,
                                completion_control,
                                self.colors,
                            ),
                            settings_row(
                                language.detect_urls_row(),
                                Some(language.detect_urls_description()),
                                detect_urls_control,
                                self.colors,
                            ),
                            settings_row(
                                language.copy_on_select_row(),
                                None,
                                copy_on_select_control,
                                self.colors,
                            ),
                            settings_row(language.bell_row(), None, bell_control, self.colors),
                            settings_row(
                                if language == Language::SimplifiedChinese {
                                    "粘贴预览"
                                } else {
                                    "Paste preview"
                                },
                                None,
                                paste_preview_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.terminal_security_section()),
                        vec![settings_row(
                            language.osc_clipboard_read_row(),
                            Some(language.osc_clipboard_read_description()),
                            osc_clipboard_read_control,
                            self.colors,
                        )],
                        self.colors,
                    ),
                    settings_section(
                        Some(language.shell_section()),
                        vec![
                            settings_row(
                                language.shell_program_row(),
                                Some(language.shell_program_description()),
                                shell_program_control,
                                self.colors,
                            ),
                            settings_row(
                                language.shell_args_row(),
                                Some(language.shell_args_description()),
                                shell_args_control,
                                self.colors,
                            ),
                            settings_row(
                                language.shell_integration_path_row(),
                                Some(language.shell_integration_path_description()),
                                shell_integration_path_control,
                                self.colors,
                            ),
                        ],
                        self.colors,
                    ),
                ]
            }
            SettingsSection::Keybindings => self.render_keybindings_section(language, cx),
            SettingsSection::Profiles => self.render_profiles_section(language, window, cx),
            SettingsSection::About => self.render_about_section(language, cx),
        }
    }
}

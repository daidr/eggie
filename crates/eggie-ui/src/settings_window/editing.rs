use super::*;

impl SettingsWindow {
    pub(super) fn set_theme_mode(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.theme_mode = mode, cx)
        });
    }

    /// Switch between "the terminal follows the interface theme" and "the terminal has its own pair".
    ///
    /// Opting into the custom pair seeds it from the interface pair (see
    /// [`AppSettings::use_custom_terminal_themes`]) so the toggle itself changes nothing on screen.
    /// Going back keeps the custom pair on disk, so toggling twice does not discard the choice.
    pub(super) fn set_terminal_theme_source(&mut self, custom: bool, cx: &mut Context<Self>) {
        let lua = &self.settings.read(cx).lua_config().registered;
        if lua.terminal_light_theme.is_some() || lua.terminal_dark_theme.is_some() {
            return;
        }
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |config| {
                    if custom {
                        config.use_custom_terminal_themes();
                    } else {
                        config.terminal_theme_source = TerminalThemeSource::FollowInterface;
                    }
                },
                cx,
            )
        });
    }

    /// Light up `name` as a transient hover preview across the interface and real terminals,
    /// attributed to `kind`'s slot so it resolves through the same layering the committed
    /// value would. No-op if the name doesn't resolve to a known theme, or if `kind` picks a font.
    pub(super) fn preview_theme(&mut self, kind: SelectorKind, name: &str, cx: &mut Context<Self>) {
        if self.lua_manages_selector(kind, cx) {
            return;
        }
        let profile_id = self.selected_profile_id(cx);
        let Some(slot) = kind.preview_slot(profile_id) else {
            return;
        };
        if let Some(theme) = theme_catalog().theme_by_name(name) {
            self.settings
                .update(cx, |store, cx| store.set_theme_preview(slot, theme, cx));
        }
    }

    /// Drop any active hover preview, restoring the persisted theme everywhere.
    pub(super) fn clear_theme_preview(&mut self, cx: &mut Context<Self>) {
        self.settings
            .update(cx, |store, cx| store.clear_theme_preview(cx));
    }

    pub(super) fn set_bell_mode(&mut self, mode: BellMode, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.bell_mode = mode, cx)
        });
    }

    pub(super) fn set_osc_clipboard_read(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.allow_osc_clipboard_read = enabled, cx)
        });
    }

    pub(super) fn set_detect_urls(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.detect_urls = enabled, cx)
        });
    }

    pub(super) fn set_shell_integration_path(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.shell_integration_path = enabled, cx)
        });
        self.update_default_profile(|profile| profile.shell_integration_path = enabled, cx);
    }

    pub(super) fn set_copy_on_select(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.copy_on_select = enabled, cx)
        });
    }

    pub(super) fn set_auto_check_updates(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.auto_check_updates = enabled, cx)
        });
    }

    pub(super) fn set_update_channel(&mut self, channel: UpdateChannel, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.update_channel = channel, cx)
        });
        // Re-check against the newly selected channel's feed so a switch is
        // reflected immediately (silent: a failure just stays quiet).
        self.updates.update(cx, |controller, cx| {
            controller.check(true, channel.slug(), cx)
        });
    }

    pub(super) fn set_ligatures(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.ligatures = enabled, cx)
        });
    }

    pub(super) fn set_shaping_break_cursor(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.font_shaping_break_cursor = enabled, cx)
        });
    }

    pub(super) fn set_font_thicken(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.font_thicken = enabled, cx)
        });
    }

    pub(super) fn change_font_thicken_strength(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    settings.font_thicken_strength =
                        (i32::from(settings.font_thicken_strength) + delta).clamp(0, 255) as u8;
                },
                cx,
            )
        });
    }

    pub(super) fn set_synthetic_style(
        &mut self,
        kind: SyntheticStyleKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let field = match kind {
                        SyntheticStyleKind::Bold => &mut settings.font_synthetic_style.bold,
                        SyntheticStyleKind::Italic => &mut settings.font_synthetic_style.italic,
                        SyntheticStyleKind::BoldItalic => {
                            &mut settings.font_synthetic_style.bold_italic
                        }
                    };
                    *field = enabled;
                },
                cx,
            )
        });
    }

    pub(super) fn set_cursor_shape(&mut self, shape: CursorShapeSetting, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.cursor_shape = shape, cx)
        });
    }

    pub(super) fn set_cursor_blink(&mut self, blink: CursorBlink, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.cursor_blink = blink, cx)
        });
    }

    pub(super) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.language = language, cx)
        });
    }

    pub(super) fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        if self
            .settings
            .read(cx)
            .lua_config()
            .registered
            .terminal_font
            .is_some()
        {
            return;
        }
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| settings.font_size = (settings.font_size + delta).round(),
                cx,
            )
        });
    }

    pub(super) fn change_minimum_contrast(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    settings.minimum_contrast =
                        ((settings.minimum_contrast + delta) * 10.).round() / 10.;
                },
                cx,
            )
        });
    }

    pub(super) fn change_terminal_padding(
        &mut self,
        axis: PaddingAxis,
        delta: f32,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let value = match axis {
                        PaddingAxis::Horizontal => &mut settings.terminal_padding_x,
                        PaddingAxis::Vertical => &mut settings.terminal_padding_y,
                    };
                    *value = (*value + delta).round();
                },
                cx,
            )
        });
    }

    /// Step a font-metric adjustment by `delta` integer pixels. The GUI only edits the integer
    /// (absolute) form; a value of 0 clears the entry back to "unset". A hand-written percent value
    /// is treated as a 0 baseline for stepping (the next step replaces it with an integer delta).
    pub(super) fn change_metric_adjustment(
        &mut self,
        kind: MetricAdjustmentKind,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let slot = kind.slot(&mut settings.font_metrics);
                    let current = slot
                        .as_deref()
                        .and_then(|text| text.trim().parse::<i32>().ok())
                        .unwrap_or(0);
                    let next = (current + delta).clamp(-64, 64);
                    *slot = if next == 0 {
                        None
                    } else {
                        Some(next.to_string())
                    };
                },
                cx,
            )
        });
    }

    pub(super) fn change_progress_timeout(
        &mut self,
        kind: ProgressTimeoutKind,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let value = match kind {
                        ProgressTimeoutKind::Complete => {
                            &mut settings.progress_complete_timeout_secs
                        }
                        ProgressTimeoutKind::Stale => &mut settings.progress_stale_timeout_secs,
                    };
                    *value = (i64::from(*value) + i64::from(delta)).clamp(
                        i64::from(MIN_PROGRESS_TIMEOUT_SECS),
                        i64::from(MAX_PROGRESS_TIMEOUT_SECS),
                    ) as u32;
                },
                cx,
            )
        });
    }

    pub(super) fn change_scrollback_lines(&mut self, delta: i64, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| {
                    let next = (settings.scrollback_lines as i64 + delta)
                        .clamp(MIN_SCROLLBACK_LINES as i64, MAX_SCROLLBACK_LINES as i64);
                    settings.scrollback_lines = next as usize;
                },
                cx,
            )
        });
    }
}

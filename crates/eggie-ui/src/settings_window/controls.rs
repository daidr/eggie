use super::*;

impl SettingsWindow {
    pub(super) fn render_sidebar_layout_control(
        &self,
        selected: SidebarLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        self.render_segmented(
            "sidebar-layout",
            SidebarLayout::ALL
                .into_iter()
                .map(|layout| {
                    (
                        SharedString::from(format!("{layout:?}")),
                        SharedString::from(language.sidebar_layout_label(layout)),
                        selected == layout,
                    )
                })
                .collect(),
            |window, index, cx| {
                window.settings.update(cx, |store, cx| {
                    store.update(
                        |config| config.sidebar_layout = SidebarLayout::ALL[index],
                        cx,
                    );
                });
            },
            cx,
        )
    }

    pub(super) fn render_segmented(
        &self,
        id_prefix: &str,
        options: Vec<(SharedString, SharedString, bool)>,
        on_pick: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_pick = std::rc::Rc::new(on_pick);
        options
            .into_iter()
            .enumerate()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, (index, (id_suffix, label, selected))| {
                    let on_pick = on_pick.clone();
                    control.child(
                        div()
                            .id(format!("{id_prefix}-{id_suffix}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(
                                cx.listener(move |settings, _, _, cx| on_pick(settings, index, cx)),
                            )
                            .child(label),
                    )
                },
            )
            .into_any_element()
    }

    /// A bordered −/value/+ stepper. The caller pre-formats the current value into `display`,
    /// picks the box width, and supplies `on_step`, which receives the direction (-1 or +1) and
    /// applies the control's own delta before invoking the matching change setter.
    pub(super) fn render_stepper(
        &self,
        dec_id: &str,
        inc_id: &str,
        display: String,
        can_dec: bool,
        can_inc: bool,
        value_box_width: f32,
        on_step: impl Fn(&mut Self, i8, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_step = std::rc::Rc::new(on_step);
        let button = |id: SharedString,
                      label: &'static str,
                      enabled: bool,
                      dir: i8,
                      on_step: std::rc::Rc<dyn Fn(&mut Self, i8, &mut Context<Self>)>,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(30.))
                .text_size(px(16.))
                .cursor_pointer()
                .text_color(rgb(if enabled {
                    self.colors.text
                } else {
                    self.colors.muted
                }))
                .when(enabled, |element| {
                    element
                        .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                        .on_click(cx.listener(move |settings, _, _, cx| on_step(settings, dir, cx)))
                })
                .child(label)
        };
        div()
            .flex()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(rgb(self.colors.border))
            .overflow_hidden()
            .child(button(
                SharedString::from(dec_id.to_owned()),
                "−",
                can_dec,
                -1,
                on_step.clone(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(value_box_width))
                    .h(px(30.))
                    .border_l_1()
                    .border_r_1()
                    .border_color(rgb(self.colors.border))
                    .child(display),
            )
            .child(button(
                SharedString::from(inc_id.to_owned()),
                "+",
                can_inc,
                1,
                on_step,
                cx,
            ))
            .into_any_element()
    }

    pub(super) fn render_mode_control(
        &self,
        selected: ThemeMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = ThemeMode::ALL
            .into_iter()
            .map(|mode| {
                let label = match mode {
                    ThemeMode::Dark => language.theme_mode_dark(),
                    ThemeMode::Light => language.theme_mode_light(),
                    ThemeMode::System => language.theme_mode_system(),
                };
                (
                    SharedString::from(mode.label()),
                    SharedString::from(label),
                    selected == mode,
                )
            })
            .collect();
        self.render_segmented(
            "theme-mode",
            options,
            |settings, index, cx| settings.set_theme_mode(ThemeMode::ALL[index], cx),
            cx,
        )
    }

    /// "Follow the interface theme" vs "use a custom pair" for the terminal.
    pub(super) fn render_terminal_theme_source_control(
        &self,
        selected: TerminalThemeSource,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = TerminalThemeSource::ALL
            .into_iter()
            .map(|source| {
                let label = match source {
                    TerminalThemeSource::FollowInterface => {
                        language.terminal_theme_follow_interface()
                    }
                    TerminalThemeSource::Custom => language.terminal_theme_custom(),
                };
                (
                    SharedString::from(format!("{source:?}")),
                    SharedString::from(label),
                    selected == source,
                )
            })
            .collect();
        self.render_segmented(
            "terminal-theme-source",
            options,
            |settings, index, cx| {
                settings.set_terminal_theme_source(
                    TerminalThemeSource::ALL[index] == TerminalThemeSource::Custom,
                    cx,
                )
            },
            cx,
        )
    }

    pub(super) fn bell_mode_label(language: Language, mode: BellMode) -> &'static str {
        match mode {
            BellMode::Silent => language.bell_mode_silent(),
            BellMode::Flash => language.bell_mode_flash(),
            BellMode::Sound => language.bell_mode_sound(),
            BellMode::FlashAndSound => language.bell_mode_flash_and_sound(),
        }
    }

    pub(super) fn render_bell_control(
        &self,
        selected: BellMode,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let dropdown_position = self.bell_dropdown_bounds.map(|bounds| {
            let popover_height = px(BELL_DROPDOWN_ROW_HEIGHT * BellMode::ALL.len() as f32 + 8.);
            let gap = px(SELECTOR_DROPDOWN_GAP);
            let margin = px(SELECTOR_WINDOW_MARGIN);
            let y = if window.viewport_size().height - bounds.bottom()
                >= popover_height + gap + margin
            {
                bounds.bottom() + gap
            } else {
                bounds.top() - popover_height - gap
            };
            point(bounds.left(), y)
        });
        let control = cx.entity().downgrade();
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let control = control.clone();
                cx.defer(move |cx| {
                    control
                        .update(cx, |settings, cx| {
                            if settings.bell_dropdown_bounds != Some(bounds) {
                                settings.bell_dropdown_bounds = Some(bounds);
                                if settings.bell_dropdown_open {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .id("bell-mode-container")
            .relative()
            .w(px(BELL_DROPDOWN_WIDTH))
            .h(px(32.))
            .child(
                div()
                    .id("bell-mode-trigger")
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .size_full()
                    .px_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(self.colors.border))
                    .bg(rgb(self.colors.panel_alt))
                    .cursor_pointer()
                    .hover(|element| element.border_color(rgb(self.colors.accent)))
                    .on_click(cx.listener(|settings, _, _, cx| {
                        settings.bell_dropdown_open = !settings.bell_dropdown_open;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(Self::bell_mode_label(language, selected)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(self.colors.muted))
                            .child(icon(IconName::ArrowDown)),
                    ),
            )
            .when(self.bell_dropdown_open, |control| {
                let mut dropdown = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    dropdown = dropdown.position(position);
                }
                control.child(
                    deferred(dropdown.child(self.render_bell_dropdown(selected, language, cx)))
                        .priority(2),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_bell_dropdown(
        &self,
        selected: BellMode,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options =
            BellMode::ALL
                .into_iter()
                .fold(div().flex().flex_col().py_1(), |list, mode| {
                    let is_selected = mode == selected;
                    list.child(
                        div()
                            .id(SharedString::from(format!("bell-mode-{}", mode.slug())))
                            .flex()
                            .items_center()
                            .flex_none()
                            .h(px(BELL_DROPDOWN_ROW_HEIGHT))
                            .mx_1()
                            .px_3()
                            .rounded_md()
                            .cursor_pointer()
                            .when(is_selected, |element| {
                                element
                                    .bg(rgb(self.colors.panel_alt))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                            .on_click(cx.listener(move |settings, _, window, cx| {
                                settings.set_bell_mode(mode, cx);
                                settings.bell_dropdown_open = false;
                                let _ = window;
                                cx.notify();
                            }))
                            .child(Self::bell_mode_label(language, mode)),
                    )
                });
        div()
            .id("bell-mode-dropdown")
            .flex()
            .flex_col()
            .w(px(BELL_DROPDOWN_WIDTH))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .shadow_lg()
            // Block mouse/hover for the settings rows painted behind the popover so clicks on its
            // blank areas can't bubble through to a control underneath (canonical GPUI overlay
            // isolation). Children still receive their own events.
            .occlude()
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |settings, _, cx| {
                    if settings.bell_dropdown_open {
                        settings.bell_dropdown_open = false;
                        cx.notify();
                    }
                });
            }))
            .child(options)
            .into_any_element()
    }

    pub(super) fn render_osc_clipboard_read_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.block()), (true, language.allow())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "osc-clipboard-read",
            options,
            |settings, index, cx| settings.set_osc_clipboard_read(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_detect_urls_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "detect-urls",
            options,
            |settings, index, cx| settings.set_detect_urls(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_auto_check_updates_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "auto-check-updates",
            options,
            |settings, index, cx| settings.set_auto_check_updates(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_update_channel_control(
        &self,
        selected: UpdateChannel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = UpdateChannel::ALL
            .into_iter()
            .map(|channel| {
                (
                    SharedString::from(channel.slug()),
                    SharedString::from(language.update_channel_label(channel)),
                    selected == channel,
                )
            })
            .collect();
        self.render_segmented(
            "update-channel",
            options,
            |settings, index, cx| settings.set_update_channel(UpdateChannel::ALL[index], cx),
            cx,
        )
    }

    pub(super) fn render_shell_integration_path_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "shell-integration-path",
            options,
            |settings, index, cx| settings.set_shell_integration_path(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_synthetic_style_control(
        &self,
        kind: SyntheticStyleKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            &format!("synthetic-{}", kind.slug()),
            options,
            move |settings, index, cx| settings.set_synthetic_style(kind, index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_ligatures_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "ligatures",
            options,
            |settings, index, cx| settings.set_ligatures(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_shaping_break_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "shaping-break",
            options,
            |settings, index, cx| settings.set_shaping_break_cursor(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_font_thicken_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "font-thicken",
            options,
            |settings, index, cx| settings.set_font_thicken(index == 1, cx),
            cx,
        )
    }

    pub(super) fn render_font_thicken_strength_control(
        &self,
        strength: u8,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_stepper(
            "decrease-font-thicken-strength",
            "increase-font-thicken-strength",
            format!("{strength}"),
            strength > 0,
            strength < 255,
            54.,
            |settings, dir, cx| settings.change_font_thicken_strength(i32::from(dir) * 16, cx),
            cx,
        )
    }

    pub(super) fn render_copy_on_select_control(
        &self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = [(false, language.disabled()), (true, language.enabled())]
            .into_iter()
            .map(|(value, label)| {
                (
                    SharedString::from(label),
                    SharedString::from(label),
                    enabled == value,
                )
            })
            .collect();
        self.render_segmented(
            "copy-on-select",
            options,
            |settings, index, cx| settings.set_copy_on_select(index == 1, cx),
            cx,
        )
    }

    pub(super) fn metric_adjustment_labels(
        language: Language,
        kind: MetricAdjustmentKind,
    ) -> (&'static str, &'static str) {
        match kind {
            MetricAdjustmentKind::CellWidth => (
                language.adjust_cell_width_row(),
                language.adjust_cell_width_description(),
            ),
            MetricAdjustmentKind::CellHeight => (
                language.adjust_cell_height_row(),
                language.adjust_cell_height_description(),
            ),
            MetricAdjustmentKind::FontBaseline => (
                language.adjust_font_baseline_row(),
                language.adjust_font_baseline_description(),
            ),
            MetricAdjustmentKind::UnderlinePosition => (
                language.adjust_underline_position_row(),
                language.adjust_underline_position_description(),
            ),
            MetricAdjustmentKind::UnderlineThickness => (
                language.adjust_underline_thickness_row(),
                language.adjust_underline_thickness_description(),
            ),
            MetricAdjustmentKind::StrikethroughPosition => (
                language.adjust_strikethrough_position_row(),
                language.adjust_strikethrough_position_description(),
            ),
            MetricAdjustmentKind::StrikethroughThickness => (
                language.adjust_strikethrough_thickness_row(),
                language.adjust_strikethrough_thickness_description(),
            ),
            MetricAdjustmentKind::CursorThickness => (
                language.adjust_cursor_thickness_row(),
                language.adjust_cursor_thickness_description(),
            ),
            MetricAdjustmentKind::BoxThickness => (
                language.adjust_box_thickness_row(),
                language.adjust_box_thickness_description(),
            ),
            MetricAdjustmentKind::IconHeight => (
                language.adjust_icon_height_row(),
                language.adjust_icon_height_description(),
            ),
        }
    }

    pub(super) fn cursor_shape_label(
        language: Language,
        shape: CursorShapeSetting,
    ) -> &'static str {
        match shape {
            CursorShapeSetting::Block => language.cursor_shape_block(),
            CursorShapeSetting::Bar => language.cursor_shape_bar(),
            CursorShapeSetting::Underline => language.cursor_shape_underline(),
            CursorShapeSetting::BlockHollow => language.cursor_shape_block_hollow(),
        }
    }

    pub(super) fn render_cursor_shape_control(
        &self,
        selected: CursorShapeSetting,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        CursorShapeSetting::ALL
            .into_iter()
            .fold(
                div()
                    .flex()
                    .p(px(2.))
                    .rounded_lg()
                    .bg(rgb(self.colors.panel_alt))
                    .border_1()
                    .border_color(rgb(self.colors.border)),
                |control, shape| {
                    control.child(
                        div()
                            .id(format!("cursor-shape-{}", shape.slug()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected == shape, |element| {
                                element
                                    .bg(rgb(self.colors.background))
                                    .text_color(rgb(self.colors.accent))
                            })
                            .on_click(cx.listener(move |settings, _, _, cx| {
                                settings.set_cursor_shape(shape, cx)
                            }))
                            .child(Self::cursor_shape_label(language, shape)),
                    )
                },
            )
            .into_any_element()
    }

    pub(super) fn cursor_blink_label(language: Language, blink: CursorBlink) -> &'static str {
        match blink {
            CursorBlink::Program => language.cursor_blink_program(),
            CursorBlink::On => language.cursor_blink_on(),
            CursorBlink::Off => language.cursor_blink_off(),
        }
    }

    pub(super) fn render_cursor_blink_control(
        &self,
        selected: CursorBlink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = self.settings.read(cx).config().language;
        let options = CursorBlink::ALL
            .into_iter()
            .map(|blink| {
                (
                    SharedString::from(blink.slug()),
                    SharedString::from(Self::cursor_blink_label(language, blink)),
                    selected == blink,
                )
            })
            .collect();
        self.render_segmented(
            "cursor-blink",
            options,
            |settings, index, cx| settings.set_cursor_blink(CursorBlink::ALL[index], cx),
            cx,
        )
    }

    pub(super) fn render_language_control(
        &self,
        selected: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = Language::ALL
            .into_iter()
            .map(|language| {
                (
                    SharedString::from(language.label()),
                    SharedString::from(language.label()),
                    selected == language,
                )
            })
            .collect();
        self.render_segmented(
            "language",
            options,
            |settings, index, cx| settings.set_language(Language::ALL[index], cx),
            cx,
        )
    }

    pub(super) fn render_font_size_control(
        &self,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let managed = self
            .settings
            .read(cx)
            .lua_config()
            .registered
            .terminal_font
            .is_some();
        self.render_stepper(
            "decrease-font-size",
            "increase-font-size",
            if managed {
                format!("{font_size:.0} · init.lua")
            } else {
                format!("{font_size:.0}")
            },
            !managed && font_size > MIN_FONT_SIZE,
            !managed && font_size < MAX_FONT_SIZE,
            54.,
            |settings, dir, cx| settings.change_font_size(f32::from(dir), cx),
            cx,
        )
    }

    pub(super) fn render_minimum_contrast_control(
        &self,
        minimum_contrast: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_stepper(
            "decrease-minimum-contrast",
            "increase-minimum-contrast",
            format!("{minimum_contrast:.1}"),
            minimum_contrast > MIN_MINIMUM_CONTRAST,
            minimum_contrast < MAX_MINIMUM_CONTRAST,
            54.,
            |settings, dir, cx| settings.change_minimum_contrast(f32::from(dir) * 0.1, cx),
            cx,
        )
    }

    pub(super) fn render_terminal_padding_control(
        &self,
        axis: PaddingAxis,
        value: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (decrease_id, increase_id) = match axis {
            PaddingAxis::Horizontal => (
                "decrease-horizontal-terminal-padding",
                "increase-horizontal-terminal-padding",
            ),
            PaddingAxis::Vertical => (
                "decrease-vertical-terminal-padding",
                "increase-vertical-terminal-padding",
            ),
        };
        self.render_stepper(
            decrease_id,
            increase_id,
            format!("{value:.0}"),
            value > MIN_TERMINAL_PADDING,
            value < MAX_TERMINAL_PADDING,
            54.,
            move |settings, dir, cx| settings.change_terminal_padding(axis, f32::from(dir), cx),
            cx,
        )
    }

    pub(super) fn render_metric_adjustment_control(
        &self,
        kind: MetricAdjustmentKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Parse the current integer pixel delta (percent values, hand-edited in settings.json,
        // display as their raw string and step from 0).
        let raw = kind
            .value(&self.settings.read(cx).config().font_metrics)
            .clone();
        let numeric = raw
            .as_deref()
            .and_then(|text| text.trim().parse::<i32>().ok());
        let display = match (&raw, numeric) {
            (Some(text), None) => text.clone(), // percent or other non-integer form
            (_, Some(value)) if value > 0 => format!("+{value}"),
            (_, Some(value)) => format!("{value}"),
            (None, _) => "0".to_owned(),
        };
        let current = numeric.unwrap_or(0);
        let decrease_id = format!("decrease-adjust-{}", kind.slug());
        let increase_id = format!("increase-adjust-{}", kind.slug());
        self.render_stepper(
            &decrease_id,
            &increase_id,
            display,
            current > -64,
            current < 64,
            54.,
            move |settings, dir, cx| settings.change_metric_adjustment(kind, i32::from(dir), cx),
            cx,
        )
    }

    pub(super) fn render_progress_timeout_control(
        &self,
        kind: ProgressTimeoutKind,
        value: u32,
        step: i32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (decrease_id, increase_id) = match kind {
            ProgressTimeoutKind::Complete => (
                "decrease-progress-complete-timeout",
                "increase-progress-complete-timeout",
            ),
            ProgressTimeoutKind::Stale => (
                "decrease-progress-stale-timeout",
                "increase-progress-stale-timeout",
            ),
        };
        self.render_stepper(
            decrease_id,
            increase_id,
            format!("{value}s"),
            value > MIN_PROGRESS_TIMEOUT_SECS,
            value < MAX_PROGRESS_TIMEOUT_SECS,
            62.,
            move |settings, dir, cx| {
                settings.change_progress_timeout(kind, i32::from(dir) * step, cx)
            },
            cx,
        )
    }

    pub(super) fn render_scrollback_control(
        &self,
        value: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_stepper(
            "decrease-scrollback-lines",
            "increase-scrollback-lines",
            value.to_string(),
            value > MIN_SCROLLBACK_LINES,
            value < MAX_SCROLLBACK_LINES,
            72.,
            |settings, dir, cx| {
                settings.change_scrollback_lines(i64::from(dir) * SCROLLBACK_LINES_STEP, cx)
            },
            cx,
        )
    }

    /// Wrap a persistent shell `TextInput` entity in a bordered, fixed-width control box for a
    /// settings row. `id` disambiguates the two boxes (program vs args).
    pub(super) fn render_shell_input_control(
        &self,
        id: &'static str,
        input: &Entity<TextInput>,
    ) -> AnyElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .w(px(220.))
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
}

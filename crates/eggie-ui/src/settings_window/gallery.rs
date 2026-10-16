use super::*;

impl SettingsWindow {
    pub(super) fn render_theme_search(
        &self,
        kind: SelectorKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dropdown_position = self.selector_dropdown_position(kind, window);
        let selector = cx.entity().downgrade();
        let language = self.settings.read(cx).config().language;
        let search_label = if kind.is_dark() {
            language.search_dark_themes()
        } else {
            language.search_light_themes()
        };
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let selector = selector.clone();
                cx.defer(move |cx| {
                    selector
                        .update(cx, |settings, cx| {
                            let stored = &mut settings.selector_bounds[kind.index()];
                            if *stored != Some(bounds) {
                                *stored = Some(bounds);
                                if settings.open_selector == Some(kind) {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .id(format!("theme-search-container-{}", kind.index()))
            .relative()
            .flex_none()
            .size(px(26.))
            .child(
                div()
                    .id(format!("theme-search-{}", kind.index()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .rounded_md()
                    .text_color(rgb(self.colors.muted))
                    .cursor_pointer()
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .tooltip(tooltip_text(search_label, self.colors))
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        if settings.lua_manages_selector(kind, cx) {
                            return;
                        }
                        if settings.open_selector == Some(kind) {
                            settings.open_selector = None;
                            settings.clear_theme_preview(cx);
                        } else {
                            settings.open_selector = Some(kind);
                            settings.selector_search_input.update(cx, |input, cx| {
                                input.set_placeholder(search_label);
                                input.set_content("", cx);
                            });
                            settings
                                .selector_scroll_handle
                                .set_offset(point(px(0.), px(0.)));
                            settings
                                .selector_search_input
                                .read(cx)
                                .focus_handle()
                                .focus(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(icon_sized(IconName::Search, 14.)),
            )
            .when(self.open_selector == Some(kind), |element| {
                let mut dropdown = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    dropdown = dropdown.position(position);
                }
                element.child(
                    deferred(dropdown.child(self.render_selector_dropdown(kind, window, cx)))
                        .priority(2),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_theme_card(
        &self,
        kind: SelectorKind,
        name: &str,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let managed = self.lua_manages_selector(kind, cx);
        let theme = theme_catalog()
            .theme_by_name(name)
            .expect("catalog name must resolve to a theme");
        let colors = self.colors;
        let card_fill = theme.background;
        let card_edge = blend_rgb(card_fill, theme.foreground, 0.2);
        let sample_fill = blend_rgb(card_fill, theme.foreground, 0.055);
        let chip_fill = theme.palette[0];
        let chip_text = readable_on(chip_fill);
        // Match the horizontal card-edge inset to the marker's vertical card-edge inset.
        let sample_padding = 11. + (42. - 29.) / 2. - 8.;
        let name_owned = name.to_owned();
        let hover_name = name_owned.clone();
        let selected_name = name_owned.clone();
        div()
            .id(format!("theme-card-{}-{name}", kind.index()))
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .w(px(THEME_CARD_WIDTH))
            .when(!managed, |element| element.cursor_pointer())
            .when(managed, |element| element.opacity(0.58))
            .on_hover(cx.listener(move |settings, hovered: &bool, _, cx| {
                if *hovered {
                    settings.preview_theme(kind, &hover_name, cx);
                }
            }))
            .on_click(cx.listener(move |settings, _, _, cx| {
                if settings.lua_manages_selector(kind, cx) {
                    return;
                }
                settings.clear_theme_preview(cx);
                settings.commit_selector_choice(kind, selected_name.clone(), cx);
                cx.notify();
            }))
            .child(
                div()
                    .relative()
                    .w(px(THEME_CARD_WIDTH))
                    .h(px(THEME_CARD_HEIGHT))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| {
                                if selected {
                                    paint_continuous_rect(
                                        window,
                                        bounds,
                                        0.,
                                        THEME_CARD_RADIUS,
                                        colors.accent,
                                    );
                                    paint_continuous_rect(
                                        window,
                                        bounds,
                                        3.,
                                        THEME_CARD_RADIUS,
                                        colors.panel,
                                    );
                                }
                                paint_continuous_rect(
                                    window,
                                    bounds,
                                    THEME_CARD_BASE_INSET,
                                    THEME_CARD_RADIUS,
                                    card_edge,
                                );
                                paint_continuous_rect(
                                    window,
                                    bounds,
                                    THEME_CARD_BASE_INSET + 1.,
                                    THEME_CARD_RADIUS,
                                    card_fill,
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_between()
                            .absolute()
                            .top(px(THEME_CARD_BASE_INSET + 11.))
                            .bottom(px(THEME_CARD_BASE_INSET + 11.))
                            .left(px(THEME_CARD_BASE_INSET + 8.))
                            .right(px(THEME_CARD_BASE_INSET + 8.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .h(px(42.))
                                    .px(px(sample_padding))
                                    .rounded_xl()
                                    .bg(rgb(sample_fill))
                                    .child(
                                        div()
                                            .text_size(px(26.))
                                            .text_color(rgb(theme.foreground))
                                            .child("Aa"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .w(px(34.))
                                            .h(px(29.))
                                            .rounded_md()
                                            .bg(rgb(chip_fill))
                                            .font_family("Menlo")
                                            .text_size(px(14.))
                                            .text_color(rgb(chip_text))
                                            .child("~/"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_end()
                                            .justify_center()
                                            .w(px(15.))
                                            .h(px(29.))
                                            .pb(px(3.))
                                            .rounded_md()
                                            .bg(rgb(theme.cursor))
                                            .child(
                                                div()
                                                    .w(px(7.))
                                                    .h(px(2.))
                                                    .bg(rgb(theme.cursor_text)),
                                            ),
                                    ),
                            )
                            .child(div().flex().items_center().children(
                                theme.palette[..8].iter().enumerate().map(|(index, color)| {
                                    div()
                                        .flex_none()
                                        .size(px(21.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgb(card_fill))
                                        .bg(rgb(*color))
                                        .when(index > 0, |element| element.ml(px(-5.)))
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_center()
                    .text_size(px(12.))
                    .text_color(rgb(if selected { colors.text } else { colors.muted }))
                    .when(selected, |element| {
                        element.font_weight(gpui::FontWeight::SEMIBOLD)
                    })
                    .child(name_owned),
            )
            .into_any_element()
    }

    pub(super) fn render_theme_gallery(
        &self,
        kind: SelectorKind,
        label: &'static str,
        selected: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let managed = self.lua_manages_selector(kind, cx);
        let scroll_handle = &self.theme_gallery_scroll_handles[kind.index()];
        let offset_x = -f32::from(scroll_handle.offset().x);
        let max_offset_x = f32::from(scroll_handle.max_offset().x);
        let viewport_width = match f32::from(scroll_handle.bounds().size.width) {
            width if width > 0. => width,
            _ => 720.,
        };
        let card_step = THEME_CARD_WIDTH + 12.;
        let preload = card_step * 2.;
        let visible_start = (offset_x - preload).max(0.);
        let visible_end = offset_x + viewport_width + preload;
        let default = if kind.is_dark() {
            DEFAULT_DARK_THEME
        } else {
            DEFAULT_LIGHT_THEME
        };
        let names = self.selector_names(kind);
        let has_default = names.iter().any(|name| name == default);
        let total_width = names.len() as f32 * card_step + if has_default { 13. } else { 0. } - 12.;
        let mut visible_items = Vec::new();
        let mut item_x = 0.;
        if has_default {
            if item_x + THEME_CARD_WIDTH >= visible_start && item_x <= visible_end {
                visible_items.push((
                    item_x,
                    self.render_theme_card(kind, default, selected == default, cx),
                ));
            }
            item_x += card_step;
            if item_x + 1. >= visible_start && item_x <= visible_end {
                visible_items.push((
                    item_x,
                    div()
                        .flex_none()
                        .w(px(1.))
                        .h(px(THEME_CARD_HEIGHT - 8.))
                        .mt_1()
                        .bg(rgb(self.colors.border))
                        .into_any_element(),
                ));
            }
            item_x += 13.;
        }
        for name in names.iter().filter(|name| *name != default) {
            if item_x + THEME_CARD_WIDTH >= visible_start && item_x <= visible_end {
                visible_items.push((
                    item_x,
                    self.render_theme_card(kind, name, name == selected, cx),
                ));
            }
            item_x += card_step;
        }
        let mut content = div()
            .flex()
            .flex_none()
            .items_start()
            .gap(px(12.))
            .w(px(total_width));
        if let Some((first_x, _)) = visible_items.first()
            && *first_x > 0.
        {
            content = content.child(div().flex_none().w(px(first_x - 12.)).h(px(1.)));
        }
        for (_, item) in visible_items {
            content = content.child(item);
        }
        let strip = div()
            .id(format!("theme-gallery-{}", kind.index()))
            .min_w_0()
            .w_full()
            .overflow_x_scroll()
            .track_scroll(scroll_handle)
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(|settings, hovered: &bool, _, cx| {
                if !*hovered {
                    settings.clear_theme_preview(cx);
                }
            }))
            .child(content);
        let panel_color = rgb(self.colors.panel);
        let mut strip_wrapper = div().relative().min_w_0().w_full().child(strip);
        if offset_x > 0.5 {
            strip_wrapper =
                strip_wrapper.child(div().absolute().left_0().top_0().bottom_0().w(px(28.)).bg(
                    linear_gradient(
                        90.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    ),
                ));
        }
        if offset_x < max_offset_x - 0.5 {
            strip_wrapper =
                strip_wrapper.child(div().absolute().right_0().top_0().bottom_0().w(px(28.)).bg(
                    linear_gradient(
                        270.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    ),
                ));
        }
        self.search_anchor(label)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .min_w_0()
                    .w_full()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .w_full()
                            .child(
                                div()
                                    .id(format!("theme-gallery-label-{}", kind.index()))
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(if managed {
                                        format!("{label} · init.lua")
                                    } else {
                                        label.to_owned()
                                    }),
                            )
                            .child(self.render_theme_search(kind, window, cx)),
                    )
                    .child(strip_wrapper),
            )
            .into_any_element()
    }
}

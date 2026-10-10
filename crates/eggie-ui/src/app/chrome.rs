use super::*;

impl EggieApp {
    pub(super) fn with_window_drag_handlers(
        element: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        element
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.moving_window = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|app, _, _, _| app.moving_window = false),
            )
            .on_mouse_move(cx.listener(|app, _, window, _| {
                if app.moving_window {
                    app.moving_window = false;
                    window.start_window_move();
                }
            }))
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
    }

    pub(super) fn render_window_drag_region(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut titlebar = Self::with_window_drag_handlers(
            div()
                .id("window-drag-region")
                .flex()
                .flex_none()
                .items_center()
                .justify_end()
                .w_full()
                .h(px(TITLEBAR_HEIGHT))
                .pl(px(72.))
                .pr_2(),
            cx,
        );
        if !self.left_sidebar_collapsed {
            if let Some(indicator) = self.render_update_indicator(cx) {
                titlebar = titlebar.child(indicator);
            }
            titlebar = titlebar.child(icon_button(
                IconName::PanelLeftOpen,
                "collapse-left-sidebar",
                self.colors,
                Some(self.language.collapse_sidebar_tooltip().into()),
                cx.listener(|app, _, _, cx| {
                    cx.stop_propagation();
                    app.toggle_left_sidebar(cx);
                }),
            ));
        }
        titlebar.into_any_element()
    }

    /// The update indicator in the title bar (left of the collapse-sidebar button): an icon for
    /// the current update state, plus a mini progress bar while downloading. Hidden when idle.
    pub(super) fn render_update_indicator(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use eggie_update::UpdateState;

        let state = self.updates.read(cx).state().clone();
        let (icon_name, tooltip, tinted): (IconName, SharedString, bool) = match &state {
            UpdateState::Available(_) => (
                IconName::Download,
                self.language.update_available_tooltip().into(),
                true,
            ),
            UpdateState::Downloading { .. } => (
                IconName::Download,
                self.language.update_downloading().into(),
                false,
            ),
            UpdateState::Ready { .. } => (
                IconName::CheckmarkCircle,
                self.language.update_ready_tooltip().into(),
                true,
            ),
            UpdateState::Error(_) => (
                IconName::AlertCircle,
                self.language.update_error_title().into(),
                false,
            ),
            // `UpToDate` is a manual-check result shown in the update window,
            // not a persistent sidebar state — leave the sidebar clean.
            _ => return None,
        };
        let colors = self.colors;
        let opens_window = matches!(state, UpdateState::Available(_));
        let control = cx.entity().downgrade();

        let mut indicator = div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_0p5()
            .px_1()
            .h_full()
            // Capture on-screen bounds so the popover can anchor just below the icon.
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let control = control.clone();
                cx.defer(move |cx| {
                    control
                        .update(cx, |app, cx| {
                            if app.update_toggle_bounds != Some(bounds) {
                                app.update_toggle_bounds = Some(bounds);
                                if app.update_popover_open {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .child(
                div()
                    .id("update-indicator-toggle")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded_lg()
                    .when(tinted, |this| this.text_color(rgb(colors.accent)))
                    .when(!tinted, |this| this.text_color(rgb(colors.muted)))
                    .hover(move |element| {
                        element
                            .bg(rgb(colors.panel_alt))
                            .text_color(rgb(colors.text))
                    })
                    .tooltip(tooltip_text(tooltip, colors))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if opens_window {
                            crate::update_window::open_update_window(
                                app.updates.clone(),
                                app.settings.clone(),
                                cx,
                            );
                        } else {
                            app.update_popover_open = !app.update_popover_open;
                            cx.notify();
                        }
                        let _ = window;
                    }))
                    .child(icon(icon_name)),
            );

        // Mini progress bar under the icon while downloading.
        if let UpdateState::Downloading { progress, .. } = &state {
            let total = progress.total.unwrap_or(0).max(1);
            let fraction = (progress.downloaded as f32 / total as f32).clamp(0., 1.);
            indicator = indicator.child(
                div()
                    .w(px(20.))
                    .h(px(2.))
                    .rounded_full()
                    .bg(rgb(colors.panel_alt))
                    .child(
                        div()
                            .h(px(2.))
                            .rounded_full()
                            .bg(rgb(colors.accent))
                            .w(px(20. * fraction)),
                    ),
            );
        }

        Some(indicator.into_any_element())
    }

    /// Popover for the download / ready / error states, anchored under the update indicator.
    pub(super) fn render_update_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use eggie_update::UpdateState;

        if !self.update_popover_open {
            return None;
        }
        let state = self.updates.read(cx).state().clone();
        if matches!(
            state,
            UpdateState::Idle | UpdateState::Checking | UpdateState::Available(_)
        ) {
            return None;
        }
        let colors = self.colors;
        let language = self.language;

        let body: AnyElement = match &state {
            UpdateState::Downloading { progress, .. } => {
                let total = progress.total.unwrap_or(0).max(1);
                let fraction = (progress.downloaded as f32 / total as f32).clamp(0., 1.);
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(colors.text))
                            .child(language.update_downloading()),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(5.))
                            .rounded_full()
                            .bg(rgb(colors.panel_alt))
                            .child(
                                div()
                                    .h(px(5.))
                                    .rounded_full()
                                    .bg(rgb(colors.accent))
                                    .w(relative(fraction)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(11.))
                            .text_color(rgb(colors.muted))
                            .child(div().child(format!(
                                "{}/s",
                                format_update_speed(progress.bytes_per_sec)
                            )))
                            .child(div().child(format!("{:.0}%", fraction * 100.))),
                    )
                    .into_any_element()
            }
            UpdateState::Ready { release, .. } => {
                let compatible = release.daemon_compatible();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_size(px(13.)).text_color(rgb(colors.text)).child(
                        if compatible {
                            language.update_ready_restart()
                        } else {
                            language.update_ready_restart_with_daemon()
                        },
                    ))
                    .when(!compatible, |this| {
                        this.child(
                            div()
                                .p_2()
                                .rounded_lg()
                                .border_1()
                                .border_color(rgb(0xd08700))
                                .bg(rgba(0xd0870022))
                                .text_size(px(11.))
                                .text_color(rgb(0xb87400))
                                .child(language.update_daemon_warning()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(popover_button(
                                language.update_later(),
                                false,
                                colors,
                                cx.listener(|app, _, _, cx| {
                                    app.update_popover_open = false;
                                    app.updates
                                        .update(cx, |controller, cx| controller.dismiss(cx));
                                    cx.notify();
                                }),
                            ))
                            .child(popover_button(
                                if compatible {
                                    language.update_restart()
                                } else {
                                    language.update_restart_with_daemon()
                                },
                                true,
                                colors,
                                cx.listener(|app, _, _, cx| {
                                    app.updates.update(cx, |controller, cx| {
                                        if let Err(error) = controller.install_and_restart(cx) {
                                            controller.report_error(format!("{error:#}"), cx);
                                        }
                                    });
                                }),
                            )),
                    )
                    .into_any_element()
            }
            UpdateState::Error(message) => {
                let message = message.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0xff6b6b))
                            .child(format!("{}: {message}", language.update_error_title())),
                    )
                    .child(div().flex().justify_end().child(popover_button(
                        language.update_retry(),
                        true,
                        colors,
                        cx.listener(|app, _, _, cx| {
                            app.update_popover_open = false;
                            let channel = app.settings.read(cx).config().update_channel.slug();
                            app.updates
                                .update(cx, |controller, cx| controller.check(false, channel, cx));
                            cx.notify();
                        }),
                    )))
                    .into_any_element()
            }
            UpdateState::UpToDate => div()
                .text_size(px(13.))
                .text_color(rgb(colors.text))
                .child(language.update_up_to_date())
                .into_any_element(),
            _ => div().into_any_element(),
        };

        let popover = div()
            .flex()
            .flex_col()
            .w(px(260.))
            .p_3()
            .rounded_xl()
            .border_1()
            .border_color(rgb(colors.border))
            .bg(rgb(colors.panel))
            .shadow_lg()
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |app, _, _, cx| {
                if app.update_popover_open {
                    app.update_popover_open = false;
                    cx.notify();
                }
            }))
            .child(body);

        let anchored = match self.update_toggle_bounds {
            Some(bounds) => anchored()
                .anchor(Anchor::TopRight)
                .position(point(bounds.right(), bounds.bottom() + px(4.)))
                .snap_to_window_with_margin(px(8.))
                .child(popover),
            None => anchored()
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(px(8.))
                .child(popover),
        };
        Some(deferred(anchored).priority(2).into_any_element())
    }

    pub(super) fn render_sidebar_resize_handle(
        &self,
        edge: SidebarEdge,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match edge {
            SidebarEdge::Left => "resize-left-sidebar",
            SidebarEdge::Right => "resize-right-sidebar",
        };
        div()
            .id(id)
            .flex_none()
            .w(px(SIDEBAR_RESIZE_HANDLE_WIDTH))
            .h_full()
            .ml(px(-SIDEBAR_RESIZE_HANDLE_WIDTH / 2.))
            .mr(px(-SIDEBAR_RESIZE_HANDLE_WIDTH / 2.))
            .cursor_col_resize()
            .hover(|element| element.bg(rgb(self.colors.accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event, _, cx| app.start_sidebar_resize(edge, event, cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_split_divider(
        &self,
        axis: SplitAxis,
        split_id: SplitId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let horizontal = axis == SplitAxis::Horizontal;
        // The handle is a wide, transparent grab area so it is easy to click, while the visible
        // line inside it stays thin. Filling the whole grab area with the accent color would make
        // the divider look far too thick.
        let handle = SPLIT_DIVIDER_HANDLE_WIDTH;
        let line = SPLIT_DIVIDER_LINE_WIDTH;
        let group_name: SharedString = format!("split-divider-{split_id}").into();
        let mut divider = div()
            .id(format!("split-divider-{split_id}"))
            .group(group_name.clone())
            .flex_none()
            .flex()
            .items_center()
            .justify_center();
        let mut visible = div();
        if horizontal {
            // Symmetric negative margins straddle the handle across the seam so it is centered on
            // it and consumes zero net layout width (same trick as the sidebar resize handle).
            // Rendered after the first pane, it stacks above both panes and receives mouse events.
            divider = divider
                .w(px(handle))
                .h_full()
                .ml(px(-handle / 2.))
                .mr(px(-handle / 2.))
                .cursor_col_resize();
            visible = visible.w(px(line)).h_full();
        } else {
            divider = divider
                .h(px(handle))
                .w_full()
                .mt(px(-handle / 2.))
                .mb(px(-handle / 2.))
                .cursor_row_resize();
            visible = visible.h(px(line)).w_full();
        }
        divider
            .child(
                // A thin line, centered on the seam, that lights up in the accent color while the
                // grab area is hovered. `group_hover` reacts to hovering the surrounding handle,
                // not just the line itself.
                visible.group_hover(group_name, |style| style.bg(rgb(self.colors.accent))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, _, _, cx| app.start_split_resize(split_id, cx)),
            )
            .into_any_element()
    }

    pub(super) fn render_traffic_light_tab_bar_region(
        &self,
        group_id: GroupId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Self::with_window_drag_handlers(
            div()
                .id(format!("traffic-light-tab-bar-{group_id}"))
                .flex_none()
                .w(px(TRAFFIC_LIGHT_INSET_WIDTH))
                .h_full(),
            cx,
        )
        .into_any_element()
    }

    // Right-click menu on the tab bar's "+" button: list every profile and open a tab running the
    // one picked. Mirrors `show_native_tab_context_menu` — the menu is an AppKit modal and runs a
    // nested event loop, so it is shown from a foreground future after this listener has returned,
    // and the picked profile is re-fetched by id after the menu closes rather than carried across
    // the await (it may have been deleted while the menu was open).
}

use super::*;
use std::cell::Cell;

const PREVIEW_PADDING: f32 = 12.;
const PREVIEW_GAP: f32 = 8.;
const PREVIEW_HEADER_HEIGHT: f32 = 26.;
const PREVIEW_LINE_HEIGHT: f32 = 15.;
const PREVIEW_BORDER: f32 = 1.;
const PREVIEW_HORIZONTAL_CHROME: f32 = 2. * (PREVIEW_PADDING + PREVIEW_BORDER);
const PREVIEW_VERTICAL_CHROME: f32 = PREVIEW_HORIZONTAL_CHROME
    + 3. * PREVIEW_GAP
    + PREVIEW_HEADER_HEIGHT
    + 2. * PREVIEW_LINE_HEIGHT
    + 2. * PREVIEW_BORDER;

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalPreviewLayout {
    image: gpui::Size<Pixels>,
    content: gpui::Size<Pixels>,
}

impl TerminalPreviewLayout {
    fn fit(
        grid: Option<TerminalSize>,
        cell: gpui::Size<Pixels>,
        available: gpui::Size<Pixels>,
    ) -> Self {
        let max_width = f32::from(available.width)
            .min(420.)
            .max(PREVIEW_HORIZONTAL_CHROME + 3.);
        let max_height = f32::from(available.height)
            .min(430.)
            .max(PREVIEW_VERTICAL_CHROME + 1.);
        let natural = grid.map_or(size(px(320.), px(180.)), |grid| {
            size(
                cell.width * f32::from(grid.columns.max(1)),
                cell.height * f32::from(grid.rows.max(1)),
            )
        });
        let scale = ((max_width - PREVIEW_HORIZONTAL_CHROME - 2. * PREVIEW_BORDER)
            / f32::from(natural.width))
        .min((max_height - PREVIEW_VERTICAL_CHROME).min(300.) / f32::from(natural.height))
        .min(1.);
        let image = size(natural.width * scale, natural.height * scale);
        Self {
            image,
            content: size(
                px(
                    (f32::from(image.width) + PREVIEW_HORIZONTAL_CHROME + 2. * PREVIEW_BORDER)
                        .max(280_f32.min(max_width)),
                ),
                image.height + px(PREVIEW_VERTICAL_CHROME),
            ),
        }
    }
}

fn preview_available_space(window: &Window, cx: &App) -> gpui::Size<Pixels> {
    let visible = window.display(cx).map_or(window.bounds().size, |display| {
        display.visible_bounds().size
    });
    // Leave room for the native shadow and a little space from the display edge.
    size(
        (visible.width - px(48.)).max(px(1.)),
        (visible.height - px(48.)).max(px(1.)),
    )
}

fn preview_cell_size(
    window: &Window,
    font_family: SharedString,
    font_size: f32,
    adjustments: eggie_config::font::ResolvedMetricAdjustments,
) -> gpui::Size<Pixels> {
    let (width, height) = crate::terminal_renderer::terminal_font_metrics(
        font_family.as_ref(),
        px(font_size),
        window.scale_factor(),
        adjustments,
    );
    size(width, height)
}

impl EggieApp {
    pub(super) fn dismiss_terminal_preview(&mut self, cx: &mut Context<Self>) {
        self.sidebar.hovered_terminal_preview = None;
        self.sidebar.terminal_preview_revision += 1;
        if let Some(preview) = self.sidebar.terminal_preview.take() {
            preview
                .window
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .ok();
        }
    }

    pub(super) fn terminal_preview_hover(
        &mut self,
        target: TerminalPreviewAnchor,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if hovered && (cx.has_active_drag() || self.sidebar.project_selector.is_some()) {
            return;
        }
        if hovered {
            self.sidebar.hovered_terminal_preview = Some(target);
            self.sidebar.terminal_preview_revision += 1;
            if self
                .sidebar
                .terminal_preview
                .as_ref()
                .map(|preview| preview.anchor)
                != Some(target)
            {
                if self.sidebar.terminal_preview.is_some() {
                    // An open preview follows the pointer; only the first preview waits.
                    self.show_terminal_preview(target, window, cx);
                } else {
                    let revision = self.sidebar.terminal_preview_revision;
                    let executor = cx.background_executor().clone();
                    cx.spawn(async move |app, cx| {
                        executor.timer(TERMINAL_PREVIEW_HOVER_DELAY).await;
                        app.update_in(cx, |app, window, cx| {
                            if app.sidebar.hovered_terminal_preview == Some(target)
                                && app.sidebar.terminal_preview_revision == revision
                            {
                                app.show_terminal_preview(target, window, cx);
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            }
        } else if self.sidebar.hovered_terminal_preview == Some(target) {
            self.sidebar.hovered_terminal_preview = None;
            self.sidebar.terminal_preview_revision += 1;
        }
        self.terminal_preview_pointer(target, true, hovered, cx);
    }

    pub(super) fn terminal_preview_pointer(
        &mut self,
        target: TerminalPreviewAnchor,
        anchor: bool,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.sidebar.terminal_preview.as_mut() else {
            return;
        };
        if preview.anchor != target {
            return;
        }
        if anchor {
            preview.anchor_hovered = hovered;
        } else {
            preview.popup_hovered = hovered;
        }
        preview.leave_revision += 1;
        if hovered {
            preview
                .window
                .update(cx, |popup, _, cx| popup.restore(cx))
                .ok();
        } else if !preview.anchor_hovered && !preview.popup_hovered {
            let revision = preview.leave_revision;
            let content_revision = preview.content_revision;
            let executor = cx.background_executor().clone();
            cx.spawn(async move |app, cx| {
                executor.timer(TERMINAL_PREVIEW_LEAVE_DELAY).await;
                app.update(cx, |app, cx| {
                    if app
                        .sidebar
                        .terminal_preview
                        .as_ref()
                        .is_some_and(|preview| {
                            preview.is_current_target(target, content_revision)
                                && preview.leave_revision == revision
                                && !preview.anchor_hovered
                                && !preview.popup_hovered
                        })
                    {
                        app.dismiss_terminal_preview(cx);
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    pub(super) fn show_terminal_preview(
        &mut self,
        target: TerminalPreviewAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = target.session_id();
        let Some(session) = self
            .session_feed
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
        else {
            return;
        };
        let anchor = match target {
            TerminalPreviewAnchor::Sidebar(id) => {
                self.sidebar.sidebar_row_bounds.borrow().get(&id).copied()
            }
            TerminalPreviewAnchor::Tab(id, _) => self.tab_drop_bounds.borrow().get(&id).copied(),
        };
        let Some(anchor) = anchor else {
            return;
        };
        self.dismiss_project_popover(cx);
        let colors = self.colors;
        let language = self.language;
        let dark = self.use_dark;
        let icon = process_icons::card_stack(
            &[self.icon_for_item(ItemKind::Terminal, Some(session_id))],
            dark,
            SIDEBAR_CARD_SIZE,
        );
        let snapshot = Rc::new(RefCell::new(
            self.session_feed.snapshots.get(&session_id).cloned(),
        ));
        let snapshot_for_popup = snapshot.clone();
        let appearance = self.resolved_session_appearance(session_id, cx);
        let theme = appearance.theme;
        let font_family: SharedString = appearance.font_family.to_string().into();
        let metric_adjustments = self.terminal_metric_adjustments;
        let cell_size = preview_cell_size(
            window,
            font_family.clone(),
            appearance.font_size,
            metric_adjustments,
        );
        let layout = Rc::new(Cell::new(TerminalPreviewLayout::fit(
            snapshot.borrow().as_ref().map(|snapshot| snapshot.size),
            cell_size,
            preview_available_space(window, cx),
        )));
        let layout_for_popup = layout.clone();
        let font_features = self.terminal_font_features.clone();
        let ligatures = self.terminal_ligatures;
        let shaping_break_cursor = self.terminal_shaping_break_cursor;
        let font_variations = self.terminal_font_variations.clone();
        let font_thicken = self.terminal_font_thicken;
        let codepoint_map = self.terminal_font_codepoint_map.clone();
        let renderer = MetalTerminalRenderer::default();
        let inspection = Rc::new(RefCell::new(
            self.session_feed
                .session_inspections
                .get(&session_id)
                .cloned(),
        ));
        let inspection_for_popup = inspection.clone();
        let parent = Window::window_handle(window)
            .downcast::<Self>()
            .expect("terminal preview belongs to an Eggie window");
        let manager = self.window_manager.clone();
        let palette = self.interface_theme.palette;
        let memory_label = if language == Language::English {
            "Process memory"
        } else {
            "进程内存"
        };
        let content_revision = self.sidebar.terminal_preview_revision;
        let options = PopupWindowOptions {
            anchor,
            content_size: layout.get().content,
            placement: match target {
                TerminalPreviewAnchor::Sidebar(_) => PopupPlacement::Beside,
                TerminalPreviewAnchor::Tab(..) => PopupPlacement::Below,
            },
            background: colors.panel,
            border: colors.border,
        };
        let content = move |_: &mut Window, cx: &mut Context<PopupWindow>| {
            let layout = layout_for_popup.get();
            let registry = manager.read(cx);
            let session = registry
                .sessions
                .iter()
                .find(|current| current.id == session_id)
                .unwrap_or(&session);
            let title = registry
                .state
                .locate(session_id)
                .and_then(|location| registry.state.item(location))
                .and_then(|item| item.custom_title.as_deref())
                .or_else(|| {
                    session
                        .program_status
                        .as_ref()
                        .and_then(|status| status.title.as_deref())
                })
                .unwrap_or(&session.title)
                .to_owned();
            let status = session.program_status.clone();
            let osc_progress = registry
                .progress
                .get(&session_id)
                .and_then(|update| update.progress);
            let status_label = status
                .as_ref()
                .map(|status| program_state_label(status.state, language).to_owned())
                .or_else(|| {
                    osc_progress.map(|progress| {
                        let label = match (progress.state, language) {
                            (TerminalProgressState::Normal, Language::English) => "Progress",
                            (TerminalProgressState::Normal, Language::SimplifiedChinese) => "进度",
                            (TerminalProgressState::Error, Language::English) => "Error",
                            (TerminalProgressState::Error, Language::SimplifiedChinese) => "出错",
                            (TerminalProgressState::Indeterminate, Language::English) => "Working",
                            (TerminalProgressState::Indeterminate, Language::SimplifiedChinese) => {
                                "进行中"
                            }
                            (TerminalProgressState::Paused, Language::English) => "Paused",
                            (TerminalProgressState::Paused, Language::SimplifiedChinese) => {
                                "已暂停"
                            }
                        };
                        progress
                            .percent
                            .map_or_else(|| label.to_owned(), |value| format!("{label} {value}%"))
                    })
                })
                .unwrap_or_else(|| match (&session.status, language) {
                    (SessionStatus::Running, Language::English) => "Running".to_owned(),
                    (SessionStatus::Running, Language::SimplifiedChinese) => "运行中".to_owned(),
                    (SessionStatus::Exited { .. }, Language::English) => "Exited".to_owned(),
                    (SessionStatus::Exited { .. }, Language::SimplifiedChinese) => {
                        "已退出".to_owned()
                    }
                });
            let status_tint = status
                .as_ref()
                .map(|status| match status.state {
                    TerminalProgramState::Blocked => palette[3],
                    TerminalProgramState::Error => palette[1],
                    TerminalProgramState::Working => colors.accent,
                    TerminalProgramState::Done => palette[2],
                    TerminalProgramState::Idle => colors.muted,
                })
                .or_else(|| {
                    osc_progress.map(|progress| match progress.state {
                        TerminalProgressState::Error => palette[1],
                        TerminalProgressState::Paused => palette[3],
                        _ => colors.accent,
                    })
                });
            let mut status_detail = status
                .as_ref()
                .and_then(|status| status.message.clone())
                .or_else(|| session.active_command.clone())
                .unwrap_or_else(|| session.current_directory.display().to_string());
            if let Some(progress) = status.as_ref().and_then(|status| status.progress) {
                status_detail.push_str(&format!(" · {progress}%"));
            }
            let snapshot = snapshot_for_popup.borrow().clone();
            let memory = inspection_for_popup
                .borrow()
                .as_ref()
                .and_then(|inspection| {
                    inspection
                        .processes
                        .iter()
                        .filter_map(|process| process.memory_bytes)
                        .reduce(u64::saturating_add)
                });
            let thumbnail: AnyElement = if let Some(snapshot) = &snapshot {
                div()
                    .size_full()
                    .bg(rgba(terminal_background(snapshot, theme)))
                    .font_family(font_family.clone())
                    .text_size(px(appearance.font_size))
                    .child(
                        renderer.render(
                            snapshot.clone(),
                            TerminalRenderOptions::new(
                                theme,
                                colors.accent,
                                0.,
                                None,
                                None,
                                None,
                                InputLatencyTracker::default(),
                            )
                            .with_cursor_visible(false)
                            .with_fit_to_viewport()
                            .with_metric_adjustments(metric_adjustments)
                            .with_font_families(appearance.families.clone())
                            .with_font_features(
                                font_features.clone(),
                                ligatures,
                                shaping_break_cursor,
                            )
                            .with_font_variations(font_variations.clone(), font_thicken)
                            .with_codepoint_map(codepoint_map.clone()),
                        ),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .bg(rgb(theme.background))
                    .text_color(rgb(theme.foreground))
                    .text_size(px(11.))
                    .child(if language == Language::English {
                        "Waiting for terminal output"
                    } else {
                        "等待终端画面"
                    })
                    .into_any_element()
            };
            div()
                .flex()
                .flex_col()
                .size_full()
                .p(px(PREVIEW_PADDING))
                .gap(px(PREVIEW_GAP))
                .text_color(rgb(colors.text))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .h(px(PREVIEW_HEADER_HEIGHT))
                        .items_center()
                        .gap_2()
                        .when_some(icon.clone(), |row, icon| {
                            row.child(
                                img(icon)
                                    .flex_none()
                                    .w(px(process_icons::stack_width(1, SIDEBAR_CARD_SIZE)))
                                    .h(px(SIDEBAR_CARD_SIZE + 5.)),
                            )
                        })
                        .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                        .child(
                            div()
                                .flex_none()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .when_some(status_tint, |tag, tint| {
                                    tag.bg(rgba((tint << 8) | 0x28))
                                })
                                .text_size(px(10.))
                                .text_color(rgb(status_tint.unwrap_or(colors.muted)))
                                .child(status_label.clone()),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.))
                        .line_height(px(PREVIEW_LINE_HEIGHT))
                        .text_color(rgb(colors.muted))
                        .child(status_detail.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .self_center()
                        .w(layout.image.width + px(2. * PREVIEW_BORDER))
                        .h(layout.image.height + px(2. * PREVIEW_BORDER))
                        .overflow_hidden()
                        .rounded_md()
                        .mask_corners(rgb(colors.panel))
                        .border_1()
                        .border_color(rgb(colors.border))
                        .child(thumbnail),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .min_w_0()
                        .h(px(PREVIEW_LINE_HEIGHT))
                        .line_height(px(PREVIEW_LINE_HEIGHT))
                        .items_center()
                        .gap_2()
                        .justify_between()
                        .text_size(px(11.))
                        .text_color(rgb(colors.muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(session.current_process.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .child(format!("{memory_label} {}", format_memory_usage(memory))),
                        ),
                )
                .into_any_element()
        };
        let on_hover = move |hovered, _: &mut Window, cx: &mut App| {
            cx.defer(move |cx| {
                parent
                    .update(cx, |app, _, cx| {
                        if app
                            .sidebar
                            .terminal_preview
                            .as_ref()
                            .is_some_and(|preview| {
                                preview.is_current_target(target, content_revision)
                            })
                        {
                            app.terminal_preview_pointer(target, false, hovered, cx);
                        }
                    })
                    .ok();
            });
        };
        let popup = if let Some(preview) = &self.sidebar.terminal_preview {
            let handle = preview.window;
            handle
                .update(cx, |popup, popup_window, cx| {
                    popup.retarget(window, options, content, on_hover, popup_window, cx);
                })
                .map(|_| handle)
        } else {
            PopupWindow::open(cx, window, options, content, on_hover)
        };
        let Ok(popup) = popup else {
            self.sidebar.terminal_preview = None;
            return;
        };
        self.sidebar.terminal_preview = Some(TerminalPreviewState {
            anchor: target,
            session_id,
            content_revision,
            window: popup,
            anchor_hovered: true,
            popup_hovered: false,
            leave_revision: 0,
        });
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |app, cx| {
            let mut tick = 0;
            loop {
                let live = app
                    .update(cx, |app, _| {
                        app.sidebar
                            .terminal_preview
                            .as_ref()
                            .is_some_and(|preview| {
                                preview.window.window_id() == popup.window_id()
                                    && preview.is_current_target(target, content_revision)
                            })
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
                let client = client.clone();
                let (frame, metrics) = executor
                    .spawn(async move {
                        let frame = client.request(ClientRequest::Snapshot { session_id });
                        let metrics = (tick % 3 == 0)
                            .then(|| client.request(ClientRequest::InspectSession { session_id }));
                        (frame, metrics)
                    })
                    .await;
                let live = app
                    .update_in(cx, |app, parent_window, cx| {
                        if !app
                            .sidebar
                            .terminal_preview
                            .as_ref()
                            .is_some_and(|preview| {
                                preview.window.window_id() == popup.window_id()
                                    && preview.is_current_target(target, content_revision)
                            })
                        {
                            return false;
                        }
                        if let Ok(DaemonResponse::Snapshot { snapshot: frame }) = frame {
                            *snapshot.borrow_mut() = Some(frame);
                        }
                        if let Some(Ok(DaemonResponse::SessionInspection {
                            inspection: mut current,
                        })) = metrics
                        {
                            app.fill_missing_process_metrics(&mut current);
                            *inspection.borrow_mut() = Some(current.clone());
                            app.session_feed
                                .session_inspections
                                .insert(session_id, current);
                        }
                        let next_layout = TerminalPreviewLayout::fit(
                            snapshot.borrow().as_ref().map(|snapshot| snapshot.size),
                            cell_size,
                            preview_available_space(parent_window, cx),
                        );
                        layout.set(next_layout);
                        popup
                            .update(cx, |popup, window, cx| {
                                popup.resize_content(
                                    parent_window,
                                    next_layout.content,
                                    window,
                                    cx,
                                );
                                cx.notify();
                            })
                            .ok();
                        true
                    })
                    .unwrap_or(false);
                if !live {
                    break;
                }
                tick += 1;
                executor.timer(Duration::from_millis(300)).await;
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PreviewHost;
    impl gpui::Render for PreviewHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn preview_measures_and_opens_outside_the_drawing_phase(cx: &mut gpui::TestAppContext) {
        let parent = cx.add_window(|_, _| PreviewHost);
        // Hover timers run as window updates, not during element prepaint or paint.
        let popup = cx.update(|cx| {
            parent
                .update(cx, |_, window, cx| {
                    let cell = preview_cell_size(window, "Menlo".into(), 14., Default::default());
                    assert!(cell.width > px(0.) && cell.height > px(0.));
                    let layout = TerminalPreviewLayout::fit(
                        Some(grid(120, 40)),
                        cell,
                        preview_available_space(window, cx),
                    );
                    PopupWindow::open(
                        cx,
                        window,
                        PopupWindowOptions {
                            anchor: Bounds::new(point(px(10.), px(10.)), size(px(120.), px(24.))),
                            content_size: layout.content,
                            placement: PopupPlacement::Below,
                            background: 0xffffff,
                            border: 0xdddddd,
                        },
                        move |_, _| {
                            div()
                                .w(layout.image.width)
                                .h(layout.image.height)
                                .into_any_element()
                        },
                        |_, _, _| {},
                    )
                })
                .unwrap()
                .unwrap()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| cx.windows().len()), 2);
        cx.update(|cx| {
            popup
                .update(cx, |popup, window, cx| popup.dismiss(window, cx))
                .unwrap()
        });
    }

    fn grid(columns: u16, rows: u16) -> TerminalSize {
        TerminalSize {
            columns,
            rows,
            ..Default::default()
        }
    }

    #[test]
    fn preview_follows_grid_aspect_and_fits_available_space() {
        let cell = size(px(8.), px(16.));
        let available = size(px(1000.), px(700.));
        let wide = TerminalPreviewLayout::fit(Some(grid(240, 30)), cell, available);
        let tall = TerminalPreviewLayout::fit(Some(grid(60, 100)), cell, available);
        assert!(wide.content.height < tall.content.height);
        assert!(tall.content.width < wide.content.width);
        for (grid, available) in [
            (grid(240, 30), available),
            (grid(60, 100), available),
            (grid(240, 80), size(px(260.), px(210.))),
        ] {
            let layout = TerminalPreviewLayout::fit(Some(grid), cell, available);
            assert!(layout.content.width <= available.width);
            assert!(layout.content.height <= available.height);
            let expected_aspect = f32::from(grid.columns) * 8. / (f32::from(grid.rows) * 16.);
            let actual_aspect = f32::from(layout.image.width) / f32::from(layout.image.height);
            assert!((actual_aspect - expected_aspect).abs() < 0.0001);
            assert_eq!(
                layout.content.height,
                layout.image.height + px(PREVIEW_VERTICAL_CHROME)
            );
        }
    }

    #[test]
    fn missing_and_empty_grids_have_finite_layout_without_upscaling() {
        let available = size(px(1000.), px(700.));
        let cell = size(px(8.), px(16.));
        let empty = TerminalPreviewLayout::fit(Some(grid(0, 0)), cell, available);
        assert_eq!(empty.image, cell);
        let waiting = TerminalPreviewLayout::fit(None, cell, available);
        assert_eq!(waiting.image, size(px(320.), px(180.)));
    }
}

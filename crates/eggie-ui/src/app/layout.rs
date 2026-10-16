use super::*;

impl EggieApp {
    pub(super) fn toggle_left_sidebar(&mut self, cx: &mut Context<Self>) {
        self.dismiss_project_selector(cx);
        self.dismiss_project_popover(cx);
        self.left_sidebar_collapsed = !self.left_sidebar_collapsed;
        self.resizing_sidebar = None;
        cx.notify();
    }

    pub(super) fn toggle_right_sidebar(&mut self, cx: &mut Context<Self>) {
        self.right_sidebar_collapsed = !self.right_sidebar_collapsed;
        self.resizing_sidebar = None;
        cx.notify();
    }

    pub(super) fn start_sidebar_resize(
        &mut self,
        edge: SidebarEdge,
        _: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.resizing_sidebar = Some(edge);
        cx.stop_propagation();
    }

    pub(super) fn resize_sidebar(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edge) = self.resizing_sidebar else {
            return;
        };
        if !event.dragging() {
            self.resizing_sidebar = None;
            return;
        }

        let window_width = f32::from(window.bounds().size.width);
        let center_min_width = 320.;
        match edge {
            SidebarEdge::Left => {
                let right_width = if self.right_sidebar_collapsed {
                    0.
                } else {
                    self.right_sidebar_width
                };
                let maximum = LEFT_SIDEBAR_MAX_WIDTH
                    .min(window_width - right_width - center_min_width)
                    .max(LEFT_SIDEBAR_MIN_WIDTH);
                self.left_sidebar_width =
                    f32::from(event.position.x).clamp(LEFT_SIDEBAR_MIN_WIDTH, maximum);
            }
            SidebarEdge::Right => {
                let left_width = if self.left_sidebar_collapsed {
                    0.
                } else {
                    self.left_sidebar_width
                };
                let maximum = RIGHT_SIDEBAR_MAX_WIDTH
                    .min(window_width - left_width - center_min_width)
                    .max(RIGHT_SIDEBAR_MIN_WIDTH);
                self.right_sidebar_width = (window_width - f32::from(event.position.x))
                    .clamp(RIGHT_SIDEBAR_MIN_WIDTH, maximum);
            }
        }
        cx.notify();
    }

    pub(super) fn start_split_resize(&mut self, split_id: SplitId, cx: &mut Context<Self>) {
        self.resizing_split = Some(split_id);
        cx.stop_propagation();
    }

    pub(super) fn resize_split(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(split_id) = self.resizing_split else {
            return;
        };
        if !event.dragging() {
            self.resizing_split = None;
            return;
        }

        let Some(bounds) = self.split_bounds.borrow().get(&split_id).copied() else {
            return;
        };

        let layout = self.workspace_state.workspace().layout.clone();
        let Some(axis) = layout.split_axis(split_id) else {
            return;
        };

        let ratio = match axis {
            SplitAxis::Horizontal => {
                let width = f32::from(bounds.size.width);
                if width <= 0. {
                    return;
                }
                (f32::from(event.position.x - bounds.left()) / width).clamp(0.05, 0.95)
            }
            SplitAxis::Vertical => {
                let height = f32::from(bounds.size.height);
                if height <= 0. {
                    return;
                }
                (f32::from(event.position.y - bounds.top()) / height).clamp(0.05, 0.95)
            }
        };

        self.workspace_state
            .workspace_mut()
            .layout
            .set_split_ratio(split_id, ratio);
        cx.notify();
    }

    /// Send an absolute scroll request for the scrollbar thumb drag / track click.
    pub(super) fn scroll_terminal_to_offset(&self, session_id: SessionId, offset: u32) {
        if let Err(error) = self
            .client
            .request(ClientRequest::TerminalScrollToOffset { session_id, offset })
        {
            eprintln!("failed to scroll terminal to offset: {error:#}");
        }
    }

    /// Drive an in-progress scrollbar thumb drag. Mirrors `resize_split`: it reads the cached track
    /// bounds, maps the pointer Y (minus the grab offset) to an absolute scroll offset, renders the
    /// thumb optimistically at that offset, and asks the daemon to scroll there.
    pub(super) fn drag_scrollbar(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut drag) = self.dragging_scrollbar else {
            return;
        };
        if !event.dragging() {
            self.dragging_scrollbar = None;
            return;
        }
        let Some(bounds) = self.scrollbar_bounds.borrow().get(&drag.group_id).copied() else {
            return;
        };
        let Some(snapshot) = self.session_feed.snapshots.get(&drag.session_id) else {
            return;
        };
        let history = snapshot.history_size;
        let rows = snapshot.size.rows;
        let track_height = f32::from(bounds.size.height);
        let thumb_top = f32::from(event.position.y) - f32::from(bounds.top()) - drag.grab_offset;
        let offset = scrollbar_offset_from_thumb_top(track_height, history, rows, thumb_top);
        drag.pending_offset = offset;
        self.dragging_scrollbar = Some(drag);
        self.scroll_terminal_to_offset(drag.session_id, offset);
        cx.notify();
    }

    /// Handle a click on the scrollbar track (outside the thumb): page up if the click is above the
    /// thumb, page down if below. Reuses the discrete PageUp/PageDown scroll commands.
    pub(super) fn scrollbar_track_click(
        &mut self,
        group_id: GroupId,
        session_id: SessionId,
        position: Point<Pixels>,
    ) {
        let Some(bounds) = self.scrollbar_bounds.borrow().get(&group_id).copied() else {
            return;
        };
        let Some(snapshot) = self.session_feed.snapshots.get(&session_id) else {
            return;
        };
        let track_height = f32::from(bounds.size.height);
        let Some(thumb) = scrollbar_thumb_geometry(
            track_height,
            snapshot.history_size,
            snapshot.size.rows,
            snapshot.display_offset,
        ) else {
            return;
        };
        let click_y = f32::from(position.y) - f32::from(bounds.top());
        let command = if click_y < thumb.top {
            TerminalScrollCommand::PageUp
        } else if click_y > thumb.top + thumb.height {
            TerminalScrollCommand::PageDown
        } else {
            return;
        };
        if let Err(error) = self.client.request(ClientRequest::TerminalScrollTo {
            session_id,
            command,
        }) {
            eprintln!("failed to page terminal scrollbar: {error:#}");
        }
    }

    /// Render the terminal scrollbar overlay for a pane. Returns `None` when there is no scrollback
    /// (the scrollbar is hidden) or on the first frame before the track's bounds have been captured.
    ///
    /// The whole strip is `.occlude()`d so pointer events over it never fall through to the terminal
    /// (selection / link-hover) beneath — matching the search-bar overlay precedent. The thumb top
    /// is rendered from the previous frame's cached track height; while this group's thumb is being
    /// dragged we substitute the optimistic `pending_offset` so it tracks the pointer immediately.
    pub(super) fn render_scrollbar(
        &self,
        group_id: GroupId,
        session_id: SessionId,
        history: u32,
        offset: u32,
        rows: u16,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if history == 0 {
            return None;
        }
        // Optimistic offset during an active drag on this group.
        let render_offset = match self.dragging_scrollbar {
            Some(drag) if drag.group_id == group_id => drag.pending_offset,
            _ => offset,
        };
        let track_height = self
            .scrollbar_bounds
            .borrow()
            .get(&group_id)
            .map(|bounds| f32::from(bounds.size.height));
        let thumb = track_height
            .and_then(|height| scrollbar_thumb_geometry(height, history, rows, render_offset));

        let scrollbar_bounds = self.scrollbar_bounds.clone();
        let dragging_this = self
            .dragging_scrollbar
            .is_some_and(|drag| drag.group_id == group_id);
        let muted = self.colors.muted;
        // Same hue at every state, only the opacity changes: light at rest, a touch darker on hover,
        // darkest while dragging. Never the accent color.
        const THUMB_ALPHA_IDLE: u32 = 0x59;
        const THUMB_ALPHA_HOVER: u32 = 0x99;
        const THUMB_ALPHA_DRAG: u32 = 0xcc;

        let mut track = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(TERMINAL_SCROLLBAR_WIDTH))
            .occlude()
            // Capture the track's window-space bounds for thumb geometry and drag hit-mapping.
            .child(
                canvas(
                    move |bounds, _, _| {
                        scrollbar_bounds.borrow_mut().insert(group_id, bounds);
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .size_full(),
            )
            // A click on the track (outside the thumb) pages toward the click.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event: &MouseDownEvent, _, cx| {
                    app.scrollbar_track_click(group_id, session_id, event.position);
                    cx.stop_propagation();
                }),
            );

        if let Some(thumb) = thumb {
            let idle_alpha = if dragging_this {
                THUMB_ALPHA_DRAG
            } else {
                THUMB_ALPHA_IDLE
            };
            // A stable id lets GPUI track the hover state so the darker hover fill repaints promptly.
            let thumb_margin = (TERMINAL_SCROLLBAR_WIDTH - TERMINAL_SCROLLBAR_THUMB_WIDTH) / 2.;
            track = track.child(
                div()
                    .id(SharedString::from(format!("scrollbar-thumb-{group_id}")))
                    .absolute()
                    .top(px(thumb.top))
                    .right(px(thumb_margin))
                    .w(px(TERMINAL_SCROLLBAR_THUMB_WIDTH))
                    .h(px(thumb.height))
                    .rounded_full()
                    .bg(rgba((muted << 8) | idle_alpha))
                    .when(!dragging_this, |element| {
                        element.hover(|style| style.bg(rgba((muted << 8) | THUMB_ALPHA_HOVER)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |app, event: &MouseDownEvent, _, cx| {
                            let thumb_top_window = app
                                .scrollbar_bounds
                                .borrow()
                                .get(&group_id)
                                .map(|bounds| f32::from(bounds.top()) + thumb.top);
                            if let Some(thumb_top_window) = thumb_top_window {
                                app.dragging_scrollbar = Some(ScrollbarDrag {
                                    group_id,
                                    session_id,
                                    grab_offset: f32::from(event.position.y) - thumb_top_window,
                                    pending_offset: render_offset,
                                });
                            }
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        Some(track.into_any_element())
    }

    /// Attach the manual window-drag handlers Eggie uses under `app_owns_titlebar_drag`.
    ///
    /// On macOS the `WindowControlArea::Drag` marker is a no-op; dragging is driven entirely by
    /// these handlers (press arms `moving_window`, the first move starts the OS drag, and a
    /// double click zooms). The marker is kept for Windows portability. Apply this only to blank
    /// sibling elements without interactive children, so tab clicks and drag-reordering stay
    /// unaffected by the drag hitbox.
    pub(super) fn render_layout(&self, node: &LayoutNode, cx: &mut Context<Self>) -> AnyElement {
        self.render_layout_with_outer_edges(node, true, true, true, true, cx)
    }

    pub(super) fn render_layout_with_outer_edges(
        &self,
        node: &LayoutNode,
        touches_left_edge: bool,
        touches_right_edge: bool,
        touches_bottom_edge: bool,
        touches_top_edge: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            LayoutNode::Group { group } => self.render_group(
                group,
                touches_left_edge,
                touches_right_edge,
                touches_bottom_edge,
                touches_top_edge,
                cx,
            ),
            LayoutNode::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                let horizontal = *axis == SplitAxis::Horizontal;
                let ratio = ratio.clamp(0.05, 0.95);
                // Identify the split by its own stable id, not the first group inside it: nested
                // splits share the same "first group" and would otherwise collide, so dragging an
                // inner divider would resize the outer split instead.
                let split_id = *id;
                let split_bounds = self.split_bounds.clone();
                div()
                    .flex()
                    .when(!horizontal, |element| element.flex_col())
                    .size_full()
                    .min_w_0()
                    .on_children_prepainted(move |children, _, _| {
                        if children.len() < 2 {
                            return;
                        }
                        // Use the first and last children (the two panes) to compute the split
                        // container's bounds. The middle child is the drag handle, whose negative
                        // margins would otherwise skew the union.
                        let bounds = children[0].union(&children[children.len() - 1]);
                        split_bounds.borrow_mut().insert(split_id, bounds);
                    })
                    .child(
                        div()
                            .flex_grow(ratio)
                            .flex_shrink_0()
                            // Force the flex basis to zero so `flex_grow` distributes the *entire*
                            // container along the split axis in proportion to `ratio`. Without this
                            // the panes keep their large intrinsic (content) size as the basis and
                            // `ratio` only splits the leftover space, so dragging barely moves them.
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_layout_with_outer_edges(
                                first,
                                touches_left_edge,
                                touches_right_edge && !horizontal,
                                touches_bottom_edge && horizontal,
                                touches_top_edge,
                                cx,
                            )),
                    )
                    .child(self.render_split_divider(*axis, split_id, cx))
                    .child(
                        div()
                            .flex_grow(1.0 - ratio)
                            .flex_shrink_0()
                            .flex_basis(relative(0.))
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_layout_with_outer_edges(
                                second,
                                touches_left_edge && !horizontal,
                                touches_right_edge,
                                touches_bottom_edge,
                                touches_top_edge && horizontal,
                                cx,
                            )),
                    )
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_group(
        &self,
        group: &TabGroup,
        touches_left_edge: bool,
        touches_right_edge: bool,
        touches_bottom_edge: bool,
        touches_top_edge: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_id = group.id;
        let is_top_left_group =
            self.workspace_state.workspace().layout.top_left_group_id() == group_id;
        let is_top_right_group =
            self.workspace_state.workspace().layout.top_right_group_id() == group_id;
        let tab_insertion_index = self.tab_drop_target.and_then(|target| {
            if target.group_id != group_id {
                return None;
            }
            match target.zone {
                TabDropZone::TabBar { insertion_index } => Some(insertion_index),
                TabDropZone::Center | TabDropZone::Edge(_) => None,
            }
        });
        let mut tabs = div()
            .flex()
            .items_center()
            .h(px(TAB_BAR_HEIGHT))
            .p(px(TAB_BAR_PADDING))
            .bg(rgb(self.colors.panel));
        if self.left_sidebar_collapsed && is_top_left_group {
            tabs = tabs
                .child(self.render_traffic_light_tab_bar_region(group_id, cx))
                .child(top_bar_icon_button(
                    IconName::PanelLeftClose,
                    format!("expand-left-sidebar-{group_id}"),
                    self.colors,
                    false,
                    false,
                    Some(self.language.expand_sidebar_tooltip().into()),
                    cx.listener(|app, _, _, cx| app.toggle_left_sidebar(cx)),
                ));
        }
        // Tabs and the trailing drag/drop strip live in a horizontally scrollable strip so tabs
        // keep their minimum width and overflow into a scroll instead of being squeezed. The
        // `+` and collapse controls stay outside this strip so they never scroll away.
        let scroll_handle = self
            .tab_bar_scroll_handles
            .borrow_mut()
            .entry(group_id)
            .or_insert_with(ScrollHandle::new)
            .clone();
        let mut tabs_scroll = div()
            .id(format!("tabbar-scroll-{group_id}"))
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .track_scroll(&scroll_handle);
        for (tab_index, item) in group.items.iter().enumerate() {
            let item_id = item.id;
            let active = group.active_item_id == Some(item.id);
            let title_key = format!("tab-title-{item_id}");
            let label_background = if active {
                self.colors.panel_alt
            } else {
                self.colors.panel
            };
            // The first tab's "before" indicator sits at a negative offset that the scroll strip's
            // overflow would clip, so it is drawn once on the non-clipping scroll wrapper instead
            // (see below). Every other tab draws its own indicator in the preceding gap.
            let show_insertion_before = tab_insertion_index == Some(tab_index) && tab_index != 0;
            let show_insertion_after = tab_insertion_index == Some(group.items.len())
                && tab_index + 1 == group.items.len();
            let dragged_tab = DraggedTab {
                source_window_id: self.window_id,
                project_id: self.workspace_state.active_project,
                source_group_id: group_id,
                item_id,
                icon: self.icon_for_item(item.kind, item.session_id),
                dark: self.use_dark,
                title: item.display_title().to_owned().into(),
                active,
                colors: self.colors,
            };
            let drag_owner = cx.weak_entity();
            let tab_bounds = self.tab_drop_bounds.clone();
            tabs_scroll = tabs_scroll.child(
                div()
                    .flex_none()
                    .h_full()
                    .on_children_prepainted(move |children, _, _| {
                        if let Some(bounds) = children.first() {
                            tab_bounds.borrow_mut().insert(item_id, *bounds);
                        }
                    })
                    .child(
                        div()
                            .id(format!("tab-{item_id}"))
                            .flex()
                            .items_center()
                            .relative()
                            .flex_shrink_0()
                            .min_w(px(TAB_MIN_WIDTH))
                            .max_w(px(TAB_MAX_WIDTH))
                            .h_full()
                            .pl(px(TAB_ICON_LEADING_PADDING))
                            .pr_1()
                            .gap_2()
                            .text_size(px(TAB_TITLE_FONT_SIZE))
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(self.colors.border))
                            .when(tab_index + 1 < group.items.len(), |element| {
                                element.mr(px(TAB_GAP))
                            })
                            .when(active, |element| element.bg(rgb(self.colors.panel_alt)))
                            .when_some(
                                item.session_id.and_then(|sid| self.bell_flash_tint(sid)),
                                |element, tint| element.bg(rgba(tint)),
                            )
                            .cursor_pointer()
                            .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                                app.set_marquee_hover(
                                    &[format!("tab-title-{item_id}")],
                                    *hovered,
                                    cx,
                                );
                            }))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                app.activate_item(group_id, item_id, window, cx)
                            }))
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |app, _, window, cx| {
                                    app.show_native_tab_context_menu(group_id, item_id, window, cx);
                                }),
                            )
                            .on_drag(dragged_tab, move |tab, _, _, cx| {
                                let _ = drag_owner.update(cx, |app, cx| {
                                    app.publish_window_state(cx);
                                    app.dragged_tab = Some(tab.clone());
                                });
                                cx.new(|_| DraggedTabPreview { tab: tab.clone() })
                            })
                            .on_drag_move::<DraggedTab>(cx.listener(
                                move |app, event, window, cx| {
                                    cx.set_active_drag_cursor_style(
                                        gpui::CursorStyle::ClosedHand,
                                        window,
                                    );
                                    app.update_tab_bar_drop_target(group_id, tab_index, event, cx)
                                },
                            ))
                            .on_drop(cx.listener(|app, dragged_tab: &DraggedTab, _, cx| {
                                app.handle_tab_drop(dragged_tab, cx)
                            }))
                            .child(self.render_item_icon(
                                item.kind,
                                item.session_id,
                                format!("tab-progress-{item_id}"),
                            ))
                            .when_some(
                                self.tab_profile_badge(item, cx),
                                |element, (color, name)| {
                                    element.child(
                                        div()
                                            // `tooltip` requires a stateful (identified) element.
                                            .id(format!("tab-badge-{item_id}"))
                                            .flex_none()
                                            .size(px(TAB_BADGE_SIZE))
                                            .rounded_full()
                                            .bg(rgb(color))
                                            // The dot alone says "this tab is special" but not which profile;
                                            // the tooltip names it, which is the whole point of the marker.
                                            .tooltip(tooltip_text(name, self.colors)),
                                    )
                                },
                            )
                            .when_some(item.session_id.and_then(|id| self.session_feed.global_sessions.iter().find(|session| session.id == id)).and_then(|session| session.remote.as_ref()), |element, remote| {
                                element.child(div().id(format!("ssh-host-{item_id}"))
                                    .text_size(px(9.)).text_color(rgb(self.colors.muted))
                                    .tooltip(tooltip_text(remote.target.consent_key(), self.colors)).child("SSH"))
                            })
                            .child(self.render_marquee(
                                title_key,
                                item.display_title().to_owned(),
                                TAB_TITLE_FONT_SIZE,
                                16.,
                                label_background,
                                cx,
                            ))
                            .child(
                                div()
                                    .id(format!("close-tab-{item_id}"))
                                    .flex_none()
                                    .text_color(rgb(self.colors.muted))
                                    .cursor_pointer()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        cx.stop_propagation();
                                        app.close_item(group_id, item_id, cx)
                                    }))
                                    .child(icon_sized(IconName::Close, TAB_CLOSE_ICON_SIZE)),
                            )
                            .when(show_insertion_before, |element| {
                                // Centered in the gap before this tab. (The first tab is handled on the
                                // scroll wrapper to avoid overflow clipping — see `show_insertion_before`.)
                                element.child(
                                    div()
                                        .absolute()
                                        .left(px(-TAB_DROP_INDICATOR_OFFSET))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(TAB_DROP_INDICATOR_WIDTH))
                                        .bg(rgb(self.colors.accent)),
                                )
                            })
                            .when(show_insertion_after, |element| {
                                element.child(
                                    div()
                                        .absolute()
                                        .right(px(-TAB_DROP_INDICATOR_OFFSET))
                                        .top_0()
                                        .bottom_0()
                                        .w(px(TAB_DROP_INDICATOR_WIDTH))
                                        .bg(rgb(self.colors.accent)),
                                )
                            }),
                    ),
            );
        }
        let item_count = group.items.len();
        let show_empty_group_insertion = item_count == 0 && tab_insertion_index == Some(0);
        let mut trailing_space = div()
            .id(format!("tabbar-trailing-space-{group_id}"))
            .flex_1()
            .relative()
            .h_full()
            .on_drag_move::<DraggedTab>(cx.listener(move |app, event, window, cx| {
                cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                app.update_tab_bar_trailing_drop_target(group_id, item_count, event, cx)
            }))
            .on_drop(cx.listener(|app, dragged_tab: &DraggedTab, _, cx| {
                app.handle_tab_drop(dragged_tab, cx)
            }))
            .when(show_empty_group_insertion, |element| {
                element.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(TAB_DROP_INDICATOR_WIDTH))
                        .bg(rgb(self.colors.accent)),
                )
            });
        // Only a tab bar flush against the top window edge doubles as a drag region; a lower
        // group's tab bar sits mid-window and must not move the window.
        if touches_top_edge {
            trailing_space = Self::with_window_drag_handlers(trailing_space, cx);
        }
        // Fade masks on the left/right edges of the scroll strip indicate that more tabs are
        // available off-screen. They are plain visual overlays with no hitbox, so wheel/trackpad
        // scrolling and tab clicks pass straight through to the scroll container beneath.
        let offset_x = -f32::from(scroll_handle.offset().x);
        let max_offset_x = f32::from(scroll_handle.max_offset().x);
        let show_left_mask = offset_x > 0.5;
        let show_right_mask = offset_x < max_offset_x - 0.5;
        let panel_color = rgb(self.colors.panel);
        let mask_width = px(24.);
        let mut scroll_wrapper = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(tabs_scroll.child(trailing_space));
        if show_left_mask {
            scroll_wrapper = scroll_wrapper.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(mask_width)
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    )),
            );
        }
        if show_right_mask {
            scroll_wrapper = scroll_wrapper.child(
                div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(mask_width)
                    .bg(linear_gradient(
                        270.,
                        linear_color_stop(panel_color, 0.),
                        linear_color_stop(panel_color.opacity(0.), 1.),
                    )),
            );
        }
        // The indicator for inserting before the FIRST tab lives on the (non-clipping) scroll
        // wrapper rather than inside the scroll strip: at the strip's x=0 origin its negative
        // offset would fall outside the overflow viewport and be clipped away. Drawn here it lands
        // at the very same spot — in the tab bar's left padding — without being cut off.
        if item_count > 0 && tab_insertion_index == Some(0) {
            scroll_wrapper = scroll_wrapper.child(
                div()
                    .absolute()
                    .left(px(-TAB_DROP_INDICATOR_OFFSET))
                    .top_0()
                    .bottom_0()
                    .w(px(TAB_DROP_INDICATOR_WIDTH))
                    .bg(rgb(self.colors.accent)),
            );
        }
        // A fixed gap between the scrollable tab strip and the new-tab button gives the user a
        // reliable grab handle to move the window, even when the tab bar is packed full.
        let mut tab_bar_drag_gap = div()
            .id(format!("tabbar-drag-gap-{group_id}"))
            .flex_none()
            .w(px(40.))
            .h_full();
        if touches_top_edge {
            tab_bar_drag_gap = Self::with_window_drag_handlers(tab_bar_drag_gap, cx);
        }
        tabs = tabs.child(scroll_wrapper).child(tab_bar_drag_gap).child(
            // Right click on the "+" picks the profile the new tab runs, instead of the project
            // default. The handler sits on this wrapper because `top_bar_icon_button` returns an
            // opaque element that cannot take another method.
            div()
                .flex()
                .flex_none()
                .h_full()
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |app, _, window, cx| {
                        app.show_profile_picker_menu(group_id, window, cx);
                    }),
                )
                .child(top_bar_icon_button(
                    IconName::Add,
                    format!("new-terminal-{group_id}"),
                    self.colors,
                    false,
                    false,
                    Some(self.language.new_terminal_tooltip().into()),
                    cx.listener(move |app, _, window, cx| {
                        app.new_terminal(group_id, None, None, window, cx)
                    }),
                )),
        );
        if is_top_right_group {
            let right_sidebar_icon = if self.right_sidebar_collapsed {
                IconName::PanelRightOpen
            } else {
                IconName::PanelRightClose
            };
            tabs = tabs.child(top_bar_icon_button(
                right_sidebar_icon,
                format!("toggle-right-sidebar-{group_id}"),
                self.colors,
                false,
                false,
                Some(self.language.toggle_right_sidebar_tooltip().into()),
                cx.listener(|app, _, _, cx| app.toggle_right_sidebar(cx)),
            ));
        }

        let content_drop_zone = self.tab_drop_target.and_then(|target| {
            if target.group_id != group_id {
                return None;
            }
            match target.zone {
                TabDropZone::Center => Some(TabDropZone::Center),
                TabDropZone::Edge(direction) => Some(TabDropZone::Edge(direction)),
                TabDropZone::TabBar { .. } => None,
            }
        });
        let terminal_captures_mouse = group
            .active_item()
            .and_then(|item| item.session_id)
            .and_then(|session_id| self.session_feed.snapshots.get(&session_id))
            .is_some_and(|snapshot| snapshot.input_modes.captures_mouse());
        // Whether an auto-detected URL is hovered in this group's active session (pointer cursor).
        let link_hovered = group
            .active_item()
            .and_then(|item| item.session_id)
            .zip(self.terminal_hovered_link.as_ref())
            .is_some_and(|(session_id, (hovered_session, _))| session_id == *hovered_session);
        let content = group
            .active_item()
            .and_then(|item| item.session_id)
            .and_then(|session_id| self.session_feed.snapshots.get(&session_id).cloned())
            .map(|snapshot| {
                let session_id = snapshot.session_id;
                let snapshot_size = snapshot.size;
                // The daemon projects its authoritative selection into the viewport and ships the
                // range inside the snapshot, so the highlight always matches this frame's cells.
                let selection = snapshot.selection.map(|range| TerminalSelection {
                    anchor: TerminalPoint {
                        line: range.start.line,
                        column: range.start.column,
                    },
                    head: TerminalPoint {
                        line: range.end.line,
                        column: range.end.column,
                    },
                });
                let ime = self.terminal_ime_states.get(&session_id).cloned();
                let search = self
                    .terminal_search_matches(session_id)
                    .map(TerminalSearchHighlights::from_result);
                // Underline the hovered auto-detected URL, but only for the session it belongs to.
                let url_hover = self
                    .terminal_hovered_link
                    .as_ref()
                    .filter(|(hovered_session, _)| *hovered_session == session_id)
                    .map(|(_, link)| link.clone());
                let app = cx.entity().downgrade();
                let input = (self.workspace_state.workspace().active_group_id == group_id)
                    .then(|| TerminalInputContext::new(app.clone(), self.focus_handle.clone()));
                // Resolve the blink phase before `snapshot` is moved into the renderer.
                let cursor_visible = self.cursor_visible_this_frame(&snapshot);
                let metric_adjustments = self.terminal_metric_adjustments;
                // This terminal's own appearance: its profile's overrides merged over the global
                // layer. Recomputed per frame so an edit to either layer moves it immediately.
                let appearance = self.resolved_session_appearance(session_id, cx);
                div()
                    .size_full()
                    .overflow_hidden()
                    .bg(rgba(terminal_background(&snapshot, appearance.theme)))
                    .px(px(self.terminal_padding_x))
                    .py(px(self.terminal_padding_y))
                    .font_family(SharedString::from(appearance.font_family.to_string()))
                    .text_size(px(appearance.font_size))
                    .on_children_prepainted(move |children, window, cx| {
                        let Some(bounds) = children.first().copied() else {
                            return;
                        };
                        let (cell_width, line_height) =
                            terminal_cell_metrics(window, metric_adjustments);
                        let size = terminal_size_for_viewport(
                            f32::from(bounds.size.width),
                            f32::from(bounds.size.height),
                            f32::from(cell_width),
                            f32::from(line_height),
                            window.scale_factor(),
                        );
                        let app = app.clone();
                        cx.defer(move |cx| {
                            app.update(cx, |app, cx| {
                                app.record_terminal_viewport(
                                    group_id,
                                    session_id,
                                    bounds,
                                    f32::from(cell_width),
                                    f32::from(line_height),
                                    snapshot_size,
                                );
                                app.resize_terminal(session_id, size, cx);
                            })
                            .ok();
                        });
                    })
                    .child(
                        self.terminal_renderer.render(
                            snapshot,
                            TerminalRenderOptions::new(
                                appearance.theme,
                                // The accent tints the selection band and the URL-hover underline, both of
                                // which are painted *inside* terminal cells — so it has to come from this
                                // terminal's own resolved theme, not from `self.colors` (which is derived
                                // from the interface theme). Reading the chrome accent here would paint a
                                // light-interface selection over a dark terminal once the two layers differ.
                                UiColors::from_theme(appearance.theme).accent,
                                self.terminal_minimum_contrast,
                                selection,
                                ime,
                                input,
                                self.input_latency.clone(),
                            )
                            .with_search(search)
                            .with_url_hover(url_hover)
                            .with_cursor_visible(cursor_visible)
                            .with_metric_adjustments(metric_adjustments)
                            .with_font_families(appearance.families.clone())
                            .with_font_features(
                                self.terminal_font_features.clone(),
                                self.terminal_ligatures,
                                self.terminal_shaping_break_cursor,
                            )
                            .with_font_variations(
                                self.terminal_font_variations.clone(),
                                self.terminal_font_thicken,
                            )
                            .with_codepoint_map(self.terminal_font_codepoint_map.clone()),
                        ),
                    )
                    .into_any_element()
            })
            .unwrap_or_else(|| {
                let ended = group
                    .active_item()
                    .and_then(|item| item.session_id)
                    .is_some_and(|id| {
                        !self
                            .session_feed
                            .global_sessions
                            .iter()
                            .any(|session| session.id == id)
                    });
                let profile = self
                    .profile_store
                    .read(cx)
                    .resolve(group.active_item().and_then(|item| item.profile_id))
                    .clone();
                let status = self.poll_error.clone().or_else(|| {
                    group.active_item().map(|_| {
                        match (ended, self.language) {
                            (true, Language::SimplifiedChinese) => "进程已结束",
                            (true, Language::English) => "Process ended",
                            (false, Language::SimplifiedChinese) => "正在连接终端…",
                            (false, Language::English) => "Connecting to terminal…",
                        }
                        .to_owned()
                    })
                });
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(self.colors.muted))
                    .children(status)
                    .when(ended || group.active_item().is_none(), |element| {
                        element.child(
                            div()
                                .id(format!("new-terminal-empty-{group_id}"))
                                .px_3()
                                .py_2()
                                .rounded_lg()
                                .bg(rgb(self.colors.panel_alt))
                                .cursor_pointer()
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    app.new_terminal_with_profile(
                                        group_id,
                                        None,
                                        None,
                                        profile.clone(),
                                        window,
                                        cx,
                                    )
                                }))
                                .child(self.language.new_terminal_tooltip()),
                        )
                    })
                    .into_any_element()
            });

        let corners = terminal_container_corners(
            touches_left_edge,
            touches_right_edge,
            touches_bottom_edge,
            self.left_sidebar_collapsed,
            self.right_sidebar_collapsed,
        );
        let borders = terminal_container_borders(
            touches_left_edge,
            touches_right_edge,
            touches_bottom_edge,
            self.left_sidebar_collapsed,
            self.right_sidebar_collapsed,
        );
        // The tab bar sits above the content region, so a vertical split seam drawn only on the
        // content region leaves a gap at tab-bar height. Extend the same left/right seam border up
        // through the tab bar. Skip the side that is rounded (an outer layout edge), where the
        // content region's rounded corner owns the look and a straight tab-bar border would spoil it.
        tabs = tabs
            .border_color(rgb(self.colors.border))
            .when(borders.left && !corners.top_left, |element| {
                element.border_l_1()
            })
            .when(borders.right && !corners.top_right, |element| {
                element.border_r_1()
            });
        let mut content_region = div()
            // Custom terminal output is covered at the same curve as the pane's border.
            .mask_corners(rgb(self.colors.background))
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .shadow_xs()
            .border_t_1()
            .border_color(rgb(self.colors.border))
            .when(borders.bottom, |element| element.border_b_1())
            .when(corners.top_left, |element| element.rounded_tl_md())
            .when(corners.top_right, |element| element.rounded_tr_md())
            .when(corners.bottom_left, |element| element.rounded_bl_md())
            .when(corners.bottom_right, |element| element.rounded_br_md())
            .when(borders.left, |element| element.border_l_1())
            .when(borders.right, |element| element.border_r_1())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |app, event, window, cx| {
                    app.terminal_mouse_down(group_id, event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |app, event, window, cx| {
                    app.terminal_mouse_down(group_id, event, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |app, event, window, cx| {
                    app.terminal_mouse_down(group_id, event, window, cx)
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |app, event, _, cx| app.terminal_mouse_up(group_id, event, cx)),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |app, event, _, cx| app.terminal_mouse_up(group_id, event, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |app, event, _, cx| app.terminal_mouse_up(group_id, event, cx)),
            )
            .on_mouse_move(
                cx.listener(move |app, event, _, cx| app.terminal_mouse_move(group_id, event, cx)),
            )
            .on_scroll_wheel(
                cx.listener(move |app, event, _, cx| app.terminal_scroll(group_id, event, cx)),
            )
            .when(link_hovered, |element| {
                element.cursor(gpui::CursorStyle::PointingHand)
            })
            .when(terminal_captures_mouse && !link_hovered, |element| {
                element.cursor_default()
            })
            .when(!terminal_captures_mouse && !link_hovered, |element| {
                element.cursor_text()
            })
            .child(content);
        if let Some(zone) = content_drop_zone {
            let overlay = div()
                .absolute()
                .border_2()
                .border_color(rgb(self.colors.accent))
                .bg(rgba((self.colors.accent << 8) | 0x33))
                .map(|overlay| match zone {
                    TabDropZone::Center => overlay.inset_0(),
                    TabDropZone::Edge(Direction::Up) => {
                        overlay.top_0().left_0().right_0().h(relative(0.5))
                    }
                    TabDropZone::Edge(Direction::Down) => {
                        overlay.bottom_0().left_0().right_0().h(relative(0.5))
                    }
                    TabDropZone::Edge(Direction::Left) => {
                        overlay.top_0().bottom_0().left_0().w(relative(0.5))
                    }
                    TabDropZone::Edge(Direction::Right) => {
                        overlay.top_0().bottom_0().right_0().w(relative(0.5))
                    }
                    TabDropZone::TabBar { .. } => unreachable!(),
                })
                .on_drop(cx.listener(|app, dragged_tab: &DraggedTab, _, cx| {
                    app.handle_tab_drop(dragged_tab, cx)
                }));
            content_region = content_region.child(overlay);
        }

        if let Some(paste) = group
            .active_item()
            .and_then(|item| item.session_id)
            .and_then(|session| self.render_paste_progress(session, cx))
        {
            content_region = content_region.child(paste);
        }

        if let Some(status) = self.render_ssh_status(group, cx) {
            content_region = content_region.child(status);
        }

        // Overlay the search bar when it is bound to this group's active session.
        let active_session_id = group.active_item().and_then(|item| item.session_id);
        if let Some(search_bar) = active_session_id
            .filter(|session_id| {
                self.terminal_search
                    .as_ref()
                    .is_some_and(|search| search.session_id == *session_id)
            })
            .and_then(|_| self.render_terminal_search_bar(cx))
        {
            content_region = content_region.child(search_bar);
        }

        // Overlay the scrollbar on the right edge when the active session has scrollback.
        if let Some(scrollbar) = active_session_id.and_then(|session_id| {
            let snapshot = self.session_feed.snapshots.get(&session_id)?;
            self.render_scrollbar(
                group_id,
                session_id,
                snapshot.history_size,
                snapshot.display_offset,
                snapshot.size.rows,
                cx,
            )
        }) {
            content_region = content_region.child(scrollbar);
        }

        let group_bounds = self.group_drop_bounds.clone();
        div()
            .on_children_prepainted(move |children, _, _| {
                if let (Some(top), Some(content)) = (children.first(), children.get(1)) {
                    group_bounds.borrow_mut().insert(
                        group_id,
                        Bounds::new(
                            top.origin,
                            size(top.size.width, content.bottom() - top.top()),
                        ),
                    );
                }
            })
            .id(format!("terminal-group-{group_id}"))
            .flex()
            .flex_col()
            .relative()
            .size_full()
            .min_w_0()
            .bg(rgb(self.colors.background))
            .on_drag_move::<DraggedTab>(cx.listener(move |app, event, window, cx| {
                cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
                app.update_content_drop_target(group_id, event, cx)
            }))
            .child(tabs)
            .child(content_region)
            .into_any_element()
    }
}

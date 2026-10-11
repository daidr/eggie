use super::*;

impl EggieApp {
    pub(super) fn show_profile_picker_menu(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let default_profile_id = self
            .workspace_state
            .active_project_data()
            .default_profile_id;
        let profiles: Vec<(eggie_domain::ProfileId, String, bool)> = self
            .profile_store
            .read(cx)
            .profiles()
            .iter()
            .map(|profile| {
                (
                    profile.id,
                    profile.name.clone(),
                    Some(profile.id) == default_profile_id,
                )
            })
            .collect();
        let menu_entries: Vec<(String, bool)> = profiles
            .iter()
            .map(|(_, name, is_default)| (name.clone(), *is_default))
            .collect();
        let Some(menu) = prepare_profile_menu(window, menu_entries, self.language) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Some(NativeProfileMenuCommand::PickProfile(index)) = menu.show() else {
                return;
            };
            // Re-fetch by id: the menu's index is only meaningful for the list as it was at open
            // time, and a remote profile may need a consent prompt, which requires a live window.
            this.update_in(cx, move |app, window, cx| {
                let Some(profile) = profiles
                    .get(index)
                    .and_then(|(id, _, _)| app.profile_store.read(cx).profile(*id).cloned())
                else {
                    return;
                };
                app.new_terminal_with_profile(group_id, None, None, profile, window, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_native_tab_context_menu(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_item(group_id, item_id, window, cx);
        cx.stop_propagation();
        let directions = [
            Direction::Up,
            Direction::Down,
            Direction::Left,
            Direction::Right,
        ];
        let move_enabled = directions.map(|direction| {
            self.workspace_state
                .workspace()
                .layout
                .neighbor_group_id(group_id, direction)
                .is_some()
        });
        let clear_title_enabled = self
            .workspace_state
            .workspace()
            .layout
            .find_group(group_id)
            .and_then(|group| group.items.iter().find(|item| item.id == item_id))
            .is_some_and(|item| item.custom_title.is_some());
        let Some(menu) = prepare_tab_menu(window, move_enabled, clear_title_enabled, self.language)
        else {
            return;
        };
        // AppKit menus are modal and run a nested event loop. Start the menu from a foreground
        // future after this GPUI listener has returned so polling tasks cannot re-enter the App
        // while its entity RefCell is still mutably borrowed.
        cx.spawn(async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            // `update_in` rather than `update`: creating a terminal may need to put a consent prompt
            // on screen (a remote profile on an unanswered host), which requires a live window. The
            // menu's own `window` cannot cross the await.
            this.update_in(cx, |app, window, cx| {
                let item_still_exists = app
                    .workspace_state
                    .workspace()
                    .layout
                    .find_group(group_id)
                    .is_some_and(|group| group.items.iter().any(|item| item.id == item_id));
                if !item_still_exists {
                    return;
                }
                match command {
                    NativeTabMenuCommand::Split(direction) => {
                        app.new_terminal(group_id, Some(direction), Some(item_id), window, cx);
                    }
                    NativeTabMenuCommand::Move(direction) => {
                        app.move_tab_in_direction(group_id, item_id, direction, cx);
                    }
                    NativeTabMenuCommand::SetCustomTitle => {
                        app.prompt_custom_title(group_id, item_id, cx);
                    }
                    NativeTabMenuCommand::ClearCustomTitle => {
                        app.set_custom_title(group_id, item_id, None, cx);
                    }
                    NativeTabMenuCommand::Background => app.background_tab(group_id, item_id, cx),
                    NativeTabMenuCommand::MoveToNewWindow => {
                        let source = app.item_location(group_id, item_id);
                        app.move_tab_to_new_window(source, cx);
                    }
                    NativeTabMenuCommand::MoveToWindow => {
                        let source = app.item_location(group_id, item_id);
                        app.open_palette(PaletteMode::MoveToWindow(source), window, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn move_tab_in_direction(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        direction: Direction,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace_state.workspace_mut();
        let Some(target_group_id) = workspace.layout.neighbor_group_id(group_id, direction) else {
            cx.notify();
            return;
        };
        if workspace
            .layout
            .move_item(group_id, item_id, target_group_id)
            .is_some()
        {
            workspace.active_group_id = target_group_id;
        }
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    /// Set (or clear, with `None`) a tab's custom title override, then repaint. The custom title is
    /// per-tab layout metadata; it wins over the process/OSC title without ever mutating it, so
    /// clearing restores the live process title.
    pub(super) fn set_custom_title(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        custom_title: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspace_state
            .workspace_mut()
            .layout
            .set_custom_title(group_id, item_id, custom_title)
        {
            cx.notify();
        }
    }

    /// Open a native prompt to set or edit a tab's custom title. The field is prefilled with the
    /// tab's current display title (custom title if set, otherwise the process title). An empty
    /// result clears the override so the process title takes over again.
    pub(super) fn prompt_custom_title(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        cx: &mut Context<Self>,
    ) {
        let language = self.language;
        let current = self
            .workspace_state
            .workspace()
            .layout
            .find_group(group_id)
            .and_then(|group| group.items.iter().find(|item| item.id == item_id))
            .map(|item| item.display_title().to_owned())
            .unwrap_or_default();
        let Some(window) = cx.active_window() else {
            return;
        };
        let app = cx.entity().downgrade();
        window
            .update(cx, |_, window, cx| {
                let receiver = prompt_for_text(
                    window,
                    language.custom_title_prompt_title(),
                    Some(language.custom_title_prompt_message()),
                    Some(language.custom_title_placeholder()),
                    Some(&current),
                    language.ok(),
                    language.cancel(),
                );
                cx.spawn(async move |cx| {
                    let value = receiver.await.ok()??;
                    let trimmed = value.trim();
                    let custom_title = if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_owned())
                    };
                    app.update(cx, move |app, cx| {
                        app.set_custom_title(group_id, item_id, custom_title, cx);
                    })
                    .ok()?;
                    Some(())
                })
                .detach();
            })
            .ok();
    }

    pub(super) fn clear_tab_drop_target(&mut self, cx: &mut Context<Self>) {
        if self.tab_drop_target.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn update_content_drop_target(
        &mut self,
        group_id: GroupId,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }

        let x = f32::from(event.event.position.x - event.bounds.left());
        let group_y = f32::from(event.event.position.y - event.bounds.top());
        if group_y < TAB_BAR_HEIGHT {
            return;
        }

        let dragged_tab = event
            .dragged_item()
            .downcast_ref::<DraggedTab>()
            .expect("drag move type must match DraggedTab");
        let source_item_count = self
            .workspace_state
            .workspace()
            .layout
            .find_group(dragged_tab.source_group_id)
            .map_or(0, |group| group.items.len());
        if !should_show_content_drop_target(
            dragged_tab.source_group_id,
            group_id,
            source_item_count,
        ) {
            self.clear_tab_drop_target(cx);
            return;
        }

        let width = f32::from(event.bounds.size.width);
        let height = (f32::from(event.bounds.size.height) - TAB_BAR_HEIGHT).max(0.);
        let y = group_y - TAB_BAR_HEIGHT;
        let zone = content_drop_zone(width, height, x, y);
        let target = Some(TabDropTarget { group_id, zone });
        if self.tab_drop_target != target {
            self.tab_drop_target = target;
            cx.notify();
        }
    }

    pub(super) fn update_tab_bar_drop_target(
        &mut self,
        group_id: GroupId,
        tab_index: usize,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let relative_x = f32::from(event.event.position.x - event.bounds.left());
        let insertion_index =
            tab_bar_insertion_index(tab_index, f32::from(event.bounds.size.width), relative_x);
        let target = Some(TabDropTarget {
            group_id,
            zone: TabDropZone::TabBar { insertion_index },
        });
        if self.tab_drop_target != target {
            self.tab_drop_target = target;
            cx.notify();
        }
    }

    pub(super) fn update_tab_bar_trailing_drop_target(
        &mut self,
        group_id: GroupId,
        insertion_index: usize,
        event: &DragMoveEvent<DraggedTab>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let target = Some(TabDropTarget {
            group_id,
            zone: TabDropZone::TabBar { insertion_index },
        });
        if self.tab_drop_target != target {
            self.tab_drop_target = target;
            cx.notify();
        }
    }

    pub(super) fn handle_tab_drop(&mut self, dragged_tab: &DraggedTab, cx: &mut Context<Self>) {
        let Some(target) = self.tab_drop_target.take() else {
            return;
        };
        if dragged_tab.source_window_id != self.window_id {
            self.publish_window_state(cx);
            let source = eggie_domain::ItemLocation {
                window_id: dragged_tab.source_window_id,
                project_id: dragged_tab.project_id,
                group_id: dragged_tab.source_group_id,
                item_id: dragged_tab.item_id,
            };
            // A drop keeps the terminal's project while adding its view to the target window.
            let group = if dragged_tab.project_id == self.workspace_state.active_project {
                target.group_id
            } else {
                self.window_manager.read(cx).state.windows[&self.window_id].views
                    [&dragged_tab.project_id]
                    .active_group_id
            };
            let destination = eggie_domain::PresentationTarget {
                window_id: self.window_id,
                project_id: dragged_tab.project_id,
                group_id: group,
                insertion_index: match target.zone {
                    TabDropZone::TabBar { insertion_index } => Some(insertion_index),
                    _ => None,
                },
                split: match target.zone {
                    TabDropZone::Edge(direction) => Some(direction),
                    _ => None,
                },
            };
            self.window_manager
                .update(cx, |manager, cx| manager.move_item(source, destination, cx));
            self.pull_window_state(cx);
            self.project_picker = false;
            cx.notify();
            return;
        }
        let workspace = self.workspace_state.workspace_mut();
        let destination_group = match target.zone {
            TabDropZone::Center => workspace.layout.move_item(
                dragged_tab.source_group_id,
                dragged_tab.item_id,
                target.group_id,
            ),
            TabDropZone::Edge(direction) => workspace.layout.move_item_to_new_group(
                dragged_tab.source_group_id,
                dragged_tab.item_id,
                target.group_id,
                direction,
            ),
            TabDropZone::TabBar { insertion_index } => workspace.layout.move_item_at(
                dragged_tab.source_group_id,
                dragged_tab.item_id,
                target.group_id,
                insertion_index,
            ),
        };
        if let Some(destination_group) = destination_group {
            workspace.active_group_id = destination_group;
        }
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    // Order a project's sessions to match the reading order of its layout tree — left-to-right,
    // top-to-bottom, and within a tab group by tab order. This keeps the sidebar list in sync with
    // how the terminals are actually arranged in the main area (splits and tab reordering), rather
    // than showing them in creation order.
    //
    // Sessions that belong to the project but are absent from the layout tree (which should not
    // normally happen) are appended afterwards in their existing order so a terminal can never
    // silently disappear from the sidebar.
}

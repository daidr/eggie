use super::*;

impl EggieApp {
    // AppKit delivers an internal drag to its source window even when the pointer leaves it.
    pub(in crate::app) fn route_cross_window_drag(
        &self,
        pointer: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.dragged_tab.is_none() {
            return;
        }
        let outside_source = !window.bounds().contains(&pointer);
        let handles = self.window_manager.read(cx).handles.clone();
        for (id, handle) in handles {
            if id == self.window_id {
                continue;
            }
            let _ = handle.update(cx, |app, target, cx| {
                let hit = outside_source && target.bounds().contains(&pointer);
                let next = hit
                    .then(|| app.tab_drop_target_at(pointer - target.bounds().origin))
                    .flatten();
                if app.tab_drop_target != next {
                    app.tab_drop_target = next;
                    cx.notify();
                }
            });
        }
    }

    fn tab_drop_target_at(&self, pointer: Point<Pixels>) -> Option<TabDropTarget> {
        if self.project_picker {
            return Some(TabDropTarget {
                group_id: self.workspace_state.workspace().active_group_id,
                zone: TabDropZone::Center,
            });
        }
        for (&group_id, bounds) in self.group_drop_bounds.borrow().iter() {
            if !bounds.contains(&pointer) {
                continue;
            }
            let x = f32::from(pointer.x - bounds.left());
            let y = f32::from(pointer.y - bounds.top());
            let zone = if y < TAB_BAR_HEIGHT {
                let insertion_index = self
                    .workspace_state
                    .workspace()
                    .layout
                    .find_group(group_id)
                    .map_or(0, |group| {
                        group
                            .items
                            .iter()
                            .enumerate()
                            .find_map(|(index, item)| {
                                let bounds =
                                    self.tab_drop_bounds.borrow().get(&item.id).copied()?;
                                (pointer.x <= bounds.right()).then(|| {
                                    tab_bar_insertion_index(
                                        index,
                                        f32::from(bounds.size.width),
                                        f32::from(pointer.x - bounds.left()),
                                    )
                                })
                            })
                            .unwrap_or(group.items.len())
                    });
                TabDropZone::TabBar { insertion_index }
            } else {
                content_drop_zone(
                    f32::from(bounds.size.width),
                    (f32::from(bounds.size.height) - TAB_BAR_HEIGHT).max(0.),
                    x,
                    y - TAB_BAR_HEIGHT,
                )
            };
            return Some(TabDropTarget { group_id, zone });
        }
        None
    }

    pub(in crate::app) fn finish_tab_tear_out(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.dragged_tab.take() else {
            return;
        };
        self.clear_tab_drop_target(cx);
        let pointer = event.position + window.bounds().origin;
        let handles = self.window_manager.read(cx).handles.clone();
        let mut inside_window = false;
        for (id, handle) in handles {
            if id == self.window_id {
                continue;
            }
            let _ = handle.update(cx, |app, target, cx| {
                if target.bounds().contains(&pointer) {
                    inside_window = true;
                    app.tab_drop_target = app.tab_drop_target_at(pointer - target.bounds().origin);
                    if app.tab_drop_target.is_some() {
                        app.handle_tab_drop(&tab, cx);
                        target.activate_window();
                        app.focus_handle.focus(target, cx);
                    }
                } else {
                    app.clear_tab_drop_target(cx);
                }
            });
            if inside_window {
                break;
            }
        }
        cx.stop_active_drag(window);
        let source = ItemLocation {
            window_id: tab.source_window_id,
            project_id: tab.project_id,
            group_id: tab.source_group_id,
            item_id: tab.item_id,
        };
        if !inside_window && self.window_manager.read(cx).state.item(source).is_some() {
            self.move_tab_to_new_window_at(source, Some(pointer), cx);
        }
    }
    pub(in crate::app) fn item_location(&self, group_id: GroupId, item_id: ItemId) -> ItemLocation {
        ItemLocation {
            window_id: self.window_id,
            project_id: self.workspace_state.active_project,
            group_id,
            item_id,
        }
    }

    pub(in crate::app) fn target_for_project(
        &self,
        project_id: ProjectId,
        cx: &App,
    ) -> PresentationTarget {
        let group_id = self.window_manager.read(cx).state.windows[&self.window_id].views
            [&project_id]
            .active_group_id;
        PresentationTarget {
            window_id: self.window_id,
            project_id,
            group_id,
            insertion_index: None,
            split: None,
        }
    }

    pub(in crate::app) fn background_tab(
        &mut self,
        group: GroupId,
        item: ItemId,
        cx: &mut Context<Self>,
    ) {
        self.publish_window_state(cx);
        let source = self.item_location(group, item);
        self.window_manager
            .update(cx, |manager, cx| manager.background_item(source, cx));
        self.pull_window_state(cx);
        cx.notify();
    }

    pub(in crate::app) fn move_tab_to_window(
        &mut self,
        source: ItemLocation,
        target_window: WindowId,
        cx: &mut Context<Self>,
    ) {
        self.publish_window_state(cx);
        let manager = self.window_manager.read(cx);
        let Some(target_view) = manager
            .state
            .windows
            .get(&target_window)
            .and_then(|w| w.views.get(&source.project_id))
        else {
            return;
        };
        let Some(handle) = manager.handles.get(&target_window).copied() else {
            return;
        };
        let target = PresentationTarget {
            window_id: target_window,
            project_id: source.project_id,
            group_id: target_view.active_group_id,
            insertion_index: None,
            split: None,
        };
        let moved = self
            .window_manager
            .update(cx, |manager, cx| manager.move_item(source, target, cx));
        self.pull_window_state(cx);
        if let Some(location) = moved {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |app, window, cx| {
                    app.pull_window_state(cx);
                    app.project_picker = false;
                    app.activate_project(location.project_id, cx);
                    app.activate_item(location.group_id, location.item_id, window, cx);
                    window.activate_window();
                });
            });
        }
        cx.notify();
    }

    pub(in crate::app) fn move_tab_to_new_window(
        &mut self,
        source: ItemLocation,
        cx: &mut Context<Self>,
    ) {
        self.move_tab_to_new_window_at(source, None, cx);
    }

    fn move_tab_to_new_window_at(
        &mut self,
        source: ItemLocation,
        pointer: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        self.publish_window_state(cx);
        let (client, settings, updates, projects, profiles, snippets, notes, routes) = (
            self.client.clone(),
            self.settings.clone(),
            self.updates.clone(),
            self.project_store.clone(),
            self.profile_store.clone(),
            self.snippet_store.clone(),
            self.notes_store.clone(),
            self.notification_routes.clone(),
        );
        let app = cx.weak_entity();
        cx.defer(move |cx| {
            if app.upgrade().is_none() {
                return;
            }
            if let Some(handle) = Self::open_new_window(
                cx,
                client,
                settings,
                updates,
                projects,
                profiles,
                snippets,
                notes,
                routes,
                pointer.map_or(NewWindowInit::Empty, NewWindowInit::EmptyAt),
            ) {
                let id = handle.read(cx).ok().map(|app| app.window_id);
                if let Some(id) = id {
                    let _ = app.update(cx, |app, cx| app.move_tab_to_window(source, id, cx));
                    let manager = WindowManager::global(cx);
                    if manager.read(cx).state.item(source).is_some() {
                        let _ = handle.update(cx, |app, window, _| {
                            app.closing = true;
                            window.remove_window();
                        });
                        manager.update(cx, |manager, cx| manager.discard_empty_window(id, cx));
                    } else {
                        manager.update(cx, |manager, _| {
                            manager.include_created_window_in_undo(id, source)
                        });
                    }
                }
            }
        });
    }
}

use super::*;

impl EggieApp {
    pub(in crate::app) fn undo_window_operation(&mut self, cx: &mut Context<Self>) {
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
        cx.defer(move |cx| {
            Self::undo_shared_window_operation(
                client, settings, updates, projects, profiles, snippets, notes, routes, cx,
            )
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn undo_shared_window_operation(
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        projects: Entity<ProjectStore>,
        profiles: Entity<ProfileStore>,
        snippets: Entity<SnippetStore>,
        notes: Entity<NotesStore>,
        routes: NotificationRoutes,
        cx: &mut App,
    ) {
        let manager = WindowManager::global(cx);
        if !manager.update(cx, |manager, cx| manager.undo(cx)) {
            return;
        }
        let project_list = projects.read(cx).projects().to_vec();
        manager.update(cx, |manager, cx| {
            manager.reconcile_projects(&project_list, cx)
        });
        let state = manager.read(cx).state.clone();
        let handles = manager.read(cx).handles.clone();
        for (id, handle) in &handles {
            if !state.windows.contains_key(id) {
                let _ = handle.update(cx, |app, window, cx| {
                    app.closing = true;
                    app.dismiss_project_popover(cx);
                    app.dismiss_sidebar_preview(cx);
                    window.remove_window();
                });
                manager.update(cx, |manager, _| {
                    manager.handles.remove(id);
                });
            }
        }
        for window in state.windows.values() {
            if !handles.contains_key(&window.id) {
                Self::open_new_window(
                    cx,
                    client.clone(),
                    settings.clone(),
                    updates.clone(),
                    projects.clone(),
                    profiles.clone(),
                    snippets.clone(),
                    notes.clone(),
                    routes.clone(),
                    NewWindowInit::Restore(window.clone()),
                );
            }
        }
        if let Some(handle) = state
            .active_window
            .and_then(|id| manager.read(cx).handles.get(&id).copied())
        {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    }
    fn presentation(&self) -> WindowPresentation {
        WindowPresentation {
            id: self.window_id,
            number: self.window_number,
            project_picker: self.project_picker,
            active_project: self.workspace_state.active_project,
            views: self.workspace_state.project_views.clone(),
            collapsed_projects: self.workspace_state.collapsed_projects.clone(),
            left_sidebar_collapsed: self.left_sidebar_collapsed,
            right_sidebar_collapsed: self.right_sidebar_collapsed,
            left_sidebar_width: self.left_sidebar_width,
            right_sidebar_width: self.right_sidebar_width,
            frame: self.window_frame,
        }
    }

    pub(in crate::app) fn publish_window_state(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let revision = self
            .window_manager
            .read(cx)
            .revisions
            .get(&self.window_id)
            .copied()
            .unwrap_or(0);
        // A shared transaction takes precedence over an older local projection.
        if self.window_revision != revision {
            self.pull_window_state(cx);
            return;
        }
        if self
            .window_manager
            .read(cx)
            .state
            .windows
            .get(&self.window_id)
            .is_some_and(|state| {
                state.active_project == self.workspace_state.active_project
                    && state.project_picker == self.project_picker
                    && state.views == self.workspace_state.project_views
                    && state.collapsed_projects == self.workspace_state.collapsed_projects
                    && state.left_sidebar_collapsed == self.left_sidebar_collapsed
                    && state.right_sidebar_collapsed == self.right_sidebar_collapsed
                    && state.left_sidebar_width == self.left_sidebar_width
                    && state.right_sidebar_width == self.right_sidebar_width
                    && state.frame == self.window_frame
            })
        {
            return;
        }
        let state = self.presentation();
        let expected = self.window_revision;
        match self
            .window_manager
            .update(cx, |manager, cx| manager.capture(state, Some(expected), cx))
        {
            Ok(revision) => self.window_revision = revision,
            Err(()) => self.pull_window_state(cx),
        }
    }

    pub(in crate::app) fn pull_window_state(&mut self, cx: &mut Context<Self>) {
        let manager = self.window_manager.read(cx);
        let Some(state) = manager.state.windows.get(&self.window_id) else {
            return;
        };
        let revision = manager.revisions.get(&self.window_id).copied().unwrap_or(0);
        let state = (self.window_revision != revision).then(|| state.clone());
        self.window_revision = revision;
        let sessions = manager.sessions.clone();
        self.session_feed.progress_updates = manager.progress.clone();
        let error = manager
            .error
            .clone()
            .or_else(|| manager.connection_error.clone());
        if let Some(state) = state {
            self.project_picker = state.project_picker;
            self.window_number = state.number;
            self.workspace_state.active_project = state.active_project;
            self.workspace_state.project_views = state.views;
            self.workspace_state.collapsed_projects = state.collapsed_projects;
            self.left_sidebar_collapsed = state.left_sidebar_collapsed;
            self.right_sidebar_collapsed = state.right_sidebar_collapsed;
            self.left_sidebar_width = state.left_sidebar_width;
            self.right_sidebar_width = state.right_sidebar_width;
        }
        self.apply_session_list(sessions, cx);
        let local: HashSet<_> = self.window_session_ids().into_iter().collect();
        self.session_feed
            .snapshots
            .retain(|id, _| local.contains(id));
        self.terminal_sizes.retain(|id, _| local.contains(id));
        self.terminal_ime_states.retain(|id, _| local.contains(id));
        self.session_feed
            .session_inspections
            .retain(|id, _| local.contains(id));
        self.session_feed
            .shell_integration
            .retain(|id, _| local.contains(id));
        if self
            .sidebar
            .sidebar_preview
            .as_ref()
            .is_some_and(|p| !local.contains(&p.session_id))
        {
            self.dismiss_sidebar_preview(cx);
        }
        self.poll_error = error;
        self.reconcile_layout_caches();
        self.sync_terminal_focus();
    }

    pub(in crate::app) fn install_window_lifecycle(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.window_revision = self
            .window_manager
            .read(cx)
            .revisions
            .get(&self.window_id)
            .copied()
            .unwrap_or(0);
        let manager = self.window_manager.clone();
        cx.observe_in(&manager, window, |app, _, window, cx| {
            if app.closing {
                return;
            }
            app.pull_window_state(cx);
            if let Some(selector) = &app.sidebar.project_selector {
                let _ = selector.window.update(cx, |_, _, cx| cx.notify());
            }
            if let Some(popover) = &app.sidebar.project_popover {
                let _ = popover.window.update(cx, |_, _, cx| cx.notify());
            }
            if let Some(preview) = &app.sidebar.sidebar_preview {
                let _ = preview.window.update(cx, |_, _, cx| cx.notify());
            }
            app.update_window_title(window, cx);
            app.ensure_progress_animation(window, cx);
            cx.notify();
        })
        .detach();
        cx.observe_self(|app, cx| app.publish_window_state(cx))
            .detach();
        cx.observe_window_bounds(window, |app, window, cx| {
            let bounds = window.bounds();
            app.window_frame = Some(WindowFrame {
                x: bounds.origin.x.into(),
                y: bounds.origin.y.into(),
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
            });
            app.publish_window_state(cx);
        })
        .detach();
        cx.observe_window_activation(window, |app, window, cx| {
            if window.is_window_active() {
                app.window_manager
                    .update(cx, |manager, cx| manager.focused(app.window_id, cx));
            }
        })
        .detach();
        let app = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = app.update(cx, |app, cx| app.close_presentation(window, cx));
            true
        });
        self.update_window_title(window, cx);
    }

    fn update_window_title(&self, window: &mut Window, cx: &App) {
        window.set_window_title(&self.window_label(self.window_id, cx));
    }

    pub(in crate::app) fn window_label(&self, id: WindowId, cx: &App) -> String {
        self.window_manager.read(cx).window_label(id)
    }

    pub(crate) fn close_presentation(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.publish_window_state(cx);
        self.closing = true;
        self.dismiss_project_popover(cx);
        self.dismiss_sidebar_preview(cx);
        self.window_manager
            .update(cx, |manager, cx| manager.close(self.window_id, cx));
    }
}

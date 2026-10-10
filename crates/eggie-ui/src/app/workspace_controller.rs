use super::*;

impl EggieApp {
    pub(super) fn persist_workspace(&self) {
        let views = self
            .workspace_state
            .project_views
            .iter()
            .map(|(id, view)| (*id, view.clone()))
            .collect();
        WorkspaceRestoreState {
            active_project: self.workspace_state.active_project,
            views,
            left_sidebar_collapsed: self.left_sidebar_collapsed,
            right_sidebar_collapsed: self.right_sidebar_collapsed,
            left_sidebar_width: self.left_sidebar_width,
            right_sidebar_width: self.right_sidebar_width,
        }
        .save();
    }

    pub(super) fn install_close_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            app.update(cx, |app, cx| {
                if app.closing || app.session_feed.sessions.is_empty() {
                    return true;
                }
                let language = app.language;
                let prompt = window.prompt(
                    PromptLevel::Warning,
                    language.close_window_title(),
                    Some(language.close_window_message()),
                    &[
                        language.terminate_all(),
                        language.detach(),
                        language.cancel(),
                    ],
                    cx,
                );
                let app = cx.entity().downgrade();
                cx.spawn_in(window, async move |_, cx| {
                    let Ok(choice) = prompt.await else {
                        return;
                    };
                    if choice == 2 {
                        return;
                    }
                    let _ = cx.update(|window, cx| {
                        let _ = app.update(cx, |app, _| {
                            // Persist before the window is torn down: on_app_quit's weak entity is
                            // already dead by the time the async platform quit runs, so the close
                            // path must save explicitly. Harmless for both choices — dead sessions
                            // are pruned on restore.
                            app.persist_workspace();
                            if choice == 0 {
                                // Terminate all: kill this window's sessions.
                                for session_id in app.window_session_ids() {
                                    let _ =
                                        app.client.request(ClientRequest::Terminate { session_id });
                                }
                            } else {
                                // Detach (choice == 1): clear this window's ownership so the sessions
                                // survive as claimable, unowned terminals instead of pointing at a
                                // window that is about to be destroyed.
                                let _ = app.client.request(ClientRequest::DetachWindow {
                                    window_id: app.window_id,
                                });
                            }
                            app.closing = true;
                        });
                        window.remove_window();
                    });
                })
                .detach();
                false
            })
            .unwrap_or(true)
        });
    }

    /// Rebuild `project_views` and the local `projects` mirror to track the shared store: create an
    /// empty view for each newly-synced project, drop views for removed projects, and keep
    /// `active_project` pointing at a project that still exists.
    pub(super) fn reconcile_project_views(&mut self, cx: &mut Context<Self>) {
        self.workspace_state.projects = self.project_store.read(cx).projects().to_vec();
        // Add views for projects that don't have one yet (synced in from another window).
        for project in &self.workspace_state.projects {
            self.workspace_state
                .project_views
                .entry(project.id)
                .or_insert_with(WindowProjectView::empty);
        }
        // Drop views for projects that no longer exist in the shared list.
        let live: HashSet<ProjectId> = self
            .workspace_state
            .projects
            .iter()
            .map(|project| project.id)
            .collect();
        if self
            .sidebar
            .project_popover
            .as_ref()
            .is_some_and(|popover| !live.contains(&popover.project_id))
        {
            self.dismiss_project_popover(cx);
        }
        self.workspace_state
            .project_views
            .retain(|id, _| live.contains(id));
        self.workspace_state
            .collapsed_projects
            .retain(|id| live.contains(id));
        self.sidebar
            .project_row_bounds
            .borrow_mut()
            .retain(|id, _| live.contains(id));
        if self
            .sidebar
            .project_drag
            .source
            .is_some_and(|id| !live.contains(&id))
        {
            self.clear_project_drag(cx);
        }
        // Keep the active project valid; fall back to the first project if it was removed.
        if !live.contains(&self.workspace_state.active_project)
            && let Some(first) = self.workspace_state.projects.first()
        {
            self.workspace_state.active_project = first.id;
        }
    }

    /// Reap per-`GroupId` / per-`SplitId` render caches whose layout node no longer exists.
    ///
    /// `GroupId`/`SplitId` are globally unique across every project's layout, and these caches are
    /// not partitioned by project, so the live set must span *all* `project_views` — retaining
    /// against only the active workspace would wrongly drop another project's entries. Called after
    /// any layout mutation (tab/split close) so closed nodes don't leak bounds/handles for the
    /// window's lifetime.
    pub(super) fn reconcile_layout_caches(&mut self) {
        let live_groups: HashSet<GroupId> = self
            .workspace_state
            .project_views
            .values()
            .flat_map(|view| view.layout.group_ids())
            .collect();
        let live_splits: HashSet<SplitId> = self
            .workspace_state
            .project_views
            .values()
            .flat_map(|view| view.layout.split_ids())
            .collect();
        self.terminal_viewports
            .retain(|group_id, _| live_groups.contains(group_id));
        self.scrollbar_bounds
            .borrow_mut()
            .retain(|group_id, _| live_groups.contains(group_id));
        self.tab_bar_scroll_handles
            .borrow_mut()
            .retain(|group_id, _| live_groups.contains(group_id));
        self.split_bounds
            .borrow_mut()
            .retain(|split_id, _| live_splits.contains(split_id));
    }

    pub(super) fn active_session_id(&self) -> Option<SessionId> {
        self.workspace_state
            .workspace()
            .layout
            .find_group(self.workspace_state.workspace().active_group_id)?
            .active_item()?
            .session_id
    }

    /// Whether the right sidebar should force the Notes tab and disable Info/Commands: true when
    /// the active terminal has exited (e.g. the shell ran `exit`) or there is no active session at
    /// all. Info/Commands are meaningless without a live shell, so we steer the user to Notes.
    pub(super) fn notes_forced(&self) -> bool {
        match self.active_session_id() {
            None => true,
            Some(id) => self
                .session_feed
                .sessions
                .iter()
                .find(|session| session.id == id)
                .map(|session| matches!(session.status, SessionStatus::Exited { .. }))
                .unwrap_or(true),
        }
    }

    /// Returns the active terminal tab in every visible split group. This is intentionally
    /// independent from `active_group_id`: that field selects the keyboard input target, while
    /// every session returned here must keep receiving snapshots and rendering frames.
    pub(super) fn visible_active_session_ids(&self) -> Vec<SessionId> {
        visible_active_terminal_sessions(&self.workspace_state.workspace().layout)
    }

    pub(super) fn is_visible_active_session(&self, session_id: SessionId) -> bool {
        layout_has_visible_active_terminal(&self.workspace_state.workspace().layout, session_id)
    }

    pub(super) fn set_terminal_has_focus(&mut self, focused: bool) {
        self.terminal_has_focus = focused;
        self.sync_terminal_focus();
    }

    pub(super) fn sync_terminal_focus(&mut self) {
        let next_session = self
            .terminal_has_focus
            .then(|| self.active_session_id())
            .flatten();
        if self.terminal_focused_session == next_session {
            return;
        }
        if let Some(session_id) = self.terminal_focused_session.take()
            && let Err(error) = self.input_sender.send_focus(session_id, false)
        {
            eprintln!("failed to enqueue terminal focus-out: {error:#}");
        }
        if let Some(session_id) = next_session {
            if let Err(error) = self.input_sender.send_focus(session_id, true) {
                eprintln!("failed to enqueue terminal focus-in: {error:#}");
                return;
            }
            self.terminal_focused_session = Some(session_id);
        }
    }

    /// Ids of the sessions this window owns. `self.session_feed.sessions` is already filtered to this window's
    /// sessions by the polling classifier, so this is just their ids.
    pub(super) fn window_session_ids(&self) -> Vec<SessionId> {
        self.session_feed
            .sessions
            .iter()
            .map(|session| session.id)
            .collect()
    }

    /// Keeps the daemon session summary and every tab model in lockstep. The sidebar renders the
    /// former while group tab bars render the latter, so updating only one produces divergent OSC
    /// titles until the window is rebuilt.
    pub(super) fn synchronize_session_title(&mut self, session_id: SessionId, title: &str) -> bool {
        let mut changed = false;
        if let Some(session) = self
            .session_feed
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
            && session.title != title
        {
            session.title = title.to_owned();
            changed = true;
        }
        for view in self.workspace_state.project_views.values_mut() {
            changed |= view.layout.update_terminal_title(session_id, title);
        }
        changed
    }

    /// Claim a detached session into this window: set its owner to this window in the daemon, then
    /// place it as a tab in its project's layout. If the session's project isn't in the shared list
    /// (its project was deleted), fall back to the active project. Activates the claimed session.
    pub(super) fn claim_detached_session(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(DaemonResponse::SessionClaimed { session }) =
            self.client.request(ClientRequest::ClaimSession {
                session_id,
                window_id: self.window_id,
            })
        else {
            return;
        };
        // Choose the project this session lands in: its own project if this window has a view for it,
        // otherwise the currently active project (orphaned project_id, R8).
        let target_project = if self
            .workspace_state
            .project_views
            .contains_key(&session.project_id)
        {
            session.project_id
        } else {
            self.workspace_state.active_project
        };
        if let Some(view) = self.workspace_state.project_views.get_mut(&target_project) {
            let group_id = view.active_group_id;
            if let Some(group) = view.layout.find_group_mut(group_id) {
                group.add_item(TabItem::terminal(session.id, session.title.clone()));
            }
        }
        self.configure_session_progress(session.id);
        self.configure_session_osc_policy(session.id);
        self.session_feed.sessions.push(session);
        self.session_feed
            .detached_sessions
            .retain(|candidate| candidate.id != session_id);
        if self.session_feed.detached_sessions.is_empty() {
            self.detached_popover_open = false;
        }
        self.workspace_state.active_project = target_project;
        self.activate_session(target_project, session_id, window, cx);
        self.ensure_snapshot_watchers(cx);
        self.ensure_progress_watchers(cx);
        self.ensure_osc_watchers(cx);
        cx.notify();
    }

    /// Permanently terminate a detached session from the popover.
    pub(super) fn destroy_detached_session(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) {
        let _ = self.client.request(ClientRequest::Terminate { session_id });
        self.session_feed
            .detached_sessions
            .retain(|candidate| candidate.id != session_id);
        if self.session_feed.detached_sessions.is_empty() {
            self.detached_popover_open = false;
        }
        cx.notify();
    }

    pub(super) fn configure_session_progress(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetProgressTimeouts {
            session_id,
            timeouts: self.progress_timeouts,
        }) {
            eprintln!("failed to configure terminal progress for {session_id}: {error:#}");
        }
    }

    pub(super) fn configure_session_osc_policy(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetOscPolicy {
            session_id,
            allow_clipboard_read: self.allow_osc_clipboard_read,
        }) {
            eprintln!("failed to configure terminal OSC policy for {session_id}: {error:#}");
        }
    }

    pub(super) fn configure_session_url_detection(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetUrlDetection {
            session_id,
            detect_urls: self.detect_urls,
        }) {
            eprintln!("failed to configure terminal URL detection for {session_id}: {error:#}");
        }
    }

    pub(super) fn configure_session_cursor_shape(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetCursorStyle {
            session_id,
            shape: self.cursor_shape.to_protocol(),
        }) {
            eprintln!("failed to configure terminal cursor style for {session_id}: {error:#}");
        }
    }

    pub(super) fn configure_session_scrollback(&self, session_id: SessionId) {
        if let Err(error) = self.client.request(ClientRequest::SetScrollbackLimit {
            session_id,
            limit: self.scrollback_limit,
        }) {
            eprintln!("failed to configure terminal scrollback for {session_id}: {error:#}");
        }
    }

    /// The live working directory of the session a new terminal should inherit from: the OSC 7-reported
    /// cwd (falling back to the foreground process cwd in the daemon), so a new tab or split opens
    /// beside where the user is working. `source_item` names a specific tab the creation was requested
    /// from (the tab context menu's Split); when present, its session's cwd wins over the group's
    /// active tab. `None` when the tab has no session (empty group, non-terminal item) or no summary
    /// has arrived yet.
    ///
    /// Returned raw (not existence-checked) because the callers layer different fallbacks on top:
    /// [`resolve_spawn_config`] lets a profile's own `cwd` win first, then applies the existence check.
    pub(super) fn active_session_cwd(
        &self,
        group_id: GroupId,
        source_item: Option<ItemId>,
    ) -> Option<PathBuf> {
        let session_id = match source_item {
            Some(item_id) => self
                .workspace_state
                .workspace()
                .layout
                .find_group(group_id)
                .and_then(|group| group.items.iter().find(|item| item.id == item_id))
                .and_then(|item| item.session_id),
            None => self
                .workspace_state
                .workspace()
                .layout
                .find_group(group_id)
                .and_then(|group| group.active_item())
                .and_then(|item| item.session_id),
        };
        session_id
            .and_then(|session_id| {
                self.session_feed
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
            })
            .map(|session| session.current_directory.clone())
    }

    /// The profile a new terminal in this project should launch from: the project's configured
    /// default, falling back to the built-in Default when unset or stale (see
    /// [`ProfileStore::resolve`]).
    pub(super) fn new_terminal_profile(&self, cx: &App) -> Profile {
        let configured = self
            .workspace_state
            .active_project_data()
            .default_profile_id;
        self.profile_store.read(cx).resolve(configured).clone()
    }

    pub(super) fn new_terminal(
        &mut self,
        group_id: GroupId,
        split: Option<Direction>,
        source_item: Option<ItemId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let profile = self.new_terminal_profile(cx);
        self.new_terminal_with_profile(group_id, split, source_item, profile, window, cx);
    }

    /// Ask whether Eggie may install its shell integration on `destination`, remember the answer, and
    /// then create the terminal either way.
    ///
    /// Asked once per `user@host` (the install lands in that account's home directory), and a refusal
    /// is remembered too — declining must not mean being asked again on every new terminal. Either
    /// answer proceeds to a session: consent adds the integration, refusal opens a plain ssh session.
    /// Dismissing the prompt without choosing cancels the terminal instead of guessing.
    pub(super) fn prompt_remote_install_consent(
        &mut self,
        destination: String,
        group_id: GroupId,
        split: Option<Direction>,
        source_item: Option<ItemId>,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = self.language;
        let prompt = window.prompt(
            PromptLevel::Info,
            language.remote_install_title(),
            Some(&language.remote_install_message(&destination)),
            &[
                language.remote_install_allow(),
                language.remote_install_skip(),
            ],
            cx,
        );
        let app = cx.entity().downgrade();
        cx.spawn_in(window, async move |_, cx| {
            // A dismissed prompt (Esc) resolves to an error: no answer was given, so record nothing
            // and open no terminal — silently picking one would either write to a host the user did
            // not agree to, or bake in a refusal they never made.
            let Ok(choice) = prompt.await else {
                return;
            };
            let consented = choice == 0;
            let _ = cx.update(|window, cx| {
                let _ = app.update(cx, |app, cx| {
                    app.settings.update(cx, |settings, cx| {
                        settings.update(
                            |settings| {
                                settings
                                    .remote_install_consent
                                    .insert(destination.clone(), consented);
                            },
                            cx,
                        );
                    });
                    // Re-enter with the answer recorded: the consent gate now finds an entry and
                    // proceeds straight to creating the session.
                    app.new_terminal_with_profile(
                        group_id,
                        split,
                        source_item,
                        profile,
                        window,
                        cx,
                    );
                });
            });
        })
        .detach();
    }

    /// Create a terminal from an explicitly chosen `profile` (the command palette's
    /// "New Terminal with Profile…"), bypassing the project default.
    pub(super) fn new_terminal_with_profile(
        &mut self,
        group_id: GroupId,
        split: Option<Direction>,
        source_item: Option<ItemId>,
        profile: Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.workspace_state.active_project_data().clone();
        let scrollback_limit = self.settings.read(cx).config().scrollback_lines;
        // Launch fields come from the profile; scrollback stays global (it is a resource limit, not
        // part of a terminal's identity). The cwd resolution needs the same source-item lookup the
        // inheritance path uses, so compute it here and let the profile override it.
        let active_cwd = self.active_session_cwd(group_id, source_item);
        let spawn =
            resolve_spawn_config(&profile, active_cwd.as_deref(), &project.effective_root());
        // Spawn with the profile's own palette, so a themed terminal is born in its colors instead of
        // painting the global theme for the frame before the render reconcile catches up.
        let appearance = crate::settings::resolve_appearance(
            self.settings.read(cx).config(),
            (!profile.appearance.is_empty()).then_some(&profile.appearance),
            self.global_terminal_layer(),
            None,
        )
        .theme
        .appearance();

        // A remote profile whose host the user has not answered for yet needs a decision before the
        // session is created — the bootstrap writes into that host's home directory. Defer to the
        // async prompt path; it re-enters here once the answer is recorded.
        if let Some(target) = spawn.remote.as_ref()
            && !self
                .settings
                .read(cx)
                .config()
                .remote_install_consent
                .contains_key(&target.consent_key())
        {
            self.prompt_remote_install_consent(
                target.consent_key(),
                group_id,
                split,
                source_item,
                profile,
                window,
                cx,
            );
            return;
        }
        let remote_install_consented = spawn.remote.as_ref().is_some_and(|target| {
            self.settings
                .read(cx)
                .config()
                .remote_install_consent
                .get(&target.consent_key())
                == Some(&true)
        });

        let Ok(DaemonResponse::SessionCreated { session }) =
            self.client.request(ClientRequest::CreateSession {
                project_id: project.id,
                window_id: self.window_id,
                cwd: spawn.cwd,
                size: TerminalSize::default(),
                appearance,
                scrollback_limit,
                shell_program: spawn.shell_program,
                shell_args: spawn.shell_args,
                shell_features: spawn.shell_features,
                env: spawn.env,
                remote: spawn.remote,
                remote_cwd: spawn.remote_cwd,
                remote_install_consented,
            })
        else {
            return;
        };
        // Record what the daemon now has, so the render reconcile does not immediately re-send it.
        self.session_appearances.insert(session.id, appearance);
        let item = TabItem::terminal_with_profile(session.id, session.title.clone(), profile.id);
        let workspace = self.workspace_state.workspace_mut();
        if let Some(direction) = split {
            if let Some(new_group) = workspace
                .layout
                .split_group_direction(group_id, direction, item)
            {
                workspace.active_group_id = new_group;
            }
        } else if let Some(group) = workspace.layout.find_group_mut(group_id) {
            group.add_item(item);
            workspace.active_group_id = group_id;
        }
        self.configure_session_progress(session.id);
        self.configure_session_osc_policy(session.id);
        self.session_feed.sessions.push(session);
        self.ensure_snapshot_watchers(cx);
        self.ensure_progress_watchers(cx);
        self.ensure_osc_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    pub(super) fn activate_item(
        &mut self,
        group_id: GroupId,
        item_id: eggie_domain::ItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace_state.workspace_mut();
        if let Some(group) = workspace.layout.find_group_mut(group_id) {
            group.active_item_id = Some(item_id);
            workspace.active_group_id = group_id;
            self.focus_handle.focus(window, cx);
            self.terminal_has_focus = true;
            // The search bar is bound to a specific session; if the newly active session differs,
            // drop it so stale state and highlights don't linger (its doc contract: switching
            // sessions resets it).
            let active_session_id = self.active_session_id();
            if self
                .terminal_search
                .as_ref()
                .is_some_and(|search| Some(search.session_id) != active_session_id)
            {
                self.terminal_search = None;
            }
            self.ensure_snapshot_watchers(cx);
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    pub(super) fn focus_group(
        &mut self,
        group_id: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspace_state
            .workspace()
            .layout
            .find_group(group_id)
            .is_some()
        {
            self.workspace_state.workspace_mut().active_group_id = group_id;
            self.focus_handle.focus(window, cx);
            self.terminal_has_focus = true;
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    pub(super) fn activate_session(
        &mut self,
        project_id: ProjectId,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((group_id, item_id)) = self
            .workspace_state
            .project_views
            .get(&project_id)
            .and_then(|view| view.layout.find_terminal_item(session_id))
        else {
            return;
        };
        self.workspace_state.active_project = project_id;
        self.activate_item(group_id, item_id, window, cx);
    }

    pub(super) fn activate_session_by_id(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project_id) = self
            .workspace_state
            .project_views
            .iter()
            .find(|(_, view)| view.layout.find_terminal_item(session_id).is_some())
            .map(|(id, _)| *id)
        {
            self.activate_session(project_id, session_id, window, cx);
        }
    }

    /// The profile a live session was launched from, if its tab recorded one and that profile still
    /// exists. `None` means "no overrides" — the terminal follows the global appearance.
    ///
    /// Adopted sessions (restored from the daemon, or claimed from another window) legitimately have no
    /// recorded profile, and a profile deleted while its terminals run resolves to `None` rather than
    /// keeping a dangling override.
    ///
    /// Returns the profile id alongside the overrides because resolving also needs to ask whether the
    /// settings window's hover preview targets *this* profile.
    pub(super) fn session_profile_appearance(
        &self,
        session_id: SessionId,
        cx: &App,
    ) -> Option<(eggie_domain::ProfileId, eggie_domain::ProfileAppearance)> {
        let profile_id = self
            .workspace_state
            .project_views
            .values()
            .find_map(|view| {
                view.layout
                    .terminal_items()
                    .find(|item| item.session_id == Some(session_id))
            })
            .and_then(|item| item.profile_id)?;
        // A deleted profile yields no overrides, but the id is still the right preview target: the
        // settings window cannot have it selected, so the preview simply will not match.
        let appearance = self
            .profile_store
            .read(cx)
            .appearance_override(profile_id)
            .unwrap_or_default();
        Some((profile_id, appearance))
    }

    /// The global terminal layer plus the dark/light decision, as the profile merge needs them.
    pub(super) fn global_terminal_layer(&self) -> crate::settings::GlobalTerminalLayer {
        crate::settings::GlobalTerminalLayer {
            theme: self.terminal_theme,
            use_dark: self.use_dark,
        }
    }

    /// A terminal's appearance after merging its profile's overrides over the global layer.
    pub(super) fn resolved_session_appearance(
        &self,
        session_id: SessionId,
        cx: &App,
    ) -> crate::settings::ResolvedAppearance {
        let profile = self.session_profile_appearance(session_id, cx);
        // The hover preview applies only while the settings window is previewing *this* session's
        // profile, so a preview never leaks into terminals from other profiles.
        let preview = profile
            .as_ref()
            .and_then(|(profile_id, _)| self.settings.read(cx).profile_theme_preview(*profile_id));
        crate::settings::resolve_appearance(
            self.settings.read(cx).config(),
            profile
                .as_ref()
                .map(|(_, appearance)| appearance)
                .filter(|appearance| !appearance.is_empty()),
            self.global_terminal_layer(),
            preview,
        )
    }

    /// Send every session whose resolved palette changed a fresh `SetAppearance`, and forget the ones
    /// this window no longer owns.
    ///
    /// A failed request is deliberately *not* recorded, so the next reconcile retries it — a terminal
    /// left painting stale colors is worse than one redundant request. Since the reconcile is gated on
    /// its inputs changing, a persistently failing daemon does not spin: the retry happens on the next
    /// input change, not on the next frame.
    pub(super) fn reconcile_session_appearances(&mut self, cx: &App) {
        for session_id in self.window_session_ids() {
            let appearance = self
                .resolved_session_appearance(session_id, cx)
                .theme
                .appearance();
            if self.session_appearances.get(&session_id) == Some(&appearance) {
                continue;
            }
            match self.client.request(ClientRequest::SetAppearance {
                session_id,
                appearance,
            }) {
                Ok(_) => {
                    self.session_appearances.insert(session_id, appearance);
                }
                Err(error) => eprintln!("failed to update terminal appearance: {error:#}"),
            }
        }
        // Forget sessions this window no longer owns, so the map cannot grow without bound across a
        // long-lived window's tab churn.
        if self.session_appearances.len() > self.session_feed.sessions.len() {
            let live: HashSet<SessionId> = self
                .session_feed
                .sessions
                .iter()
                .map(|session| session.id)
                .collect();
            self.session_appearances
                .retain(|session_id, _| live.contains(session_id));
        }
    }

    pub(super) fn close_item(
        &mut self,
        group_id: GroupId,
        item_id: eggie_domain::ItemId,
        cx: &mut Context<Self>,
    ) {
        let removed = self
            .workspace_state
            .workspace_mut()
            .layout
            .close_item(group_id, item_id);
        self.sidebar
            .marquee_states
            .borrow_mut()
            .remove(&format!("tab-title-{item_id}"));
        let workspace = self.workspace_state.workspace_mut();
        if workspace.layout.find_group(group_id).is_none() {
            workspace.active_group_id = workspace.layout.first_group_id();
        }
        if let Some(session_id) = removed.and_then(|item| item.session_id) {
            if self
                .sidebar
                .sidebar_preview
                .as_ref()
                .map(|preview| preview.session_id)
                == Some(session_id)
            {
                self.dismiss_sidebar_preview(cx);
            }
            self.sidebar
                .sidebar_row_bounds
                .borrow_mut()
                .remove(&session_id);
            self.sidebar
                .marquee_states
                .borrow_mut()
                .remove(&format!("sidebar-title-{session_id}"));
            self.sidebar
                .marquee_states
                .borrow_mut()
                .remove(&format!("sidebar-subtitle-{session_id}"));
            if self.terminal_focused_session == Some(session_id) {
                self.terminal_focused_session = None;
            }
            let _ = self.client.request(ClientRequest::Terminate { session_id });
            self.session_feed
                .sessions
                .retain(|session| session.id != session_id);
            self.session_feed.snapshots.remove(&session_id);
            self.terminal_sizes.remove(&session_id);
            self.terminal_resize_in_flight.remove(&session_id);
            self.terminal_ime_states.remove(&session_id);
            if self
                .terminal_hovered_link
                .as_ref()
                .is_some_and(|(hovered_session, _)| *hovered_session == session_id)
            {
                self.terminal_hovered_link = None;
            }
            self.session_feed.session_inspections.remove(&session_id);
            self.session_feed
                .session_inspection_errors
                .remove(&session_id);
            self.session_feed.shell_integration.remove(&session_id);
            // Drop any un-actioned system-notification routes owned by this session: the session is
            // gone, so their "user clicked the notification" callback can never fire to remove them.
            self.notification_routes
                .borrow_mut()
                .retain(|_, route| route.session_id != session_id);
        }
        // The closed item may have collapsed a group or split out of the layout; reap their caches.
        self.reconcile_layout_caches();
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    pub(super) fn new_tab_in_active_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let group_id = self.workspace_state.workspace().active_group_id;
        self.new_terminal(group_id, None, None, window, cx);
        true
    }

    pub(super) fn split_active_group(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let group_id = self.workspace_state.workspace().active_group_id;
        self.new_terminal(group_id, Some(direction), None, window, cx);
        true
    }

    pub(super) fn close_active_tab(&mut self, cx: &mut Context<Self>) -> bool {
        let group_id = self.workspace_state.workspace().active_group_id;
        let Some(item_id) = self
            .workspace_state
            .workspace()
            .layout
            .find_group(group_id)
            .and_then(|group| group.active_item_id)
        else {
            return false;
        };
        self.close_item(group_id, item_id, cx);
        true
    }

    pub(super) fn activate_relative_tab(
        &mut self,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let group_id = self.workspace_state.workspace().active_group_id;
        let Some(group) = self.workspace_state.workspace().layout.find_group(group_id) else {
            return false;
        };
        let count = group.items.len();
        if count < 2 {
            return false;
        }
        let Some(current) = group
            .active_item_id
            .and_then(|active| group.items.iter().position(|item| item.id == active))
        else {
            return false;
        };
        let next_index = (current as isize + delta).rem_euclid(count as isize) as usize;
        let item_id = group.items[next_index].id;
        self.activate_item(group_id, item_id, window, cx);
        true
    }

    pub(super) fn clear_active_terminal(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        // Send Ctrl-L (form feed); the shell redraws its prompt on a cleared screen.
        self.enqueue_terminal_input(session_id, vec![0x0c])
    }

    pub(super) fn scroll_active_terminal(
        &mut self,
        command: TerminalScrollCommand,
        _cx: &mut Context<Self>,
    ) -> bool {
        let Some(session_id) = self.active_session_id() else {
            return false;
        };
        self.client
            .request(ClientRequest::TerminalScrollTo {
                session_id,
                command,
            })
            .is_ok()
    }

    pub(super) fn adjust_font_size(&mut self, delta: f32, cx: &mut Context<Self>) -> bool {
        self.settings.update(cx, |settings, cx| {
            settings.update(
                |settings| settings.font_size = (settings.font_size + delta).round(),
                cx,
            )
        });
        true
    }

    pub(super) fn reset_font_size(&mut self, cx: &mut Context<Self>) -> bool {
        self.settings.update(cx, |settings, cx| {
            settings.update(|settings| settings.font_size = DEFAULT_FONT_SIZE, cx)
        });
        true
    }

    /// Open the in-terminal search bar for the active session (⌘F), or refocus it if already open.
    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // When the search input or command palette is focused it owns keyboard input; never leak
        // keys into the terminal. The input's own handlers consume the keys; this guard covers the
        // case where the event bubbles up to the workspace root before the input consumes it.
        if self
            .terminal_search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).is_focused(window))
            || self
                .command_palette
                .as_ref()
                .is_some_and(|palette| palette.input.read(cx).is_focused(window))
        {
            return;
        }
        if is_character_palette_shortcut(event) && self.active_session_id().is_some() {
            window.show_character_palette();
            cx.stop_propagation();
            return;
        }
        let Some(session_id) = self.active_session_id() else {
            return;
        };
        let keyboard_flags = self
            .session_feed
            .snapshots
            .get(&session_id)
            .map_or(0, |snapshot| snapshot.input_modes.kitty_keyboard_flags);
        let Some(bytes) = terminal_bytes_with_mode(event, keyboard_flags) else {
            return;
        };
        self.enqueue_terminal_input(session_id, bytes);
        let _ = self.input_sender.send_selection_clear(session_id);
        cx.stop_propagation();
    }

    pub(super) fn key_up(
        &mut self,
        event: &KeyUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Mirror the key_down guard: while the search input or command palette is focused it owns
        // keyboard input, so key releases must not leak into the terminal (e.g. as kitty key-release
        // escape sequences).
        if self
            .terminal_search
            .as_ref()
            .is_some_and(|search| search.input.read(cx).is_focused(window))
            || self
                .command_palette
                .as_ref()
                .is_some_and(|palette| palette.input.read(cx).is_focused(window))
        {
            return;
        }
        let Some(session_id) = self.active_session_id() else {
            return;
        };
        let keyboard_flags = self
            .session_feed
            .snapshots
            .get(&session_id)
            .map_or(0, |snapshot| snapshot.input_modes.kitty_keyboard_flags);
        if keyboard_flags & KITTY_REPORT_EVENT_TYPES == 0 {
            return;
        }
        let Some(bytes) = kitty_key_bytes(&event.keystroke, keyboard_flags, 3) else {
            return;
        };
        self.enqueue_terminal_input(session_id, bytes);
        cx.stop_propagation();
    }
}

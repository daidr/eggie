use super::*;

impl EggieApp {
    pub fn launch(project_root: PathBuf, client: DaemonClient) {
        let settings_store = match (
            client.request(ClientRequest::GetConfig),
            client.request(ClientRequest::GetPreferences),
        ) {
            (
                Ok(DaemonResponse::Config { snapshot }),
                Ok(DaemonResponse::Preferences {
                    snapshot: preferences,
                }),
            ) => SettingsStore::load_with_daemon(snapshot, preferences, client.clone()),
            (config, preferences) => {
                eprintln!("failed to load Eggie config/preferences: {config:?}, {preferences:?}");
                SettingsStore::load()
            }
        };
        // Profiles carry the launch fields that used to live in the global settings, so the store is
        // built from those values: on first run they migrate into the built-in Default profile, and
        // once `profiles.json` exists they are ignored.
        let profile_store = ProfileStore::load(settings_store.config().legacy_shell_settings());
        let project_store = ProjectStore::load();
        let snippet_store = SnippetStore::load();
        let notes_store = NotesStore::load();

        gpui_platform::application().run(move |cx: &mut App| {
            cx.set_app_identity("me.daidr.eggie", "Eggie");
            let notification_routes: NotificationRoutes = Rc::new(RefCell::new(HashMap::new()));
            let response_routes = notification_routes.clone();
            cx.on_system_notification_response(move |response, cx| {
                let tag = response.tag.to_string();
                let Some(route) = response_routes.borrow().get(&tag).cloned() else {
                    return;
                };
                let activated = response.action_id.as_deref() != Some("close");
                let _ = route.client.request(ClientRequest::NotificationResponse {
                    session_id: route.session_id,
                    notification_id: route.notification_id.clone(),
                    activated,
                });
                cx.dismiss_system_notification(&tag);
                response_routes.borrow_mut().remove(&tag);
                if activated {
                    cx.activate(true);
                    let _ = route.app.update_in(cx, |app, window, cx| {
                        window.activate_window();
                        app.activate_session_by_id(route.session_id, window, cx);
                    });
                }
            });
            eggie_ui_kit::init(cx);
            let settings = cx.new(|_| settings_store);
            let project_store = cx.new(|_| project_store);
            let profile_store = cx.new(|_| profile_store);
            let snippet_store = cx.new(|_| snippet_store);
            let notes_store = cx.new(|_| notes_store);
            let updates = cx.new(|_| crate::update_controller::UpdateController::new());
            crate::actions::install(
                settings.clone(),
                project_store.clone(),
                profile_store.clone(),
                snippet_store.clone(),
                notes_store.clone(),
                client.clone(),
                notification_routes.clone(),
                updates.clone(),
                cx,
            );
            Self::spawn_config_watcher(cx, client.clone(), settings.clone());
            Self::open_new_window(
                cx,
                client.clone(),
                settings.clone(),
                updates.clone(),
                project_store.clone(),
                profile_store.clone(),
                snippet_store.clone(),
                notes_store.clone(),
                notification_routes.clone(),
                NewWindowInit::ColdStart {
                    project_root: project_root.clone(),
                },
            );
            // Single-instance listener: bind the GUI wake socket so subsequent `eggie` invocations
            // open a window here instead of starting a second process.
            Self::spawn_wake_listener(
                cx,
                client.clone(),
                settings.clone(),
                updates.clone(),
                project_store.clone(),
                profile_store.clone(),
                snippet_store.clone(),
                notes_store.clone(),
                notification_routes.clone(),
            );
            // Automatic update check, shortly after launch (staggered to avoid the startup
            // scramble). Silent: failures stay in the log, successes light up the sidebar.
            {
                let updates = updates.clone();
                let settings = settings.clone();
                let executor = cx.background_executor().clone();
                cx.spawn(async move |cx| {
                    executor.timer(Duration::from_secs(3)).await;
                    let _ = cx.update(|cx| {
                        let config = settings.read(cx).config();
                        if config.auto_check_updates {
                            let channel = config.update_channel.slug();
                            updates
                                .update(cx, |controller, cx| controller.check(true, channel, cx));
                        }
                    });
                })
                .detach();
            }
            // D9: quitting when the last main window closes. The daemon and its sessions live on
            // independently; the next `eggie` invocation reopens a window and reclaims them. The
            // settings window alone must not keep the process alive, so only count `EggieApp` windows.
            cx.on_window_closed(|cx, _closed_window_id| {
                let has_main_window = cx
                    .windows()
                    .into_iter()
                    .any(|window| window.downcast::<EggieApp>().is_some());
                if !has_main_window {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
    }

    pub(super) fn spawn_config_watcher(
        cx: &mut App,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
    ) {
        let executor = cx.background_executor().clone();
        let mut revision = settings.read(cx).lua_config().status_revision;
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            loop {
                let request_client = client.clone();
                let result = executor
                    .spawn(async move {
                        request_client.request(ClientRequest::WaitForConfig {
                            after_revision: revision,
                            timeout_ms: 30_000,
                        })
                    })
                    .await;
                match result {
                    Ok(DaemonResponse::Config { snapshot }) => {
                        revision = snapshot.status_revision;
                        cx.update(|cx| {
                            settings.update(cx, |store, cx| store.set_lua_snapshot(snapshot, cx));
                        });
                    }
                    Ok(DaemonResponse::ConfigUnchanged { .. }) => {}
                    Ok(response) => eprintln!("unexpected Lua config watch response: {response:?}"),
                    Err(error) => {
                        eprintln!("Lua config watch failed: {error:#}");
                        executor.timer(Duration::from_secs(2)).await;
                    }
                }
            }
        })
        .detach();
    }

    /// Bind the GUI single-instance socket and serve wake requests. A blocking `accept` loop runs on
    /// a dedicated std thread (the socket API is blocking); each decoded [`GuiWakeMessage`] is pushed
    /// onto a channel that a foreground task drains, so window creation happens on the main thread.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_wake_listener(
        cx: &mut App,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        project_store: Entity<ProjectStore>,
        profile_store: Entity<ProfileStore>,
        snippet_store: Entity<SnippetStore>,
        notes_store: Entity<NotesStore>,
        notification_routes: NotificationRoutes,
    ) {
        crate::gui_ipc::reap_stale_gui_sockets();
        let listener = match crate::gui_ipc::bind_listener() {
            Ok(listener) => listener,
            Err(error) => {
                // Losing the bind race just means another instance is the primary; degrade to a
                // standalone window rather than failing.
                eprintln!("single-instance listener unavailable: {error:#}");
                return;
            }
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<PathBuf>();
        std::thread::Builder::new()
            .name("eggie-gui-wake".to_owned())
            .spawn(move || {
                crate::gui_ipc::serve(listener, |message| match message {
                    crate::gui_ipc::GuiWakeMessage::OpenWindow { cwd } => {
                        let _ = tx.unbounded_send(cwd);
                    }
                });
            })
            .expect("failed to start GUI wake listener thread");

        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            use futures::StreamExt;
            while let Some(cwd) = rx.next().await {
                let client = client.clone();
                let settings = settings.clone();
                let updates = updates.clone();
                let project_store = project_store.clone();
                let profile_store = profile_store.clone();
                let snippet_store = snippet_store.clone();
                let notes_store = notes_store.clone();
                let notification_routes = notification_routes.clone();
                cx.update(|cx| {
                    // D4: reuse a project with the same root, otherwise add one, then open a window
                    // focused on it.
                    let (project_id, _created) =
                        project_store.update(cx, |store, cx| store.upsert_by_root(cwd.clone(), cx));
                    Self::open_new_window(
                        cx,
                        client,
                        settings,
                        updates,
                        project_store.clone(),
                        profile_store,
                        snippet_store,
                        notes_store,
                        notification_routes,
                        NewWindowInit::OpenDir { project_id },
                    );
                    cx.activate(true);
                });
            }
        })
        .detach();
    }

    /// Open a new main window on an already-running app. Called for the first (cold-start) window
    /// and for every subsequent `New Window` / single-instance wake. All windows share the same
    /// `settings`, `project_store` and `profile_store` entities, so their lists stay in sync.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_new_window(
        cx: &mut App,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        project_store: Entity<ProjectStore>,
        profile_store: Entity<ProfileStore>,
        snippet_store: Entity<SnippetStore>,
        notes_store: Entity<NotesStore>,
        notification_routes: NotificationRoutes,
        init: NewWindowInit,
    ) {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: None,
                    appears_transparent: true,
                    traffic_light_position: Some(point(
                        px(TRAFFIC_LIGHT_INSET),
                        px(TRAFFIC_LIGHT_INSET),
                    )),
                }),
                app_owns_titlebar_drag: true,
                window_min_size: Some(size(px(760.), px(480.))),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    Self::new(
                        WindowId::new_v4(),
                        client,
                        settings,
                        updates,
                        project_store,
                        profile_store,
                        snippet_store,
                        notes_store,
                        notification_routes,
                        init,
                        window,
                        cx,
                    )
                })
            },
        )
        .expect("failed to open Eggie window");
    }

    /// Resolve a [`NewWindowInit`] into this window's `(active_project, owned_sessions)`, mutating the
    /// shared [`ProjectStore`] as needed. Runs before the `EggieApp` struct exists, so it takes the
    /// pieces it needs explicitly and talks to the daemon directly.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn seed_window(
        init: &NewWindowInit,
        window_id: WindowId,
        client: &DaemonClient,
        project_store: &Entity<ProjectStore>,
        profile_store: &Entity<ProfileStore>,
        config: &crate::settings::AppSettings,
        global: crate::settings::GlobalTerminalLayer,
        cx: &mut Context<Self>,
    ) -> (ProjectId, Vec<SeededSession>, Option<WorkspaceRestoreState>) {
        match init {
            NewWindowInit::ColdStart { project_root } => Self::seed_cold_start(
                project_root,
                window_id,
                client,
                project_store,
                profile_store,
                config,
                global,
                cx,
            ),
            NewWindowInit::OpenDir { project_id } => {
                // The project already exists in the shared store (added by the waking path). Open one
                // terminal in it, since `eggie <dir>` is an explicit request for a terminal there.
                let (root, profile) =
                    Self::project_root_and_profile(*project_id, project_store, profile_store, cx);
                let sessions = Self::create_session(
                    client,
                    *project_id,
                    window_id,
                    root,
                    &profile,
                    config,
                    global,
                )
                .map(|summary| SeededSession::launched(summary, profile.id))
                .into_iter()
                .collect();
                (*project_id, sessions, None)
            }
            NewWindowInit::Empty => {
                // New Window (Cmd+N): activate the first shared project, show it empty (D7).
                let active = project_store
                    .read(cx)
                    .projects()
                    .first()
                    .map(|project| project.id);
                match active {
                    Some(id) => (id, Vec::new(), None),
                    None => {
                        // No projects at all (should not happen once Home exists); create one.
                        let project = Project::new("Home".to_owned());
                        let id = project.id;
                        project_store.update(cx, |store, cx| store.add(project, cx));
                        (id, Vec::new(), None)
                    }
                }
            }
        }
    }

    /// A project's effective root and the profile its new terminals launch from. Used by the seeding
    /// paths, which run before the `EggieApp` struct exists and so cannot use the instance methods.
    /// An unknown project id falls back to the home directory and the built-in Default profile.
    pub(super) fn project_root_and_profile(
        project_id: ProjectId,
        project_store: &Entity<ProjectStore>,
        profile_store: &Entity<ProfileStore>,
        cx: &mut Context<Self>,
    ) -> (PathBuf, Profile) {
        let project = project_store
            .read(cx)
            .projects()
            .iter()
            .find(|project| project.id == project_id)
            .cloned();
        let root = project
            .as_ref()
            .map(Project::effective_root)
            .unwrap_or_else(fallback_home_dir);
        let profile = profile_store
            .read(cx)
            .resolve(project.and_then(|project| project.default_profile_id))
            .clone();
        (root, profile)
    }

    /// Cold-start seeding: reconcile the persisted project list with the daemon's surviving sessions.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn seed_cold_start(
        project_root: &Path,
        window_id: WindowId,
        client: &DaemonClient,
        project_store: &Entity<ProjectStore>,
        profile_store: &Entity<ProfileStore>,
        config: &crate::settings::AppSettings,
        global: crate::settings::GlobalTerminalLayer,
        cx: &mut Context<Self>,
    ) -> (ProjectId, Vec<SeededSession>, Option<WorkspaceRestoreState>) {
        // Adopt a project for the launch directory: reuse a persisted project with the same root,
        // otherwise create one. Done before restore so `eggie <dir>` lands in the store even when
        // an unrelated workspace is restored.
        let (launch_project, project_root_buf) = project_store.update(cx, |store, cx| {
            let (id, _) = store.upsert_by_root(project_root.to_path_buf(), cx);
            (id, project_root.to_path_buf())
        });

        // Restore the persisted workspace first: claim each session the saved layouts reference by
        // id (the daemon re-owns unconditionally; dead sessions fail and are pruned). When at least
        // one session survives, rebuild the old tabs/splits around it.
        if let Some(mut restored) = WorkspaceRestoreState::load() {
            // Drop views for projects that no longer exist BEFORE claiming, so their sessions aren't
            // claimed into this window with no tab to hold them (orphan sessions the user can't see).
            let known_projects: HashSet<ProjectId> = project_store
                .read(cx)
                .projects()
                .iter()
                .map(|project| project.id)
                .collect();
            restored
                .views
                .retain(|project_id, _| known_projects.contains(project_id));

            let mut claimed = Vec::new();
            for session_id in restored.persisted_session_ids() {
                if let Ok(DaemonResponse::SessionClaimed { session }) =
                    client.request(ClientRequest::ClaimSession {
                        session_id,
                        window_id,
                    })
                {
                    claimed.push(session);
                }
            }
            let live: HashSet<_> = claimed.iter().map(|session| session.id).collect();
            let projects = project_store.read(cx).projects().to_vec();
            if restored.prune_to_live(&live, &projects) {
                let claimed = claimed.into_iter().map(SeededSession::adopted).collect();
                return (restored.active_project, claimed, Some(restored));
            }
        }

        // No restorable workspace (or nothing survived in the daemon): claim every surviving
        // session whose project this window is adopting, and any that match the launch directory.
        let all_sessions = match client.request(ClientRequest::ListSessions) {
            Ok(DaemonResponse::Sessions { sessions }) => sessions,
            _ => Vec::new(),
        };
        let mut owned: Vec<SeededSession> = Vec::new();
        for session in all_sessions {
            let belongs = session.project_id == launch_project
                || session.initial_directory == project_root_buf;
            if !belongs {
                continue;
            }
            if let Ok(DaemonResponse::SessionClaimed { session }) =
                client.request(ClientRequest::ClaimSession {
                    session_id: session.id,
                    window_id,
                })
            {
                owned.push(SeededSession::adopted(session));
            }
        }

        // Nothing to adopt: open a fresh terminal so the window is not empty on first launch.
        if owned.is_empty() {
            // `launch_project` was just upserted from `project_root`, so the lookup all but always
            // hits; `project_root_buf` covers the impossible case without reaching for the home dir.
            let project = project_store
                .read(cx)
                .projects()
                .iter()
                .find(|project| project.id == launch_project)
                .cloned();
            let root = project
                .as_ref()
                .map(Project::effective_root)
                .unwrap_or(project_root_buf);
            let profile = profile_store
                .read(cx)
                .resolve(project.and_then(|project| project.default_profile_id))
                .clone();
            owned.extend(
                Self::create_session(
                    client,
                    launch_project,
                    window_id,
                    root,
                    &profile,
                    config,
                    global,
                )
                .map(|summary| SeededSession::launched(summary, profile.id)),
            );
        }
        (launch_project, owned, None)
    }

    /// Spawn a single terminal session in the daemon, returning its summary (or `None` on error).
    ///
    /// `profile` supplies the launch fields and, when it overrides the theme, the spawn palette;
    /// `cwd` is the already-resolved working directory (the cold-start path has no active terminal to
    /// inherit from, so its caller resolves the project root and a profile `cwd` still wins here via
    /// [`resolve_spawn_config`]). `global` is the already-resolved global terminal layer the profile's
    /// overrides merge over, plus the dark/light decision selecting which half of them applies.
    pub(super) fn create_session(
        client: &DaemonClient,
        project_id: ProjectId,
        window_id: WindowId,
        cwd: PathBuf,
        profile: &Profile,
        config: &crate::settings::AppSettings,
        global: crate::settings::GlobalTerminalLayer,
    ) -> Option<SessionSummary> {
        let spawn = resolve_spawn_config(profile, None, &cwd);
        // Born in the profile's own colors when it overrides the theme, the global palette otherwise.
        // No hover preview here: this runs at spawn time, outside any settings-window interaction.
        let appearance = crate::settings::resolve_appearance(
            config,
            (!profile.appearance.is_empty()).then_some(&profile.appearance),
            global,
            None,
        )
        .theme
        .appearance();
        match client.request(ClientRequest::CreateSession {
            project_id,
            window_id,
            cwd: spawn.cwd,
            size: TerminalSize::default(),
            appearance,
            scrollback_limit: config.scrollback_lines,
            shell_program: spawn.shell_program,
            shell_args: spawn.shell_args,
            shell_features: spawn.shell_features,
            env: spawn.env,
            // Seeding runs before any window exists, so it cannot put a consent prompt on screen.
            // A host the user already answered for is honored; an unanswered one connects without
            // installing, and the next terminal opened from a live window does the asking.
            remote_install_consented: spawn.remote.as_ref().is_some_and(|target| {
                config.remote_install_consent.get(&target.consent_key()) == Some(&true)
            }),
            remote: spawn.remote,
            remote_cwd: spawn.remote_cwd,
        }) {
            Ok(DaemonResponse::SessionCreated { session }) => Some(session),
            other => {
                eprintln!("failed to create terminal session: {other:?}");
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        window_id: WindowId,
        client: DaemonClient,
        settings: Entity<SettingsStore>,
        updates: Entity<crate::update_controller::UpdateController>,
        project_store: Entity<ProjectStore>,
        profile_store: Entity<ProfileStore>,
        snippet_store: Entity<SnippetStore>,
        notes_store: Entity<NotesStore>,
        notification_routes: NotificationRoutes,
        init: NewWindowInit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = settings.read(cx).config().clone();
        let system_is_dark = is_dark_appearance(window.appearance());
        let interface_theme = config.interface_theme(system_is_dark);
        let terminal_theme = config.terminal_theme(system_is_dark);
        let global_appearance = terminal_theme.appearance();
        let progress_timeouts = TerminalProgressTimeouts {
            completed_ms: config.progress_complete_timeout_secs.saturating_mul(1_000),
            stale_ms: config.progress_stale_timeout_secs.saturating_mul(1_000),
        };
        let colors = UiColors::from_theme(interface_theme);

        // Resolve the requested init into this window's active project and owned sessions, mutating
        // the shared project store as needed (seeding Home / adopting the launch directory). A cold
        // start additionally returns the restored workspace layout when one survived.
        let (active_project, seeded, restored) = Self::seed_window(
            &init,
            window_id,
            &client,
            &project_store,
            &profile_store,
            &config,
            crate::settings::GlobalTerminalLayer::new(&config, system_is_dark),
            cx,
        );
        // Split the seeding result: `sessions` is what the rest of construction expects, while
        // `seeded_profiles` records which of them this window just launched (and from which profile),
        // so their tabs can be built with that profile rather than defaulting to "no overrides".
        let seeded_profiles: HashMap<SessionId, eggie_domain::ProfileId> = seeded
            .iter()
            .filter_map(|seeded| seeded.profile.map(|profile| (seeded.summary.id, profile)))
            .collect();
        let sessions: Vec<SessionSummary> =
            seeded.into_iter().map(|seeded| seeded.summary).collect();

        // Build a per-window view for every shared project (empty by default). With a restored
        // workspace, adopt the saved layouts (dead tabs already pruned by `seed_cold_start`);
        // otherwise place this window's sessions into their project's layout as fresh tabs.
        let projects = project_store.read(cx).projects().to_vec();
        let mut project_views: HashMap<ProjectId, WindowProjectView> = projects
            .iter()
            .map(|project| (project.id, WindowProjectView::empty()))
            .collect();
        let restored_sidebar = match restored {
            Some(restored) => {
                let sidebar = restored.sidebar_states();
                for (project_id, view) in restored.views {
                    project_views.insert(project_id, view);
                }
                // Persisted tab titles may be stale (the shell kept running); refresh from the
                // claimed summaries. The session poll would fix these too, but not before first paint.
                for session in &sessions {
                    for view in project_views.values_mut() {
                        view.layout
                            .update_terminal_title(session.id, &session.title);
                    }
                }
                sidebar
            }
            None => {
                for session in &sessions {
                    if let Some(view) = project_views.get_mut(&session.project_id) {
                        let group_id = view.layout.first_group_id();
                        if let Some(group) = view.layout.find_group_mut(group_id) {
                            // Carry the launching profile onto the tab, so the terminal renders its
                            // appearance overrides instead of silently falling back to the global layer.
                            group.add_item(match seeded_profiles.get(&session.id) {
                                Some(profile) => TabItem::terminal_with_profile(
                                    session.id,
                                    session.title.clone(),
                                    *profile,
                                ),
                                None => TabItem::terminal(session.id, session.title.clone()),
                            });
                        }
                        view.active_group_id = group_id;
                    }
                }
                SidebarRestoreState::default()
            }
        };

        // Appearance is deliberately NOT pushed here. Broadcasting the global palette would clobber the
        // profile-resolved palette a freshly launched session was spawned with, and for a claimed
        // session the daemon still holds the palette from the run that created it — which may predate a
        // theme change. `applied_appearance_inputs: None` makes the first render reconcile every session
        // to its own resolved palette, which is both correct by construction and the same one request
        // per session this loop used to send.
        for session in &sessions {
            if let Err(error) = client.request(ClientRequest::SetProgressTimeouts {
                session_id: session.id,
                timeouts: progress_timeouts,
            }) {
                eprintln!("failed to set terminal progress timeouts: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetOscPolicy {
                session_id: session.id,
                allow_clipboard_read: config.allow_osc_clipboard_read,
            }) {
                eprintln!("failed to set terminal OSC policy: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetUrlDetection {
                session_id: session.id,
                detect_urls: config.detect_urls,
            }) {
                eprintln!("failed to set terminal URL detection: {error:#}");
            }
            if let Err(error) = client.request(ClientRequest::SetCursorStyle {
                session_id: session.id,
                shape: config.cursor_shape.to_protocol(),
            }) {
                eprintln!("failed to set terminal cursor style: {error:#}");
            }
            // Pre-existing sessions adopted from the daemon were spawned with the built-in default;
            // align them to the configured scrollback (new sessions bake it in at CreateSession).
            if let Err(error) = client.request(ClientRequest::SetScrollbackLimit {
                session_id: session.id,
                limit: config.scrollback_lines,
            }) {
                eprintln!("failed to set terminal scrollback limit: {error:#}");
            }
        }
        let snapshots = sessions
            .iter()
            .filter_map(|session| {
                match client.request(ClientRequest::Snapshot {
                    session_id: session.id,
                }) {
                    Ok(DaemonResponse::Snapshot { snapshot }) => Some((session.id, snapshot)),
                    _ => None,
                }
            })
            .collect::<HashMap<_, _>>();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        // Update state changes (found/downloading/ready) re-render the sidebar indicator.
        cx.observe(&updates, |_, _, cx| cx.notify()).detach();
        // Cross-window project sync: when any window mutates the shared store, reconcile this
        // window's per-project views (add/drop) and re-render.
        cx.observe(&project_store, |app, _, cx| {
            app.reconcile_project_views(cx);
            cx.notify();
        })
        .detach();
        // Profile edits from any window re-render here (tab badges, profile pickers). Launch fields
        // only affect terminals created from now on; appearance overrides are re-resolved on render.
        cx.observe(&profile_store, |_, _, cx| cx.notify()).detach();
        // Snippet edits from any window re-render the Commands tab here.
        cx.observe(&snippet_store, |_, _, cx| cx.notify()).detach();
        // Notes edits from another window: reseed our editor so it mirrors the shared text (unless
        // this window is the one actively editing — guarded in the observer body).
        cx.observe(&notes_store, |app, store, cx| {
            let shared = store.read(cx).text().to_owned();
            if let Some(editor) = app.notes_editor.as_ref() {
                let local = editor.input.read(cx).content().to_owned();
                if local != shared {
                    editor
                        .input
                        .update(cx, |input, cx| input.set_content(shared, cx));
                }
            }
            cx.notify();
        })
        .detach();
        cx.observe_window_appearance(window, |_, _, cx| cx.notify())
            .detach();
        cx.observe_window_bounds(window, |app, _, cx| {
            app.dismiss_project_popover(cx);
            app.dismiss_sidebar_preview(cx);
        })
        .detach();
        cx.observe_window_activation(window, |app, window, cx| {
            if !window.is_window_active() {
                app.dismiss_project_popover(cx);
                app.dismiss_sidebar_preview(cx);
            }
        })
        .detach();
        let terminal_font_features: std::sync::Arc<[crate::settings::FontFeature]> =
            config.resolved_font_features().into();
        let terminal_font_variations: std::sync::Arc<[crate::settings::FontVariation]> =
            config.resolved_font_variations().into();
        let terminal_font_codepoint_map: std::sync::Arc<[crate::settings::CodepointMapEntry]> =
            config.resolved_codepoint_map().into();
        // The Notes tab's multi-line editor: created upfront (rendering is `&self` and cannot lazily
        // build a TextInput, which needs `window`). Seeds from the shared store; a subscription drives
        // debounced autosave.
        let notes_editor = {
            let text_style = TextInputStyle {
                text_color: (colors.text << 8) | 0xff,
                placeholder_color: (colors.muted << 8) | 0xff,
                cursor_color: (colors.accent << 8) | 0xff,
                selection_color: (colors.accent << 8) | 0x55,
            };
            let seed = notes_store.read(cx).text().to_owned();
            let language = config.language;
            let input = cx.new(|cx| {
                let mut input = TextInput::new(window, cx, text_style).multiline();
                input.set_placeholder(language.notes_placeholder());
                input.set_language(language);
                if !seed.is_empty() {
                    input.set_content(seed, cx);
                }
                input
            });
            let subscription = cx.subscribe_in(&input, window, Self::on_notes_input_event);
            let scroll_handle = ScrollHandle::new();
            // Let the textarea drive this same scroll handle for edge-drag auto-scroll.
            input.update(cx, |input, _| {
                input.set_scroll_handle(scroll_handle.clone())
            });
            NotesEditor {
                input,
                _subscription: subscription,
                save_epoch: 0,
                scroll_handle,
            }
        };
        let mut app = Self {
            workspace_state: WorkspaceState {
                projects,
                project_views,
                active_project,
                collapsed_projects: HashSet::new(),
            },
            sidebar: SidebarState {
                project_row_bounds: Rc::new(RefCell::new(HashMap::new())),
                project_drag: ProjectDragState::default(),
                project_popover: None,
                sidebar_row_bounds: Rc::new(RefCell::new(HashMap::new())),
                sidebar_preview: None,
                hovered_sidebar_session: None,
                sidebar_preview_revision: 0,
                marquee_states: Rc::new(RefCell::new(HashMap::new())),
                project_icon_states: RefCell::new(HashMap::new()),
                tab_icon_states: RefCell::new(HashMap::new()),
                sidebar_status_rows: RefCell::new(HashMap::new()),
            },
            session_feed: SessionFeedState {
                snapshot_watchers: HashSet::new(),
                progress_watchers: HashSet::new(),
                osc_watchers: HashSet::new(),
                progress_updates: HashMap::new(),
                pending_progress_updates: HashMap::new(),
                sessions,
                detached_sessions: Vec::new(),
                snapshots,
                session_inspections: HashMap::new(),
                session_inspection_errors: HashMap::new(),
                process_metrics_system: System::new(),
                shell_integration: HashMap::new(),
            },
            input_sender: client
                .input_sender()
                .expect("failed to start terminal input worker"),
            input_latency: InputLatencyTracker::from_environment(),
            client,
            settings,
            updates,
            colors,
            interface_theme,
            terminal_theme,
            use_dark: config.use_dark(system_is_dark),
            global_appearance,
            // Empty on purpose: what the daemon holds for a seeded session is not knowable here (a
            // launched one carries its profile's palette, a claimed one carries the palette of the run
            // that spawned it). Pre-filling with the global palette would make the reconcile's diff
            // report "no change" and leave those terminals on stale daemon-side colors, so instead the
            // first render pushes every session its resolved palette — one request each, correct by
            // construction rather than by coincidence.
            session_appearances: HashMap::new(),
            // `None` so the first render runs the reconcile once, catching any seeded session whose
            // profile overrides the theme.
            applied_appearance_inputs: None,
            terminal_padding_x: config.terminal_padding_x,
            terminal_padding_y: config.terminal_padding_y,
            terminal_minimum_contrast: config.minimum_contrast,
            terminal_metric_adjustments: config.font_metrics.resolve(),
            terminal_font_features,
            terminal_ligatures: config.ligatures,
            terminal_shaping_break_cursor: config.font_shaping_break_cursor,
            terminal_font_variations,
            terminal_font_thicken: config.font_thicken.then_some(config.font_thicken_strength),
            terminal_font_codepoint_map,
            // `None` so the first `render` applies the config-derived state once (pushing initial
            // per-session policy to the daemon), then caches it to gate subsequent frames.
            applied_config: None,
            terminal_renderer: MetalTerminalRenderer::default(),
            terminal_images_in_flight: HashSet::new(),
            terminal_frame_scheduled: false,
            progress_frame_scheduled: false,
            progress_animation_scheduled: false,
            progress_animation_epoch: Instant::now(),
            bell_flash: HashMap::new(),
            bell_flash_scheduled: false,
            cursor_blink_epoch: Instant::now(),
            cursor_blink_scheduled: false,
            progress_timeouts,
            window_id,
            project_store,
            profile_store,
            snippet_store,
            notes_store,
            detached_popover_open: false,
            detached_toggle_bounds: None,
            update_popover_open: false,
            update_toggle_bounds: None,
            open_with_dropdown: None,
            open_with_dropdown_bounds: HashMap::new(),
            terminal_sizes: HashMap::new(),
            terminal_resize_in_flight: HashMap::new(),
            terminal_viewports: HashMap::new(),
            terminal_selection_drag: None,
            terminal_search: None,
            command_palette: None,
            hyperlink_mouse_down: false,
            terminal_hovered_link: None,
            terminal_has_focus: false,
            terminal_focused_session: None,
            terminal_ime_states: HashMap::new(),
            right_tab: RightTab::Info,
            command_history_collapsed: false,
            snippets_collapsed: false,
            notes_editor: Some(notes_editor),
            left_sidebar_width: restored_sidebar.left_width,
            right_sidebar_width: restored_sidebar.right_width,
            left_sidebar_collapsed: restored_sidebar.left_collapsed,
            right_sidebar_collapsed: restored_sidebar.right_collapsed,
            resizing_sidebar: None,
            resizing_split: None,
            split_bounds: Rc::new(RefCell::new(HashMap::new())),
            dragging_scrollbar: None,
            scrollbar_bounds: Rc::new(RefCell::new(HashMap::new())),
            moving_window: false,
            tab_drop_target: None,
            right_sidebar_scroll_handle: ScrollHandle::new(),
            tab_bar_scroll_handles: RefCell::new(HashMap::new()),
            focus_handle: cx.focus_handle(),
            notification_routes,
            allow_osc_clipboard_read: config.allow_osc_clipboard_read,
            detect_urls: config.detect_urls,
            copy_on_select: config.copy_on_select,
            cursor_shape: config.cursor_shape,
            scrollback_limit: config.scrollback_lines,
            cursor_blink: config.cursor_blink,
            language: config.language,
            closing: false,
            poll_error: None,
        };
        let visible_snapshots = app
            .visible_active_session_ids()
            .into_iter()
            .filter_map(|session_id| app.session_feed.snapshots.get(&session_id).cloned())
            .collect::<Vec<_>>();
        for snapshot in visible_snapshots {
            app.schedule_terminal_images(&snapshot, cx);
        }
        app.start_polling(cx);
        app.start_inspection_polling(cx);
        app.start_shell_integration_polling(cx);
        app.start_latency_reporting(cx);
        app.install_close_confirmation(window, cx);
        // Persist the workspace on quit so the next cold start restores tabs/splits. Every window
        // registers; each saves its own state (last-writer-wins for the single restore snapshot).
        // A window that closed earlier leaves a dead entity behind — the weak update then no-ops.
        cx.on_app_quit(move |app, _| {
            app.persist_workspace();
            std::future::ready(())
        })
        .detach();
        cx.on_focus_in(&app.focus_handle, window, |app, _, cx| {
            app.set_terminal_has_focus(true);
            cx.notify();
        })
        .detach();
        cx.on_focus_out(&app.focus_handle, window, |app, _, _, cx| {
            app.set_terminal_has_focus(false);
            cx.notify();
        })
        .detach();
        app.focus_handle.focus(window, cx);
        app.set_terminal_has_focus(true);
        app
    }
}

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

        let application = gpui_platform::application();
        application.on_reopen(|cx| {
            let Some(global) = cx.try_global::<crate::workspace::SharedWindows>() else {
                return;
            };
            let manager = global.0.read(cx);
            let handle = manager
                .state
                .active_window
                .and_then(|id| manager.handles.get(&id).copied())
                .or_else(|| manager.handles.values().next().copied());
            if let Some(handle) = handle {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            } else {
                cx.dispatch_action(&crate::actions::NewWindow);
            }
        });
        application.run(move |cx: &mut App| {
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
                    let manager = WindowManager::global(cx);
                    let handle = manager
                        .read(cx)
                        .state
                        .locate(route.session_id)
                        .and_then(|location| {
                            manager.read(cx).handles.get(&location.window_id).copied()
                        })
                        .or_else(|| manager.read(cx).handles.values().next().copied());
                    if let Some(handle) = handle {
                        let _ = handle.update(cx, |app, window, cx| {
                            window.activate_window();
                            app.navigate_terminal(route.session_id, window, cx);
                        });
                    } else {
                        cx.dispatch_action(&crate::actions::NewWindow);
                        cx.defer(move |cx| {
                            if let Some(handle) = manager.read(cx).handles.values().next().copied()
                            {
                                let _ = handle.update(cx, |app, window, cx| {
                                    app.navigate_terminal(route.session_id, window, cx)
                                });
                            }
                        });
                    }
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
            let launch_project = project_store.update(cx, |store, cx| {
                store.upsert_by_root(project_root.clone(), cx).0
            });
            let windows = cx.new(|_| WindowManager::load());
            windows.update(cx, |manager, _| {
                manager.notification_routes = notification_routes.clone();
                manager.project_store = Some(project_store.clone());
            });
            cx.set_global(crate::workspace::SharedWindows(windows.clone()));
            let projects = project_store.read(cx).projects().to_vec();
            windows.update(cx, |manager, cx| manager.reconcile_projects(&projects, cx));
            if let Ok(DaemonResponse::Sessions { sessions }) =
                client.request(ClientRequest::ListSessions)
            {
                windows.update(cx, |manager, cx| manager.set_sessions(sessions, cx));
            }
            let restore = windows
                .read(cx)
                .state
                .windows
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let active_window = windows.read(cx).state.active_window;
            if restore.is_empty() {
                let init = if windows.read(cx).state.background.is_empty() {
                    NewWindowInit::OpenDir {
                        project_id: launch_project,
                    }
                } else {
                    NewWindowInit::Empty
                };
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
                    init,
                );
            } else {
                for state in restore {
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
                        NewWindowInit::Restore(state),
                    );
                }
                if let Some(id) = active_window
                    && let Some(handle) = windows.read(cx).handles.get(&id).copied()
                {
                    let _ = handle.update(cx, |_, window, _| window.activate_window());
                }
            }
            WindowManager::start_polling(&windows, client.clone(), cx);
            let menu_settings = settings.clone();
            let mut previous_menu = Vec::new();
            cx.observe(&windows, move |manager, cx| {
                let manager = manager.read(cx);
                let menu = manager
                    .state
                    .windows
                    .keys()
                    .map(|id| {
                        (
                            *id,
                            manager.window_label(*id),
                            manager.state.active_window == Some(*id),
                        )
                    })
                    .collect::<Vec<_>>();
                if menu != previous_menu {
                    previous_menu = menu;
                    crate::actions::rebuild_menus(menu_settings.read(cx).config().language, cx);
                }
            })
            .detach();
            crate::actions::rebuild_menus(settings.read(cx).config().language, cx);
            let projects_for_windows = project_store.clone();
            cx.observe(&project_store, move |store, cx| {
                let projects = store.read(cx).projects().to_vec();
                WindowManager::global(cx)
                    .update(cx, |manager, cx| manager.reconcile_projects(&projects, cx));
            })
            .detach();
            let _ = projects_for_windows;
            let windows_for_quit = windows.clone();
            cx.on_app_quit(move |cx| {
                windows_for_quit.update(cx, |manager, _| {
                    manager.persist();
                    manager.quitting = true;
                });
                std::future::ready(())
            })
            .detach();
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
            cx.on_window_closed(|cx, native_id| {
                let manager = WindowManager::global(cx);
                let id = manager
                    .read(cx)
                    .handles
                    .iter()
                    .find(|(_, handle)| handle.window_id() == native_id)
                    .map(|(id, _)| *id);
                if let Some(id) = id {
                    manager.update(cx, |manager, cx| manager.close(id, cx));
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
                    let manager = WindowManager::global(cx);
                    let existing = manager
                        .read(cx)
                        .state
                        .windows
                        .values()
                        .find(|w| {
                            w.active_project == project_id
                                || w.views
                                    .get(&project_id)
                                    .is_some_and(|v| v.layout.item_count() > 0)
                        })
                        .and_then(|w| manager.read(cx).handles.get(&w.id).copied());
                    if let Some(handle) = existing {
                        let _ = handle.update(cx, |app, window, cx| {
                            app.activate_project(project_id, cx);
                            window.activate_window();
                        });
                        cx.activate(true);
                        return;
                    }
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

    /// Open one presentation window; the registry remains the only owner of placement state.
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
    ) -> Option<gpui::WindowHandle<Self>> {
        let manager = WindowManager::global(cx);
        let preferred = manager
            .read(cx)
            .state
            .active_window
            .and_then(|id| manager.read(cx).state.windows.get(&id))
            .map(|w| w.active_project)
            .or_else(|| project_store.read(cx).projects().first().map(|p| p.id))?;
        let (id, saved_frame) = match &init {
            NewWindowInit::Restore(state) => (state.id, state.frame),
            NewWindowInit::EmptyAt(pointer) => (
                WindowId::new_v4(),
                Some(eggie_domain::WindowFrame {
                    x: f32::from(pointer.x) - 100.,
                    y: f32::from(pointer.y) - 24.,
                    width: 1280.,
                    height: 800.,
                }),
            ),
            _ => (WindowId::new_v4(), None),
        };
        let bounds = saved_frame
            .filter(|f| {
                [f.x, f.y, f.width, f.height].iter().all(|v| v.is_finite())
                    && f.width > 0.
                    && f.height > 0.
            })
            .map(|f| {
                let saved = Bounds::new(point(px(f.x), px(f.y)), size(px(f.width), px(f.height)));
                let display = cx
                    .displays()
                    .into_iter()
                    .find(|display| display.visible_bounds().intersects(&saved))
                    .or_else(|| cx.primary_display());
                let Some(display) = display else {
                    return Bounds::centered(None, size(px(1280.), px(800.)), cx);
                };
                let visible = display.visible_bounds();
                let width = px(f.width.max(760.)).min(visible.size.width);
                let height = px(f.height.max(480.)).min(visible.size.height);
                Bounds::new(
                    point(
                        px(f.x).max(visible.left()).min(visible.right() - width),
                        px(f.y).max(visible.top()).min(visible.bottom() - height),
                    ),
                    size(width, height),
                )
            })
            .unwrap_or_else(|| Bounds::centered(None, size(px(1280.), px(800.)), cx));
        let initial = match &init {
            NewWindowInit::Restore(state) => state.clone(),
            NewWindowInit::OpenDir { project_id } => {
                eggie_domain::WindowPresentation::new(id, *project_id)
            }
            NewWindowInit::Empty | NewWindowInit::EmptyAt(_) => {
                let mut window = eggie_domain::WindowPresentation::new(id, preferred);
                window.project_picker = true;
                window
            }
        };
        if manager
            .update(cx, |manager, cx| manager.capture(initial, None, cx))
            .is_err()
        {
            return None;
        }
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Eggie".into()),
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
                        id,
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
        );
        match result {
            Ok(handle) => {
                manager.update(cx, |manager, cx| manager.register(id, handle, cx));
                Some(handle)
            }
            Err(error) => {
                eprintln!("failed to open Eggie window: {error:#}");
                manager.update(cx, |manager, cx| manager.discard_empty_window(id, cx));
                None
            }
        }
    }

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
    ) -> (
        ProjectId,
        Vec<SeededSession>,
        Option<eggie_domain::WindowPresentation>,
    ) {
        let manager = WindowManager::global(cx);
        let state = manager.read(cx).state.windows[&window_id].clone();
        if let NewWindowInit::OpenDir { project_id } = init {
            if manager.read(cx).terminating_all
                || manager.read(cx).pending_projects.contains(project_id)
            {
                return (state.active_project, Vec::new(), Some(state));
            }
            let (root, profile) =
                Self::project_root_and_profile(*project_id, project_store, profile_store, cx);
            let sessions =
                Self::create_session(client, *project_id, root, &profile, config, global)
                    .map(|session| SeededSession::launched(session, profile.id))
                    .into_iter()
                    .collect();
            (state.active_project, sessions, None)
        } else {
            let local: HashSet<_> = state
                .views
                .values()
                .flat_map(|v| v.layout.terminal_items())
                .filter_map(|t| t.session_id)
                .collect();
            let sessions = manager
                .read(cx)
                .sessions
                .iter()
                .filter(|s| local.contains(&s.id))
                .cloned()
                .map(SeededSession::adopted)
                .collect();
            (state.active_project, sessions, Some(state))
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
                let sidebar = SidebarRestoreState::from(&restored);
                for (project_id, view) in restored.views {
                    project_views.insert(project_id, view);
                }
                // Persisted tab titles may be stale (the shell kept running); refresh from the
                // global session catalog. The session poll would fix these too, but not before first paint.
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
        // profile-resolved palette a freshly launched session was spawned with, and for a restored
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
        cx.observe(&settings, |app, _, cx| {
            app.dismiss_project_selector(cx);
            let layout = app.settings.read(cx).config().sidebar_layout;
            if app
                .applied_config
                .as_ref()
                .is_some_and(|config| config.sidebar_layout != layout)
            {
                app.dismiss_project_popover(cx);
                app.dismiss_sidebar_preview(cx);
                app.clear_project_drag(cx);
                app.sidebar
                    .project_drag
                    .scroll
                    .set_offset(point(px(0.), px(0.)));
            }
            cx.notify();
        })
        .detach();
        // Update state changes (found/downloading/ready) re-render the sidebar indicator.
        cx.observe(&updates, |_, _, cx| cx.notify()).detach();
        // Cross-window project sync: when any window mutates the shared store, reconcile this
        // window's per-project views (add/drop) and re-render.
        cx.observe(&project_store, |app, _, cx| {
            app.dismiss_project_selector(cx);
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
            app.dismiss_project_selector(cx);
            app.dismiss_project_popover(cx);
            app.dismiss_sidebar_preview(cx);
        })
        .detach();
        cx.observe_window_activation(window, |app, window, cx| {
            if !window.is_window_active() {
                app.dismiss_project_selector(cx);
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
                collapsed_projects: match &init {
                    NewWindowInit::Restore(w) => w.collapsed_projects.clone(),
                    _ => HashSet::new(),
                },
            },
            sidebar: SidebarState {
                project_selector: None,
                project_selector_bounds: Rc::new(std::cell::Cell::new(None)),
                project_row_bounds: Rc::new(RefCell::new(HashMap::new())),
                project_drag: ProjectDragState::default(),
                project_popover: None,
                hovered_project: None,
                project_hover_revision: 0,
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
                progress_updates: HashMap::new(),
                sessions,
                global_sessions: WindowManager::global(cx).read(cx).sessions.clone(),
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
            // launched one carries its profile's palette, a restored one carries the palette of the run
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
            progress_animation_scheduled: false,
            progress_animation_epoch: Instant::now(),
            bell_flash: HashMap::new(),
            bell_flash_scheduled: false,
            cursor_blink_epoch: Instant::now(),
            cursor_blink_scheduled: false,
            progress_timeouts,
            window_id,
            window_number: WindowManager::global(cx).read(cx).state.windows[&window_id].number,
            window_manager: WindowManager::global(cx),
            window_revision: 0,
            window_frame: Some(eggie_domain::WindowFrame {
                x: f32::from(window.bounds().origin.x),
                y: f32::from(window.bounds().origin.y),
                width: f32::from(window.bounds().size.width),
                height: f32::from(window.bounds().size.height),
            }),
            project_picker: match &init {
                NewWindowInit::Restore(w) => w.project_picker,
                NewWindowInit::Empty | NewWindowInit::EmptyAt(_) => true,
                _ => false,
            },
            project_store,
            profile_store,
            snippet_store,
            notes_store,
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
            completion: CompletionUi::default(),
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
            group_drop_bounds: Rc::new(RefCell::new(HashMap::new())),
            tab_drop_bounds: Rc::new(RefCell::new(HashMap::new())),
            dragged_tab: None,
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
        app.install_window_lifecycle(window, cx);
        app.publish_window_state(cx);
        let mut catalog = app.window_manager.read(cx).sessions.clone();
        for session in &app.session_feed.sessions {
            if !catalog.iter().any(|existing| existing.id == session.id) {
                catalog.push(session.clone());
            }
        }
        app.window_manager
            .update(cx, |manager, cx| manager.set_sessions(catalog, cx));
        app.start_polling(cx);
        app.start_inspection_polling(cx);
        app.start_shell_integration_polling(cx);
        app.start_latency_reporting(cx);
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
        app.publish_window_state(cx);
        app
    }
}

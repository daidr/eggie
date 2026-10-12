use crate::{
    app::{EggieApp, NewWindowInit, NotificationRoutes},
    project_store::ProjectStore,
    settings::{Language, SettingsStore},
    settings_window::open_settings_window,
};
use eggie_ipc::DaemonClient;
use eggie_update::UpdateState;
use gpui::{App, Entity, Menu, MenuItem, OsAction, SystemMenuType, actions};
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub(crate) struct FocusWindow {
    pub id: eggie_domain::WindowId,
}
impl gpui::Action for FocusWindow {
    fn boxed_clone(&self) -> Box<dyn gpui::Action> {
        Box::new(self.clone())
    }
    fn partial_eq(&self, other: &dyn gpui::Action) -> bool {
        other.as_any().downcast_ref::<Self>() == Some(self)
    }
    fn name(&self) -> &'static str {
        "eggie::FocusWindow"
    }
    fn name_for_type() -> &'static str {
        "eggie::FocusWindow"
    }
    fn build(value: serde_json::Value) -> anyhow::Result<Box<dyn gpui::Action>> {
        Ok(Box::new(serde_json::from_value::<Self>(value)?))
    }
}
const EGGIE_HELP_URL: &str = "https://github.com/daidr/eggie";
actions!(
    eggie,
    [
        OpenSettings,
        NewWindow,
        Hide,
        HideOthers,
        ShowAll,
        Quit,
        TerminalCopy,
        TerminalPaste,
        TerminalPastePreview,
        TerminalSelectAll,
        TerminalFind,
        TerminalSuggest,
        NewTab,
        CloseTab,
        NextTab,
        PrevTab,
        SplitRight,
        SplitDown,
        ClearScreen,
        ScrollTop,
        ScrollBottom,
        PageUp,
        PageDown,
        FontIncrease,
        FontDecrease,
        FontReset,
        JumpPrevPrompt,
        JumpNextPrompt,
        About,
        CheckForUpdates,
        ToggleSecureKeyboardEntry,
        CloseWindow,
        CloseAllWindows,
        FindNext,
        FindPrevious,
        SplitLeft,
        SplitUp,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen,
        HelpDocs,
        CommandPalette,
        TerminalSwitcher,
        WindowSwitcher,
        UndoWindowOperation,
        QuitAndTerminate,
        ToggleLeftSidebar,
        ToggleRightSidebar,
        /// Open the profile picker, then create a terminal from the chosen profile (a one-off
        /// override of the project's default profile).
        NewTabWithProfile
    ]
);

pub(crate) fn install(
    settings: Entity<SettingsStore>,
    project_store: Entity<ProjectStore>,
    profile_store: Entity<crate::profile_store::ProfileStore>,
    snippet_store: Entity<crate::snippet_store::SnippetStore>,
    notes_store: Entity<crate::notes_store::NotesStore>,
    client: DaemonClient,
    notification_routes: NotificationRoutes,
    updates: Entity<crate::update_controller::UpdateController>,
    cx: &mut App,
) {
    let settings_for_action = settings.clone();
    let settings_for_terminate = settings.clone();
    let updates_for_action = updates.clone();
    let profile_store_for_action = profile_store.clone();
    let project_store_for_action = project_store.clone();
    cx.on_action(move |_: &OpenSettings, cx| {
        open_settings_window(
            settings_for_action.clone(),
            updates_for_action.clone(),
            profile_store_for_action.clone(),
            project_store_for_action.clone(),
            cx,
        )
    });
    // New Window is an app-level action (like OpenSettings): it works even when no Eggie window is
    // focused, and opens a fresh main window sharing the same settings/project-store entities.
    let settings_for_new_window = settings.clone();
    let updates_for_new_window = updates.clone();
    let project_store_for_new_window = project_store.clone();
    let profile_store_for_new_window = profile_store.clone();
    let snippet_store_for_new_window = snippet_store.clone();
    let notes_store_for_new_window = notes_store.clone();
    let client_for_new_window = client.clone();
    let routes_for_new_window = notification_routes.clone();
    cx.on_action(move |_: &NewWindow, cx| {
        EggieApp::open_new_window(
            cx,
            client_for_new_window.clone(),
            settings_for_new_window.clone(),
            updates_for_new_window.clone(),
            project_store_for_new_window.clone(),
            profile_store_for_new_window.clone(),
            snippet_store_for_new_window.clone(),
            notes_store_for_new_window.clone(),
            routes_for_new_window.clone(),
            NewWindowInit::Empty,
        );
    });
    let undo_resources = (
        client.clone(),
        settings.clone(),
        updates.clone(),
        project_store.clone(),
        profile_store.clone(),
        snippet_store.clone(),
        notes_store.clone(),
        notification_routes.clone(),
    );
    cx.on_action(move |_: &UndoWindowOperation, cx| {
        let (client, settings, updates, projects, profiles, snippets, notes, routes) =
            undo_resources.clone();
        EggieApp::undo_shared_window_operation(
            client, settings, updates, projects, profiles, snippets, notes, routes, cx,
        );
    });
    // Manual update check: surface the update window (which shows the live state) and kick off
    // a check when idle.
    let updates_for_check = updates.clone();
    let settings_for_check = settings.clone();
    cx.on_action(move |_: &CheckForUpdates, cx| {
        let channel = settings_for_check.read(cx).config().update_channel.slug();
        updates_for_check.update(cx, |controller, cx| {
            // Re-check from any settled state (idle / up-to-date / error);
            // leave in-flight or pending work (checking / available /
            // downloading / ready) untouched — the window shows its progress.
            if matches!(
                controller.state(),
                UpdateState::Idle | UpdateState::UpToDate | UpdateState::Error(_)
            ) {
                controller.check(false, channel, cx);
            }
        });
        // Always surface the window so the user sees the current state,
        // whether that's a fresh check, a pending download, or an error.
        crate::update_window::open_update_window(
            updates_for_check.clone(),
            settings_for_check.clone(),
            cx,
        );
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|action: &FocusWindow, cx| {
        let handle = crate::workspace::WindowManager::global(cx)
            .read(cx)
            .handles
            .get(&action.id)
            .copied();
        if let Some(handle) = handle {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    });
    cx.on_action(|_: &Quit, cx| {
        let manager = crate::workspace::WindowManager::global(cx);
        manager.update(cx, |manager, _| {
            manager.persist();
            manager.quitting = true;
        });
        cx.quit();
    });
    let terminate_client = client.clone();
    cx.on_action(move |_: &QuitAndTerminate, cx| {
        let client = terminate_client.clone();
        let manager = crate::workspace::WindowManager::global(cx);
        let Some(handle) = cx.active_window() else {
            cx.dispatch_action(&NewWindow);
            cx.defer(|cx| cx.dispatch_action(&QuitAndTerminate));
            return;
        };
        let lang = settings_for_terminate.read(cx).config().language;
        let _ = handle.update(cx, |_, window, cx| {
            let prompt = window.prompt(
                gpui::PromptLevel::Warning,
                if lang == Language::SimplifiedChinese {
                    "结束全部终端并退出？"
                } else {
                    "Terminate all terminals and quit?"
                },
                Some(if lang == Language::SimplifiedChinese {
                    "其他窗口和后台终端也会结束。"
                } else {
                    "Includes other windows and background terminals."
                }),
                &[lang.cancel(), lang.terminate_all()],
                cx,
            );
            cx.spawn(async move |cx| {
                if prompt.await != Ok(1) {
                    return;
                }
                let _ = manager.update(cx, |manager, _| manager.terminating_all = true);
                let response = cx
                    .background_executor()
                    .spawn(async move {
                        if let Ok(eggie_protocol::DaemonResponse::Sessions { sessions }) =
                            client.request(eggie_protocol::ClientRequest::ListSessions)
                        {
                            for session in sessions {
                                match client.request(eggie_protocol::ClientRequest::Terminate {
                                    session_id: session.id,
                                })? {
                                    eggie_protocol::DaemonResponse::Ok => {}
                                    response => {
                                        anyhow::bail!("Could not terminate terminal: {response:?}")
                                    }
                                }
                            }
                        } else {
                            anyhow::bail!("Could not list terminals");
                        }
                        eggie_ipc::terminate_daemon_at(&eggie_ipc::daemon_socket_path());
                        Ok::<_, anyhow::Error>(())
                    })
                    .await;
                let _ = cx.update(|cx| {
                    manager.update(cx, |manager, _| manager.terminating_all = false);
                    if let Err(error) = &response {
                        manager.update(cx, |manager, cx| {
                            manager.error = Some(error.to_string());
                            cx.notify();
                        });
                    }
                    if response.is_ok() {
                        manager.update(cx, |manager, _| {
                            manager.state = Default::default();
                            manager.persist();
                            manager.quitting = true;
                        });
                        cx.quit();
                    }
                });
            })
            .detach();
        });
    });
    // App-level menu commands that don't act on a specific terminal window.
    cx.on_action(|_: &About, _cx| crate::system_menu::show_about_panel());
    cx.on_action(|_: &CloseAllWindows, cx| {
        let handles = crate::workspace::WindowManager::global(cx)
            .read(cx)
            .handles
            .values()
            .copied()
            .collect::<Vec<_>>();
        for handle in handles {
            let _ = handle.update(cx, |app, window, cx| {
                app.close_presentation(window, cx);
                window.remove_window();
            });
        }
    });
    cx.on_action(|_: &HelpDocs, cx| cx.open_url(EGGIE_HELP_URL));
    // Toggling Secure Keyboard Entry flips a process-global input mode, then rebuilds the menu
    // bar so the checkmark reflects the new state (menus have no in-place item update API).
    let settings_for_secure_input = settings.clone();
    cx.on_action(move |_: &ToggleSecureKeyboardEntry, cx| {
        crate::system_menu::toggle_secure_input();
        let config = settings_for_secure_input.read(cx).config().clone();
        let lua = settings_for_secure_input
            .read(cx)
            .lua_config()
            .registered
            .clone();
        crate::keybindings::rebuild_keymap(&config, &lua, cx);
        rebuild_menus(config.language, cx);
    });
    // Build the keymap from settings (defaults + user overrides). This also
    // registers the text-input bindings in the correct order to preserve the
    // binding-index tiebreak. Action handlers above are registered exactly once
    // and are intentionally *not* rebuilt when the keymap changes.
    let config = settings.read(cx).config().clone();
    let lua = settings.read(cx).lua_config().registered.clone();
    crate::keybindings::rebuild_keymap(&config, &lua, cx);
    let language = config.language;
    rebuild_menus(language, cx);

    let settings_for_observer = settings.clone();
    cx.observe(&settings, move |_, cx| {
        let config = settings_for_observer.read(cx).config().clone();
        // Rebuild the keymap first: `set_menus` snapshots the current keymap to resolve each
        // menu item's shortcut, so it must run *after* the keymap reflects this change or the
        // menu bar lags one edit behind.
        let lua = settings_for_observer
            .read(cx)
            .lua_config()
            .registered
            .clone();
        crate::keybindings::rebuild_keymap(&config, &lua, cx);
        rebuild_menus(config.language, cx);
    })
    .detach();
}

pub(crate) fn rebuild_menus(language: Language, cx: &mut App) {
    let mut menus = build_menus(language);
    if let Some(global) = cx.try_global::<crate::workspace::SharedWindows>() {
        let manager = global.0.read(cx);
        if let Some(menu) = menus
            .iter_mut()
            .find(|menu| menu.name.as_ref() == language.window_menu())
        {
            menu.items.push(MenuItem::separator());
            for window in manager.state.windows.values() {
                menu.items.push(
                    MenuItem::action(
                        manager.window_label(window.id),
                        FocusWindow { id: window.id },
                    )
                    .checked(manager.state.active_window == Some(window.id)),
                );
            }
        }
    }
    cx.set_menus(menus);
}

fn build_menus(language: Language) -> Vec<Menu> {
    // SF Symbol names mirror ghostty's `setupMenuImages` where it has an equivalent item;
    // the rest are chosen to match. Unknown symbols degrade to no icon (see gpui setImage).
    vec![
        Menu::new("Eggie").items([
            MenuItem::action(language.about_eggie(), About).icon("info.circle"),
            MenuItem::separator(),
            MenuItem::action(language.settings_menu_item(), OpenSettings).icon("gear"),
            MenuItem::action(language.secure_keyboard_entry(), ToggleSecureKeyboardEntry)
                .icon("lock.display")
                .checked(crate::system_menu::secure_input_enabled()),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(language.hide_eggie(), Hide),
            MenuItem::action(language.hide_others(), HideOthers),
            MenuItem::action(language.show_all(), ShowAll),
            MenuItem::separator(),
            MenuItem::action(language.quit_eggie(), Quit),
            MenuItem::action(language.quit_and_terminate(), QuitAndTerminate),
        ]),
        Menu::new(language.file_menu()).items([
            MenuItem::action(language.action_new_tab(), NewTab).icon("macwindow"),
            MenuItem::action(language.new_window_menu_item(), NewWindow)
                .icon("macwindow.badge.plus"),
            MenuItem::separator(),
            MenuItem::action(language.action_split_right(), SplitRight)
                .icon("rectangle.righthalf.inset.filled"),
            MenuItem::action(language.action_split_left(), SplitLeft)
                .icon("rectangle.leadinghalf.inset.filled"),
            MenuItem::action(language.action_split_down(), SplitDown)
                .icon("rectangle.bottomhalf.inset.filled"),
            MenuItem::action(language.action_split_up(), SplitUp)
                .icon("rectangle.tophalf.inset.filled"),
            MenuItem::separator(),
            MenuItem::action(language.action_close_tab(), CloseTab).icon("xmark"),
            MenuItem::action(language.action_close_window(), CloseWindow).icon("xmark.rectangle"),
            MenuItem::action(language.close_all_windows_menu_item(), CloseAllWindows)
                .icon("xmark.rectangle.fill"),
        ]),
        Menu::new(language.edit_menu()).items([
            MenuItem::os_action(language.copy(), TerminalCopy, OsAction::Copy).icon("doc.on.doc"),
            MenuItem::os_action(language.paste(), TerminalPaste, OsAction::Paste)
                .icon("doc.on.clipboard"),
            MenuItem::action(
                if language == Language::SimplifiedChinese {
                    "粘贴预览"
                } else {
                    "Paste preview"
                },
                TerminalPastePreview,
            )
            .icon("doc.text.magnifyingglass"),
            MenuItem::separator(),
            MenuItem::os_action(
                language.select_all(),
                TerminalSelectAll,
                OsAction::SelectAll,
            )
            .icon("text.justify"),
            MenuItem::separator(),
            MenuItem::action(language.find_menu_item(), TerminalFind).icon("magnifyingglass"),
            MenuItem::action(language.action_find_next(), FindNext).icon("chevron.down"),
            MenuItem::action(language.action_find_previous(), FindPrevious).icon("chevron.up"),
        ]),
        Menu::new(language.view_menu()).items([
            MenuItem::action(language.command_palette_menu_item(), CommandPalette).icon("command"),
            MenuItem::separator(),
            MenuItem::action(language.action_toggle_left_sidebar(), ToggleLeftSidebar)
                .icon("sidebar.left"),
            MenuItem::action(language.action_toggle_right_sidebar(), ToggleRightSidebar)
                .icon("sidebar.right"),
            MenuItem::separator(),
            MenuItem::action(language.action_font_increase(), FontIncrease)
                .icon("textformat.size.larger"),
            MenuItem::action(language.action_font_decrease(), FontDecrease)
                .icon("textformat.size.smaller"),
            MenuItem::action(language.action_font_reset(), FontReset).icon("textformat.size"),
            MenuItem::separator(),
            MenuItem::action(language.action_scroll_top(), ScrollTop).icon("arrow.up.to.line"),
            MenuItem::action(language.action_scroll_bottom(), ScrollBottom)
                .icon("arrow.down.to.line"),
            MenuItem::action(language.action_page_up(), PageUp).icon("arrow.up"),
            MenuItem::action(language.action_page_down(), PageDown).icon("arrow.down"),
            MenuItem::separator(),
            MenuItem::action(language.action_jump_prev_prompt(), JumpPrevPrompt)
                .icon("arrowtriangle.up.circle"),
            MenuItem::action(language.action_jump_next_prompt(), JumpNextPrompt)
                .icon("arrowtriangle.down.circle"),
            MenuItem::separator(),
            MenuItem::action(language.action_clear_screen(), ClearScreen).icon("clear"),
        ]),
        Menu::new(language.window_menu()).items([
            MenuItem::action(language.window_switcher(), WindowSwitcher),
            MenuItem::action(language.terminal_switcher(), TerminalSwitcher),
            MenuItem::action(language.undo_window_operation(), UndoWindowOperation),
            MenuItem::separator(),
            MenuItem::action(language.action_minimize_window(), MinimizeWindow)
                .icon("minus.rectangle"),
            MenuItem::action(language.zoom_menu_item(), ZoomWindow)
                .icon("arrow.up.left.and.arrow.down.right"),
            MenuItem::separator(),
            MenuItem::action(language.action_toggle_full_screen(), ToggleFullScreen)
                .icon("square.arrowtriangle.4.outward"),
            MenuItem::separator(),
            MenuItem::action(language.action_next_tab(), NextTab).icon("chevron.right"),
            MenuItem::action(language.action_prev_tab(), PrevTab).icon("chevron.left"),
        ]),
        Menu::new(language.help_menu()).help().items([
            MenuItem::action(language.help_docs_menu_item(), HelpDocs).icon("questionmark.circle"),
            MenuItem::separator(),
            MenuItem::action(language.check_for_updates_menu_item(), CheckForUpdates)
                .icon("arrow.down.circle"),
        ]),
    ]
}

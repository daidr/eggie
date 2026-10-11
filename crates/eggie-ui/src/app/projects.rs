use super::*;

impl EggieApp {
    pub(super) fn prompt_add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.language;
        let receiver = prompt_for_text(
            window,
            language.new_project_title(),
            None,
            Some(language.project_name_placeholder()),
            None,
            language.ok(),
            language.cancel(),
        );
        let app = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let name = receiver.await.ok()??;
                let name = name.trim();
                if name.is_empty() {
                    return None;
                }
                app.update(cx, |app, cx| app.add_project(name.to_owned(), cx))
                    .ok()?;
                Some(())
            })
            .detach();
    }

    /// Add a new project to the shared store. Per D2/D7 this does NOT spawn a terminal — the new
    /// project starts empty (an empty layout) and the first terminal is created when the user acts
    /// in it. `reconcile_project_views` (driven by the store observer) creates the empty view; here
    /// we just make the new project active in this window.
    pub(super) fn add_project(&mut self, name: String, cx: &mut Context<Self>) {
        let project = Project::new(name);
        let id = project.id;
        self.project_store
            .update(cx, |store, cx| store.add(project, cx));
        // The observer reconciles views asynchronously; reconcile now so `active_project` is valid
        // immediately (the store update already notified, but our own view map lags until reconcile).
        self.reconcile_project_views(cx);
        self.workspace_state.active_project = id;
        self.ensure_snapshot_watchers(cx);
        self.sync_terminal_focus();
        cx.notify();
    }

    pub(super) fn activate_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        self.dismiss_project_selector(cx);
        if self.workspace_state.project_views.contains_key(&project_id) {
            self.project_picker = false;
            self.workspace_state.active_project = project_id;
            self.ensure_snapshot_watchers(cx);
            self.sync_terminal_focus();
            cx.notify();
        }
    }

    pub(super) fn toggle_project_collapsed(
        &mut self,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_project_popover(cx);
        if self
            .workspace_state
            .collapsed_projects
            .contains(&project_id)
        {
            self.workspace_state.collapsed_projects.remove(&project_id);
        } else {
            self.workspace_state.collapsed_projects.insert(project_id);
        }
        cx.notify();
    }

    pub(super) fn create_terminal_in_project(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_project_popover(cx);
        self.activate_project(project_id, cx);
        self.workspace_state.collapsed_projects.remove(&project_id);
        let group_id = self.workspace_state.workspace().active_group_id;
        self.new_terminal(group_id, None, None, window, cx);
        cx.notify();
    }

    pub(super) fn prompt_set_project_root(
        &mut self,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(
                self.language
                    .set_working_directory_prompt()
                    .to_owned()
                    .into(),
            ),
        });
        let app = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let mut paths = paths.await.ok()?.ok()??;
            let root = paths.pop()?;
            app.update(cx, move |app, cx| {
                app.project_store
                    .update(cx, |store, cx| store.set_root(project_id, Some(root), cx));
            })
            .ok()?;
            Some(())
        })
        .detach();
    }

    pub(super) fn show_project_context_menu(
        &mut self,
        project_id: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(_) = self.workspace_state.project_views.get(&project_id) else {
            return;
        };
        let tab_count = self
            .window_manager
            .read(cx)
            .state
            .project_session_ids(project_id)
            .len();
        let Some(menu) = prepare_project_menu(window, tab_count, self.language) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            // Open the picker in "set this project's default" mode. Reused rather than adding a submenu
            // of profiles to the native menu: the list is user-defined and unbounded, and the picker
            // already filters, marks the current default, and shows badge colors.
            //
            // It needs a `&mut Window` (it focuses the palette's input) and there is none in scope —
            // the native menu ran a nested event loop. `update_in` supplies the entity's own window
            // alongside its context, and is called at the top level of this task rather than inside
            // another `update`: nesting a second update of the *same* entity aborts the process with
            // "cannot update EggieApp while it is already being updated". The sibling commands below
            // look like they nest too, but they either update a different entity (`window.update`) or
            // do it from a further `spawn`, by which point this lease is released.
            if matches!(command, NativeProjectMenuCommand::SetDefaultProfile) {
                this.update_in(cx, move |app, window, cx| {
                    if !app.workspace_state.project_views.contains_key(&project_id) {
                        return;
                    }
                    app.open_palette(PaletteMode::ProjectDefaultProfile(project_id), window, cx);
                })
                .ok();
                return;
            }
            this.update(cx, move |app, cx| {
                if !app.workspace_state.project_views.contains_key(&project_id) {
                    return;
                }
                match command {
                    NativeProjectMenuCommand::EditName => {
                        app.prompt_rename_project(project_id, cx);
                    }
                    NativeProjectMenuCommand::SetRoot => {
                        app.prompt_set_project_root(project_id, cx);
                    }
                    NativeProjectMenuCommand::CloseTabs => {
                        app.close_project_tabs(project_id, cx);
                    }
                    NativeProjectMenuCommand::DeleteProject => {
                        app.delete_project(project_id, cx);
                    }
                    // Handled above: it needs a window, so it cannot share this closure.
                    NativeProjectMenuCommand::SetDefaultProfile => {}
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_process_context_menu(
        &mut self,
        pid: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let Some(menu) = prepare_process_menu(window, self.language) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Some(command) = menu.show() else {
                return;
            };
            this.update(cx, move |app, cx| {
                app.handle_process_menu_command(command, pid, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn handle_process_menu_command(
        &mut self,
        command: NativeProcessMenuCommand,
        pid: u32,
        cx: &mut Context<Self>,
    ) {
        match command {
            NativeProcessMenuCommand::Terminate => {
                kill_process(pid, libc::SIGTERM);
            }
            NativeProcessMenuCommand::ForceKill => {
                kill_process(pid, libc::SIGKILL);
            }
            NativeProcessMenuCommand::CopyPid => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(pid.to_string()));
            }
            NativeProcessMenuCommand::CopyExecutablePath => {
                if let Some(path) = executable_path_for_pid(pid) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(path));
                }
            }
        }
    }

    pub(super) fn prompt_rename_project(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        let Some(project) = self
            .workspace_state
            .projects
            .iter()
            .find(|project| project.id == project_id)
        else {
            return;
        };
        let current_name = project.name.clone();
        let language = self.language;
        let Some(window) = cx.active_window() else {
            return;
        };
        let app = cx.entity().downgrade();
        window
            .update(cx, |_, window, cx| {
                let receiver = prompt_for_text(
                    window,
                    language.rename_project_title(),
                    None,
                    None,
                    Some(&current_name),
                    language.ok(),
                    language.cancel(),
                );
                cx.spawn(async move |cx| {
                    let name = receiver.await.ok()??;
                    let name = name.trim();
                    if name.is_empty() {
                        return None;
                    }
                    app.update(cx, move |app, cx| {
                        app.project_store.update(cx, |store, cx| {
                            store.rename(project_id, name.to_owned(), cx)
                        });
                    })
                    .ok()?;
                    Some(())
                })
                .detach();
            })
            .ok();
    }

    /// Prompt for a port forward spec and open it on the session's ssh master. The spec is a single
    /// line parsed by [`parse_forward_spec`].
    pub(super) fn prompt_forward_port(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = self.language;
        let client = self.client.clone();
        let executor = cx.background_executor().clone();
        let receiver = prompt_for_text(
            window,
            language.forward_port(),
            Some(language.forward_port_prompt()),
            None,
            None,
            language.ok(),
            language.cancel(),
        );
        cx.spawn(async move |_, _| {
            let text = receiver.await.ok()??;
            let Some(spec) = parse_forward_spec(text.trim()) else {
                return None;
            };
            let request_client = client.clone();
            let _ = executor
                .spawn(async move {
                    request_client.request(ClientRequest::ForwardPort { session_id, spec })
                })
                .await;
            Some(())
        })
        .detach();
    }

    pub(super) fn close_project_tabs(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.confirm_project_operation(project, false, cx);
    }

    pub(super) fn delete_project(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.confirm_project_operation(project, true, cx);
    }

    fn confirm_project_operation(
        &mut self,
        project: ProjectId,
        delete: bool,
        cx: &mut Context<Self>,
    ) {
        self.publish_window_state(cx);
        let manager = self.window_manager.clone();
        if !manager.update(cx, |manager, _| manager.pending_projects.insert(project)) {
            return;
        }
        let client = self.client.clone();
        let store = self.project_store.clone();
        let zh = self.language == Language::SimplifiedChinese;
        let ids = manager.read(cx).state.project_session_ids(project);
        let live = manager
            .read(cx)
            .sessions
            .iter()
            .filter(|s| s.project_id == project)
            .map(|s| s.id)
            .collect::<HashSet<_>>();
        cx.spawn(async move |app, cx| {
            if !live.is_empty() {
                let prompt = app.update_in(cx, |_, window, cx| window.prompt(PromptLevel::Warning,
                    if delete { if zh { "删除项目并结束其终端？" } else { "Delete project and terminate its terminals?" } }
                    else { if zh { "结束此项目的全部终端？" } else { "Terminate all terminals in this project?" } },
                    Some(if zh { "其他窗口和后台终端也会结束，当前命令将中断。" } else { "Includes other windows and background terminals. Running commands will stop." }),
                    &[if zh { "取消" } else { "Cancel" }, if zh { "结束终端" } else { "Terminate" }], cx));
                let confirmed = match prompt { Ok(prompt) => prompt.await == Ok(1), Err(_) => false };
                if !confirmed { let _ = manager.update(cx, |manager, _| { manager.pending_projects.remove(&project); }); return; }
            }
            let result = cx.background_executor().spawn(async move {
                let sessions = match client.request(ClientRequest::ListSessions)? { DaemonResponse::Sessions { sessions } => sessions, other => anyhow::bail!("Unexpected session response: {other:?}") };
                let mut terminated = Vec::new();
                let mut error = None;
                for session in sessions.iter().filter(|s| s.project_id == project) {
                    match client.request(ClientRequest::Terminate { session_id: session.id }) {
                        Ok(DaemonResponse::Ok) => terminated.push(session.id),
                        other => { error = Some(format!("Could not terminate project terminal: {other:?}")); break; }
                    }
                }
                Ok::<_, anyhow::Error>((terminated, error))
            }).await;
            let _ = cx.update(|cx| {
                manager.update(cx, |manager, _| { manager.pending_projects.remove(&project); });
                match result {
                    Ok((terminated, error)) => {
                        manager.update(cx, |manager, cx| {
                            for id in terminated { manager.remove_session(id, cx); }
                            if let Some(error) = &error { manager.error = Some(error.clone()); cx.notify(); }
                            else { for id in ids { manager.remove_session(id, cx); } }
                        });
                        if delete && error.is_none() { store.update(cx, |store, cx| {
                            store.remove(project, cx);
                            if store.projects().is_empty() { store.add(Project::new("Home".into()), cx); }
                        }); }
                    }
                    Err(error) => manager.update(cx, |manager, cx| { manager.error = Some(error.to_string()); cx.notify(); }),
                }
            });
        }).detach();
    }
}

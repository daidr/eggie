use super::*;
use anyhow::Context as _;
use eggie_protocol::{AutomationRequest as Request, AutomationResponse as Response};
use serde_json::{Value, json};

fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !name.trim().is_empty() && name.len() <= 512,
        "Name must contain 1–512 bytes"
    );
    Ok(())
}
fn validate_root(root: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        root.is_absolute() && root.is_dir(),
        "Expected an existing absolute directory"
    );
    Ok(())
}
fn location_json(location: eggie_domain::ItemLocation) -> Value {
    json!({"window_id":location.window_id, "project_id":location.project_id, "group_id":location.group_id, "item_id":location.item_id})
}
fn terminal_text(response: DaemonResponse) -> anyhow::Result<Value> {
    match response {
        DaemonResponse::TerminalText {
            session_id,
            text,
            truncated,
        } => Ok(json!({"session_id":session_id,"text":text,"truncated":truncated})),
        DaemonResponse::ShellIntegrationState { state, .. } => Ok(serde_json::to_value(state)?),
        DaemonResponse::Ok => Ok(json!({})),
        response => anyhow::bail!("Unexpected response: {response:?}"),
    }
}

impl EggieApp {
    pub(super) async fn dispatch_automation(
        request: Request,
        client: DaemonClient,
        projects: Entity<ProjectStore>,
        profiles: Entity<ProfileStore>,
        cx: &mut gpui::AsyncApp,
    ) -> Response {
        let rpc = match request {
            Request::TerminalRead {
                session_id,
                tail_lines,
            } => Some(ClientRequest::ReadTerminalText {
                session_id,
                tail_lines,
            }),
            Request::CommandsList { session_id } => {
                Some(ClientRequest::GetShellIntegrationState { session_id })
            }
            Request::CommandsOutput {
                session_id,
                command_id,
                max_bytes,
            } => Some(ClientRequest::ReadCommandOutput {
                session_id,
                command_id,
                max_bytes,
            }),
            Request::CommandsLocate {
                session_id,
                command_id,
            } => Some(ClientRequest::LocateCommandOutput {
                session_id,
                command_id,
            }),
            _ => None,
        };
        if let Some(rpc) = rpc {
            let result = cx
                .background_executor()
                .spawn(async move {
                    terminal_text(client.request_with_timeout(rpc, Duration::from_secs(5))?)
                })
                .await;
            return match result {
                Ok(result) => Response::Ok { result },
                Err(error) => Response::error("terminal_unavailable", error.to_string()),
            };
        }
        // Terminate off the UI thread, and only remove the presentation after the daemon acknowledges.
        if let Request::TabsClose { session_id, force } = request {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let DaemonResponse::Sessions { sessions } = client.request_with_timeout(
                        ClientRequest::ListSessions,
                        Duration::from_secs(5),
                    )?
                    else {
                        anyhow::bail!("Session list unavailable");
                    };
                    if let Some(session) = sessions.iter().find(|session| session.id == session_id)
                    {
                        anyhow::ensure!(
                            force
                                || session.status != SessionStatus::Running
                                || (session.current_process.pid == session.shell_pid
                                    && session.active_command.is_none()
                                    && session.remote.is_none()),
                            "Terminal has a running command; set force=true to close it"
                        );
                        client.request_with_timeout(
                            ClientRequest::Terminate { session_id },
                            Duration::from_secs(5),
                        )?;
                    }
                    anyhow::Ok(())
                })
                .await;
            if let Err(error) = result {
                return Response::error("close_failed", error.to_string());
            }
            let _ = cx.update(|cx| {
                WindowManager::global(cx)
                    .update(cx, |manager, cx| manager.remove_session(session_id, cx))
            });
            return Response::Ok {
                result: json!({"session_id":session_id,"closed":true}),
            };
        }
        let result = cx.update(|cx| -> anyhow::Result<Value> {
            let manager = WindowManager::global(cx);
            let all_projects = projects.read(cx).projects().to_vec();
            manager.update(cx, |manager, cx| manager.reconcile_projects(&all_projects, cx));
            match request {
                Request::ProjectsList {} => Ok(json!({"projects":projects.read(cx).projects()})),
                Request::ProjectsCreate { name, root } => {
                    validate_name(&name)?;
                    if let Some(root) = &root { validate_root(root)?; }
                    let mut project = eggie_domain::Project::new(name.trim().into());
                    project.root = root;
                    projects.update(cx, |store, cx| store.add(project.clone(), cx));
                    Ok(json!({"project":project}))
                }
                Request::ProjectsUpdate { project_id, name, root, clear_root } => {
                    anyhow::ensure!(projects.read(cx).projects().iter().any(|p| p.id == project_id), "Unknown project");
                    anyhow::ensure!(!(clear_root && root.is_some()), "root and clear_root are mutually exclusive");
                    if let Some(name) = &name { validate_name(name)?; }
                    if let Some(root) = &root { validate_root(root)?; }
                    projects.update(cx, |store, cx| {
                        if let Some(name) = name { store.rename(project_id, name.trim().into(), cx); }
                        if root.is_some() || clear_root { store.set_root(project_id, root, cx); }
                    });
                    Ok(json!({"project":projects.read(cx).projects().iter().find(|p| p.id == project_id)}))
                }
                Request::ProjectsDelete { project_id } => {
                    anyhow::ensure!(projects.read(cx).projects().iter().any(|p| p.id == project_id), "Unknown project");
                    anyhow::ensure!(manager.read(cx).state.project_session_ids(project_id).is_empty(), "Project still has terminal tabs");
                    anyhow::ensure!(projects.read(cx).projects().len() > 1, "Cannot delete the last project");
                    projects.update(cx, |store, cx| store.remove(project_id, cx));
                    Ok(json!({"project_id":project_id,"deleted":true}))
                }
                Request::WindowsList {} => Ok(json!({"active_window":manager.read(cx).state.active_window,"windows":manager.read(cx).state.windows.values().collect::<Vec<_>>()})),
                Request::ProfilesList {} => Ok(json!({"profiles":profiles.read(cx).profiles().iter().map(|p| json!({"id":p.id,"name":p.name,"remote":p.remote,"builtin":p.builtin})).collect::<Vec<_>>()})),
                Request::TabsList { project_id } => {
                    let manager = manager.read(cx);
                    let mut tabs = Vec::new();
                    for window in manager.state.windows.values() {
                        for (&project, view) in &window.views {
                            if project_id.is_some_and(|id| id != project) { continue; }
                            for item in view.layout.terminal_items() {
                                let Some(id) = item.session_id else { continue; };
                                tabs.push(json!({"session_id":id,"tab":item,"location":manager.state.locate(id).map(location_json),"session":manager.sessions.iter().find(|s| s.id == id)}));
                            }
                        }
                    }
                    for (&id, tab) in &manager.state.background {
                        if project_id.is_some_and(|project| project != tab.project_id) { continue; }
                        tabs.push(json!({"session_id":id,"tab":tab.item,"project_id":tab.project_id,"location":null,"session":manager.sessions.iter().find(|s| s.id == id)}));
                    }
                    tabs.sort_by_key(|tab| tab["session_id"].as_str().unwrap_or_default().to_owned());
                    Ok(json!({"tabs":tabs}))
                }
                request => {
                    let session = match &request {
                        Request::TabsFocus { session_id } | Request::TabsRename { session_id, .. } | Request::TabsMove { session_id, .. } | Request::TerminalSend { session_id, .. } | Request::PanesSplit { session_id, .. } => Some(*session_id),
                        _ => None,
                    };
                    let location = session.and_then(|id| manager.read(cx).state.locate(id));
                    let requested_window = match &request {
                        Request::TabsCreate { window_id, .. } => *window_id,
                        Request::TabsMove { window_id, .. } => Some(*window_id),
                        _ => location.map(|location| location.window_id),
                    };
                    let window_id = requested_window.or(manager.read(cx).state.active_window)
                        .or_else(|| manager.read(cx).handles.keys().copied().min()).context("No open Eggie window")?;
                    let handle = manager.read(cx).handles.get(&window_id).copied().context("Window is unavailable")?;
                    handle.update(cx, |app, window, cx| {
                        app.pull_window_state(cx);
                        app.reconcile_project_views(cx);
                        app.control_in_window(request, window, cx)
                    })?
                }
            }
        });
        match result {
            Ok(result) => Response::Ok { result },
            Err(error) => Response::error("operation_failed", error.to_string()),
        }
    }

    fn control_in_window(
        &mut self,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        match request {
            Request::TabsCreate {
                project_id,
                profile_id,
                cwd,
                title,
                ..
            } => {
                let project = project_id.unwrap_or(self.workspace_state.active_project);
                anyhow::ensure!(
                    self.workspace_state
                        .projects
                        .iter()
                        .any(|p| p.id == project),
                    "Unknown project"
                );
                self.activate_project(project, cx);
                self.create_controlled_terminal(None, profile_id, cwd, title, window, cx)
            }
            Request::PanesSplit {
                session_id,
                direction,
                profile_id,
            } => {
                let location = self
                    .window_manager
                    .read(cx)
                    .state
                    .locate(session_id)
                    .context("Tab is not in an open window")?;
                anyhow::ensure!(
                    location.window_id == self.window_id,
                    "Tab moved to another window"
                );
                self.activate_session(location.project_id, session_id, window, cx);
                self.create_controlled_terminal(Some(direction), profile_id, None, None, window, cx)
            }
            Request::TabsFocus { session_id } => {
                let manager = self.window_manager.read(cx);
                anyhow::ensure!(
                    manager.state.locate(session_id).is_some()
                        || manager.state.background.contains_key(&session_id),
                    "Unknown tab"
                );
                self.navigate_terminal(session_id, window, cx);
                window.activate_window();
                Ok(json!({"session_id":session_id}))
            }
            Request::TabsRename { session_id, title } => {
                anyhow::ensure!(title.len() <= 512, "Title exceeds 512 bytes");
                let title = (!title.trim().is_empty()).then(|| title.trim().to_owned());
                self.publish_window_state(cx);
                self.window_manager.update(cx, |manager, cx| {
                    manager.rename_session(session_id, title, cx)
                })?;
                self.pull_window_state(cx);
                Ok(json!({"session_id":session_id}))
            }
            Request::TabsMove {
                session_id,
                group_id,
                index,
                ..
            } => {
                let manager = self.window_manager.read(cx);
                let project = manager
                    .state
                    .locate(session_id)
                    .map(|l| l.project_id)
                    .or_else(|| {
                        manager
                            .state
                            .background
                            .get(&session_id)
                            .map(|tab| tab.project_id)
                    })
                    .context("Unknown tab")?;
                let view = manager.state.windows[&self.window_id]
                    .views
                    .get(&project)
                    .context("Project is unavailable in target window")?;
                let group_id = group_id.unwrap_or(view.active_group_id);
                anyhow::ensure!(
                    view.layout.find_group(group_id).is_some(),
                    "Unknown pane in target project"
                );
                let target = eggie_domain::PresentationTarget {
                    window_id: self.window_id,
                    project_id: project,
                    group_id,
                    insertion_index: index,
                    split: None,
                };
                self.window_manager
                    .update(cx, |manager, cx| {
                        if let Some(source) = manager.state.locate(session_id) {
                            manager.move_item(source, target, cx)
                        } else {
                            manager.restore(session_id, target, cx)
                        }
                    })
                    .context("Cannot move tab")?;
                self.pull_window_state(cx);
                Ok(
                    json!({"session_id":session_id,"location":self.window_manager.read(cx).state.locate(session_id).map(location_json)}),
                )
            }
            Request::TerminalSend {
                session_id,
                text,
                submit,
            } => {
                anyhow::ensure!(text.len() <= 48 * 1024, "Text exceeds 48 KiB");
                let session = self
                    .window_manager
                    .read(cx)
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .context("Unknown terminal")?;
                anyhow::ensure!(
                    session.status == SessionStatus::Running,
                    "Terminal process has exited"
                );
                anyhow::ensure!(
                    !text.contains(['\x1b', '\0']),
                    "Use literal text without terminal control sequences"
                );
                anyhow::ensure!(
                    submit || !text.contains(['\r', '\n']),
                    "Multiline input requires submit=true"
                );
                anyhow::ensure!(
                    self.enqueue_terminal_paste(session_id, text),
                    "Terminal input is unavailable"
                );
                if submit {
                    anyhow::ensure!(
                        self.enqueue_terminal_input(session_id, b"\r".to_vec()),
                        "Enter could not be queued"
                    );
                }
                Ok(json!({"session_id":session_id,"queued":true,"submitted":submit}))
            }
            _ => anyhow::bail!("Unsupported window operation"),
        }
    }

    fn create_controlled_terminal(
        &mut self,
        split: Option<Direction>,
        profile_id: Option<eggie_domain::ProfileId>,
        cwd: Option<PathBuf>,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Value> {
        if let Some(id) = profile_id {
            anyhow::ensure!(
                self.profile_store.read(cx).profile(id).is_some(),
                "Unknown profile"
            );
        }
        let project = self.workspace_state.active_project_data();
        let mut profile = self
            .profile_store
            .read(cx)
            .resolve(profile_id.or(project.default_profile_id))
            .clone();
        if let Some(remote) = &profile.remote {
            anyhow::ensure!(
                self.settings
                    .read(cx)
                    .config()
                    .remote_install_consent
                    .contains_key(&remote.consent_key()),
                "Choose remote integration consent in Eggie before creating this SSH profile through automation"
            );
        }
        if let Some(cwd) = cwd {
            if profile.remote.is_none() {
                validate_root(&cwd)?;
            } else {
                anyhow::ensure!(
                    cwd.to_string_lossy().len() <= 4096
                        && !cwd.to_string_lossy().contains(['\0', '\n', '\r']),
                    "Invalid remote directory"
                );
            }
            profile.cwd = Some(cwd);
        }
        if let Some(title) = &title {
            anyhow::ensure!(title.len() <= 512, "Title exceeds 512 bytes");
        }
        let before: HashSet<_> = self.session_feed.sessions.iter().map(|s| s.id).collect();
        let group = self.workspace_state.workspace().active_group_id;
        self.new_terminal_with_profile(group, split, None, profile, window, cx);
        let id = self
            .session_feed
            .sessions
            .iter()
            .find(|session| !before.contains(&session.id))
            .map(|s| s.id)
            .context("Terminal could not be created")?;
        if let Some(title) = title {
            let (group, item) = self
                .workspace_state
                .workspace()
                .layout
                .find_terminal_item(id)
                .context("Created tab has no placement")?;
            self.set_custom_title(
                group,
                item,
                (!title.trim().is_empty()).then(|| title.trim().to_owned()),
                cx,
            );
            self.publish_window_state(cx);
        }
        Ok(
            json!({"session_id":id,"location":self.window_manager.read(cx).state.locate(id).map(location_json)}),
        )
    }
}

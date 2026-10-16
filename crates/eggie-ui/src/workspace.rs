//! Application-wide window registry, session catalog, and one atomic restore snapshot.
mod session_streams;
use crate::app::EggieApp;
use crate::settings::TerminalExitBehavior;
pub(crate) use eggie_domain::WindowProjectView;
use eggie_domain::{
    BackgroundTerminal, ItemLocation, PresentationTarget, PresentationWorkspace, Project,
    SessionId, TabItem, WindowId, WindowPresentation,
};
use eggie_ipc::DaemonClient;
use eggie_protocol::{ClientRequest, DaemonResponse, SessionSummary};
use gpui::{App, Context, Entity, Global, WindowHandle};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub(crate) const LEFT_SIDEBAR_DEFAULT_WIDTH: f32 = 230.;
pub(crate) const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 300.;
const WORKSPACE_FILE: &str = "workspace-v2.json";

#[derive(Clone, Copy)]
pub(crate) struct SidebarRestoreState {
    pub left_collapsed: bool,
    pub right_collapsed: bool,
    pub left_width: f32,
    pub right_width: f32,
}
impl Default for SidebarRestoreState {
    fn default() -> Self {
        Self {
            left_collapsed: false,
            right_collapsed: false,
            left_width: LEFT_SIDEBAR_DEFAULT_WIDTH,
            right_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
        }
    }
}
impl From<&WindowPresentation> for SidebarRestoreState {
    fn from(w: &WindowPresentation) -> Self {
        Self {
            left_collapsed: w.left_sidebar_collapsed,
            right_collapsed: w.right_sidebar_collapsed,
            left_width: w.left_sidebar_width,
            right_width: w.right_sidebar_width,
        }
    }
}

pub(crate) struct WindowManager {
    pub state: PresentationWorkspace,
    pub sessions: Vec<SessionSummary>,
    pub port_connections: Vec<eggie_protocol::PortMappingsSnapshot>,
    terminal_exit_behavior: TerminalExitBehavior,
    pub progress: HashMap<SessionId, eggie_protocol::TerminalProgressUpdate>,
    streams: HashSet<SessionId>,
    stream_liveness: HashMap<SessionId, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    stream_notify_scheduled: bool,
    pub notification_routes: crate::app::NotificationRoutes,
    pub pending_termination: HashSet<SessionId>,
    pub reconnect_errors: HashMap<SessionId, String>,
    pub pending_projects: HashSet<eggie_domain::ProjectId>,
    pub terminating_all: bool,
    retired_sessions: HashSet<SessionId>,
    pub handles: HashMap<WindowId, WindowHandle<EggieApp>>,
    project_names: HashMap<eggie_domain::ProjectId, String>,
    pub project_store: Option<Entity<crate::project_store::ProjectStore>>,
    pub revisions: HashMap<WindowId, u64>,
    published: HashMap<WindowId, WindowPresentation>,
    history: Vec<PresentationChange>,
    revision: u64,
    save_scheduled: bool,
    pub quitting: bool,
    pub error: Option<String>,
    pub connection_error: Option<String>,
    path: std::path::PathBuf,
}
struct PresentationChange {
    before: PresentationWorkspace,
    after: PresentationWorkspace,
}

#[derive(Clone)]
pub(crate) struct SharedWindows(pub Entity<WindowManager>);
impl Global for SharedWindows {}

impl WindowManager {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<SharedWindows>().0.clone()
    }
    pub fn load() -> Self {
        let path = eggie_storage::data_file_path(WORKSPACE_FILE);
        Self::load_from(path)
    }
    fn load_from(path: std::path::PathBuf) -> Self {
        let (state, error) = match eggie_storage::load_json_outcome::<PresentationWorkspace>(&path)
        {
            eggie_storage::LoadOutcome::Loaded(state) if state.version == 2 => (state, None),
            eggie_storage::LoadOutcome::Missing => (PresentationWorkspace::default(), None),
            _ => {
                eggie_storage::quarantine_corrupt_file(&path);
                (PresentationWorkspace::default(), Some("The saved window layout could not be read; the original file was preserved.".into()))
            }
        };
        let published = state
            .windows
            .iter()
            .map(|(id, state)| (*id, state.clone()))
            .collect();
        Self {
            state,
            sessions: Vec::new(),
            port_connections: Vec::new(),
            terminal_exit_behavior: TerminalExitBehavior::default(),
            progress: HashMap::new(),
            streams: HashSet::new(),
            stream_liveness: HashMap::new(),
            stream_notify_scheduled: false,
            notification_routes: std::rc::Rc::new(std::cell::RefCell::new(HashMap::new())),
            pending_termination: HashSet::new(),
            reconnect_errors: HashMap::new(),
            pending_projects: HashSet::new(),
            terminating_all: false,
            retired_sessions: HashSet::new(),
            handles: HashMap::new(),
            project_names: HashMap::new(),
            project_store: None,
            revisions: HashMap::new(),
            published,
            history: Vec::new(),
            revision: 0,
            save_scheduled: false,
            quitting: false,
            error,
            connection_error: None,
            path,
        }
    }
    pub fn register(
        &mut self,
        id: WindowId,
        handle: WindowHandle<EggieApp>,
        cx: &mut Context<Self>,
    ) {
        self.handles.insert(id, handle);
        cx.notify();
    }
    pub fn capture(
        &mut self,
        mut state: WindowPresentation,
        expected: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Result<u64, ()> {
        let id = state.id;
        if let Some(expected) = expected {
            if !self.state.windows.contains_key(&id)
                || self.revisions.get(&id).copied().unwrap_or(0) != expected
            {
                return Err(());
            }
        }
        if state
            .views
            .values()
            .flat_map(|v| v.layout.terminal_items())
            .filter_map(|t| t.session_id)
            .any(|session| {
                self.state
                    .locate(session)
                    .is_some_and(|location| location.window_id != id)
            })
        {
            return Err(());
        }
        if state.number == 0 {
            state.number = self.state.next_window_number;
            self.state.next_window_number += 1;
        }
        if self.state.windows.get(&id) != Some(&state) && !self.quitting {
            self.state.windows.insert(id, state);
            let placed: HashSet<_> = self
                .state
                .windows
                .values()
                .flat_map(|w| w.views.values())
                .flat_map(|v| v.layout.terminal_items())
                .filter_map(|t| t.session_id)
                .collect();
            self.state.background.retain(|id, _| !placed.contains(id));
            self.changed(cx);
        }
        Ok(self.revisions.get(&id).copied().unwrap_or(0))
    }
    fn remember(&mut self, before: PresentationWorkspace) {
        self.history.push(PresentationChange {
            before,
            after: self.state.clone(),
        });
        if self.history.len() > 30 {
            self.history.remove(0);
        }
    }
    pub fn include_created_window_in_undo(&mut self, created: WindowId, source: ItemLocation) {
        let Some(change) = self.history.last_mut() else {
            return;
        };
        let was_empty = change.before.windows.get(&created).is_some_and(|window| {
            window
                .views
                .values()
                .all(|view| view.layout.item_count() == 0)
        });
        let moved_here = change.after.windows.get(&created).is_some_and(|window| {
            window.views.values().any(|view| {
                view.layout.group_ids().iter().any(|id| {
                    view.layout.find_group(*id).is_some_and(|group| {
                        group.items.iter().any(|item| item.id == source.item_id)
                    })
                })
            })
        });
        if was_empty && moved_here && change.before.item(source).is_some() {
            change.before.windows.remove(&created);
            change.before.active_window = Some(source.window_id);
        }
    }
    pub fn move_item(
        &mut self,
        source: ItemLocation,
        target: PresentationTarget,
        cx: &mut Context<Self>,
    ) -> Option<ItemLocation> {
        let before = self.state.clone();
        let placed = self.state.move_item(source, target)?;
        self.remember(before);
        self.changed(cx);
        Some(placed)
    }
    pub fn background_item(&mut self, source: ItemLocation, cx: &mut Context<Self>) -> bool {
        let before = self.state.clone();
        if !self.state.send_to_background(source) {
            return false;
        }
        self.remember(before);
        self.changed(cx);
        true
    }
    pub fn restore(
        &mut self,
        session: SessionId,
        target: PresentationTarget,
        cx: &mut Context<Self>,
    ) -> Option<ItemLocation> {
        let before = self.state.clone();
        let placed = self.state.restore_background(session, target)?;
        self.remember(before);
        self.changed(cx);
        Some(placed)
    }
    pub fn discard_empty_window(&mut self, id: WindowId, cx: &mut Context<Self>) {
        if self.state.windows.get(&id).is_some_and(|window| {
            window
                .views
                .values()
                .all(|view| view.layout.item_count() == 0)
        }) {
            self.state.close_window(id);
            self.handles.remove(&id);
            self.changed(cx);
        }
    }
    pub fn close(&mut self, id: WindowId, cx: &mut Context<Self>) {
        if self.quitting {
            self.handles.remove(&id);
            return;
        }
        if self.state.windows.contains_key(&id) {
            let before = self.state.clone();
            self.state.close_window(id);
            self.remember(before);
            self.changed(cx);
            self.persist();
        }
        self.handles.remove(&id);
    }
    pub fn undo(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(change) = self.history.pop() else {
            return false;
        };
        let current = self.state.clone();
        let changed: HashSet<_> = change
            .before
            .windows
            .keys()
            .chain(change.after.windows.keys())
            .copied()
            .filter(|id| change.before.windows.get(id) != change.after.windows.get(id))
            .collect();
        // Undo only the windows touched by this operation; later windows and process events survive.
        for id in changed {
            self.state.windows.remove(&id);
            if let Some(window) = change.before.windows.get(&id) {
                for session in window
                    .views
                    .values()
                    .flat_map(|view| view.layout.terminal_items())
                    .filter_map(|item| item.session_id)
                {
                    self.state.remove_session(session);
                }
                self.state.windows.insert(id, window.clone());
            }
        }
        for (&id, old) in &change.before.background {
            if change.after.background.get(&id) != Some(old) && self.state.locate(id).is_none() {
                self.state.background.insert(id, old.clone());
            }
        }
        self.state.background.retain(|id, _| {
            !self.state.windows.values().any(|window| {
                window.views.values().any(|view| {
                    view.layout
                        .terminal_items()
                        .any(|item| item.session_id == Some(*id))
                })
            })
        });
        // Terminals created after the checkpoint remain discoverable rather than being discarded.
        for window in current.windows.values() {
            for (&project_id, view) in &window.views {
                for item in view.layout.terminal_items() {
                    if let Some(session) = item.session_id {
                        if self.state.locate(session).is_none()
                            && !self.state.background.contains_key(&session)
                        {
                            self.state.background.insert(
                                session,
                                BackgroundTerminal {
                                    project_id,
                                    item: item.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }
        self.state.osc_cursors = current.osc_cursors;
        self.state.active_window = change
            .before
            .active_window
            .filter(|id| self.state.windows.contains_key(id))
            .or(self
                .state
                .active_window
                .filter(|id| self.state.windows.contains_key(id)));
        self.changed(cx);
        true
    }
    pub fn remove_session(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.retired_sessions.insert(id);
        self.reconnect_errors.remove(&id);
        if let Some(alive) = self.stream_liveness.remove(&id) {
            alive.store(false, std::sync::atomic::Ordering::Release);
        }
        self.streams.remove(&id);
        self.state.remove_session(id);
        self.sessions.retain(|s| s.id != id);
        self.progress.remove(&id);
        self.notification_routes
            .borrow_mut()
            .retain(|_, route| route.session_id != id);
        for previous in &mut self.history {
            previous.before.remove_session(id);
            previous.after.remove_session(id);
        }
        self.changed(cx);
    }
    pub fn rename_session(
        &mut self,
        id: SessionId,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if let Some(location) = self.state.locate(id) {
            self.state
                .windows
                .get_mut(&location.window_id)
                .unwrap()
                .views
                .get_mut(&location.project_id)
                .unwrap()
                .layout
                .set_custom_title(location.group_id, location.item_id, title);
        } else if let Some(tab) = self.state.background.get_mut(&id) {
            tab.item.custom_title = title;
        } else {
            anyhow::bail!("Unknown tab");
        }
        self.changed(cx);
        Ok(())
    }
    pub fn reconnect(&mut self, id: SessionId, client: DaemonClient, cx: &mut Context<Self>) {
        if !self.pending_termination.insert(id) {
            return;
        }
        self.reconnect_errors.remove(&id);
        let executor = cx.background_executor().clone();
        let request_client = client.clone();
        cx.notify();
        cx.spawn(async move |manager, cx| {
            let result = executor
                .spawn(async move {
                    request_client.request(ClientRequest::ReconnectSession { session_id: id })
                })
                .await;
            let replaced = manager
                .update(cx, |manager, cx| {
                    manager.pending_termination.remove(&id);
                    match result {
                        Ok(DaemonResponse::SessionCreated { session }) => {
                            manager.replace_session(id, session, cx);
                            true
                        }
                        Ok(DaemonResponse::Error { message }) => {
                            manager.reconnect_errors.insert(id, message);
                            cx.notify();
                            false
                        }
                        Err(error) => {
                            manager.reconnect_errors.insert(id, error.to_string());
                            cx.notify();
                            false
                        }
                        _ => {
                            manager
                                .reconnect_errors
                                .insert(id, "Unexpected reconnect response".into());
                            cx.notify();
                            false
                        }
                    }
                })
                .unwrap_or(false);
            if replaced {
                let _ = executor
                    .spawn(
                        async move { client.request(ClientRequest::Terminate { session_id: id }) },
                    )
                    .await;
            }
        })
        .detach();
    }

    fn replace_session(
        &mut self,
        previous: SessionId,
        session: SessionSummary,
        cx: &mut Context<Self>,
    ) {
        // Keep item identity, custom title, profile, group and selection while replacing the process.
        self.state.remove_session(session.id);
        for window in self.state.windows.values_mut() {
            for view in window.views.values_mut() {
                if let Some((group_id, item_id)) = view.layout.find_terminal_item(previous) {
                    let item = view
                        .layout
                        .find_group_mut(group_id)
                        .unwrap()
                        .items
                        .iter_mut()
                        .find(|item| item.id == item_id)
                        .unwrap();
                    item.session_id = Some(session.id);
                    item.title.clone_from(&session.title);
                }
            }
        }
        if let Some(mut background) = self.state.background.remove(&previous) {
            background.item.session_id = Some(session.id);
            background.item.title.clone_from(&session.title);
            self.state.background.insert(session.id, background);
        }
        if self.state.locate(session.id).is_none()
            && !self.state.background.contains_key(&session.id)
        {
            self.state.background.insert(
                session.id,
                BackgroundTerminal {
                    project_id: session.project_id,
                    item: TabItem::terminal(session.id, session.title.clone()),
                },
            );
        }
        self.sessions.retain(|existing| existing.id != session.id);
        self.sessions.push(session);
        self.remove_session(previous, cx);
    }

    pub fn terminate(&mut self, id: SessionId, client: DaemonClient, cx: &mut Context<Self>) {
        self.pending_termination.insert(id);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |manager, cx| {
            let result = executor
                .spawn(async move { client.request(ClientRequest::Terminate { session_id: id }) })
                .await;
            let _ = manager.update(cx, |manager, cx| {
                manager.pending_termination.remove(&id);
                match result {
                    Ok(DaemonResponse::Ok) => manager.remove_session(id, cx),
                    Ok(DaemonResponse::Error { message }) => {
                        manager.error = Some(message);
                        cx.notify();
                    }
                    Err(error) => {
                        manager.error = Some(error.to_string());
                        cx.notify();
                    }
                    _ => {}
                }
            });
        })
        .detach();
    }
    pub fn set_terminal_exit_behavior(
        &mut self,
        behavior: TerminalExitBehavior,
        client: DaemonClient,
        cx: &mut Context<Self>,
    ) {
        self.terminal_exit_behavior = behavior;
        self.reap_exited_sessions(client, cx);
    }

    fn reap_exited_sessions(&mut self, client: DaemonClient, cx: &mut Context<Self>) {
        if self.terminal_exit_behavior != TerminalExitBehavior::RemoveTab {
            return;
        }
        let exited: Vec<_> = self
            .sessions
            .iter()
            .filter(|session| {
                matches!(session.status, eggie_protocol::SessionStatus::Exited { .. })
                    && !session.needs_remote_recovery()
                    && !self.pending_termination.contains(&session.id)
            })
            .map(|session| session.id)
            .collect();
        // Reuse acknowledged removal so the daemon also releases the finished terminal's buffers.
        for id in exited {
            self.terminate(id, client.clone(), cx);
        }
    }
    pub fn set_sessions(&mut self, mut sessions: Vec<SessionSummary>, cx: &mut Context<Self>) {
        // An older ListSessions response must not reintroduce a process explicitly terminated here.
        sessions.retain(|session| !self.retired_sessions.contains(&session.id));
        self.stream_liveness.retain(|id, alive| {
            let live = sessions.iter().any(|session| session.id == *id);
            if !live {
                alive.store(false, std::sync::atomic::Ordering::Release);
            }
            live
        });
        self.streams
            .retain(|id| self.stream_liveness.contains_key(id));
        if self.sessions == sessions {
            return;
        }
        if let Some(store) = self.project_store.clone() {
            for session in &sessions {
                if !store
                    .read(cx)
                    .projects()
                    .iter()
                    .any(|project| project.id == session.project_id)
                {
                    let mut project = Project::from_root(session.initial_directory.clone());
                    project.id = session.project_id;
                    store.update(cx, |store, cx| store.add(project, cx));
                }
            }
            let projects = store.read(cx).projects().to_vec();
            self.reconcile_projects(&projects, cx);
        }
        let mut presentation_changed = false;
        for session in &sessions {
            if self.state.locate(session.id).is_none()
                && !self.state.background.contains_key(&session.id)
            {
                self.state.background.insert(
                    session.id,
                    BackgroundTerminal {
                        project_id: session.project_id,
                        item: TabItem::terminal(session.id, &session.title),
                    },
                );
                presentation_changed = true;
            }
            for window in self.state.windows.values_mut() {
                for view in window.views.values_mut() {
                    presentation_changed |= view
                        .layout
                        .update_terminal_title(session.id, &session.title);
                }
            }
            if let Some(tab) = self.state.background.get_mut(&session.id) {
                tab.item.title.clone_from(&session.title);
            }
        }
        self.sessions = sessions;
        if presentation_changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }
    pub fn reconcile_projects(&mut self, projects: &[Project], cx: &mut Context<Self>) {
        self.project_names = projects.iter().map(|p| (p.id, p.name.clone())).collect();
        let before = self.state.clone();
        self.state.reconcile_projects(projects);
        if self.state != before {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }
    pub fn window_label(&self, id: WindowId) -> String {
        let Some(window) = self.state.windows.get(&id) else {
            return "Eggie".into();
        };
        format!(
            "Eggie · {} · {}",
            self.project_names
                .get(&window.active_project)
                .map(String::as_str)
                .unwrap_or("Project"),
            window.number
        )
    }
    pub fn focused(&mut self, id: WindowId, cx: &mut Context<Self>) {
        if self.state.active_window != Some(id) {
            self.state.active_window = Some(id);
            self.schedule_save(cx);
            cx.notify();
        }
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        for (id, window) in &self.state.windows {
            if self.published.get(id) != Some(window) {
                self.revisions.insert(*id, self.revision);
                self.published.insert(*id, window.clone());
            }
        }
        self.published
            .retain(|id, _| self.state.windows.contains_key(id));
        self.schedule_save(cx);
        cx.notify();
    }
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.save_scheduled || self.quitting {
            return;
        }
        self.save_scheduled = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |manager, cx| {
            executor.timer(Duration::from_millis(150)).await;
            let _ = manager.update(cx, |manager, _| {
                manager.save_scheduled = false;
                manager.persist();
            });
        })
        .detach();
    }
    pub fn persist(&self) {
        if let Err(error) = eggie_storage::save_json_atomic(&self.path, &self.state) {
            eprintln!("failed to save window workspace: {error:#}");
        }
    }
    pub fn start_polling(entity: &Entity<Self>, client: DaemonClient, cx: &mut App) {
        let executor = cx.background_executor().clone();
        let entity = entity.downgrade();
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            loop {
                let stream_client = client.clone();
                let client = client.clone();
                let response = executor
                    .spawn(async move { client.request(ClientRequest::ListSessions) })
                    .await;
                let result = entity.update(cx, |manager, cx| match response {
                    Ok(DaemonResponse::Sessions { sessions }) => {
                        manager.connection_error = None;
                        manager.set_sessions(sessions, cx);
                        manager.reap_exited_sessions(stream_client.clone(), cx);
                        manager.ensure_streams(stream_client, cx);
                    }
                    Err(error) => {
                        manager.connection_error = Some(error.to_string());
                        cx.notify();
                    }
                    _ => {}
                });
                if result.is_err() {
                    break;
                }
                executor.timer(Duration::from_millis(500)).await;
            }
        })
        .detach();
    }

    pub fn merge_port_snapshot(&mut self, snapshot: eggie_protocol::PortMappingsSnapshot) -> bool {
        if let Some(current) = self
            .port_connections
            .iter_mut()
            .find(|c| c.connection_id == snapshot.connection_id)
        {
            if current.revision > snapshot.revision || *current == snapshot {
                return false;
            }
            *current = snapshot;
        } else {
            self.port_connections.push(snapshot);
            self.port_connections
                .sort_by(|a, b| a.destination.cmp(&b.destination));
        }
        true
    }
}

#[cfg(test)]
mod tests;

use super::*;

pub(super) struct WorkspaceState {
    pub(super) projects: Vec<Project>,
    pub(super) project_views: HashMap<ProjectId, WindowProjectView>,
    pub(super) active_project: ProjectId,
    pub(super) collapsed_projects: HashSet<ProjectId>,
}

pub(super) struct SidebarState {
    pub(super) project_row_bounds: Rc<RefCell<HashMap<ProjectId, Bounds<Pixels>>>>,
    pub(super) project_drag: ProjectDragState,
    pub(super) project_popover: Option<ProjectPopoverState>,
    pub(super) sidebar_row_bounds: Rc<RefCell<HashMap<SessionId, Bounds<Pixels>>>>,
    pub(super) sidebar_preview: Option<SidebarPreviewState>,
    pub(super) hovered_sidebar_session: Option<SessionId>,
    pub(super) sidebar_preview_revision: u64,
    pub(super) marquee_states: Rc<RefCell<HashMap<String, MarqueeState>>>,
    pub(super) project_icon_states: RefCell<HashMap<ProjectId, ProjectIconState>>,
    pub(super) tab_icon_states: RefCell<HashMap<SessionId, TabIconState>>,
    pub(super) sidebar_status_rows: RefCell<HashMap<SessionId, SidebarStatusRowState>>,
}

pub(super) struct SessionFeedState {
    pub(super) snapshot_watchers: HashSet<SessionId>,
    pub(super) progress_watchers: HashSet<SessionId>,
    pub(super) osc_watchers: HashSet<SessionId>,
    pub(super) progress_updates: HashMap<SessionId, TerminalProgressUpdate>,
    pub(super) pending_progress_updates: HashMap<SessionId, TerminalProgressUpdate>,
    pub(super) sessions: Vec<SessionSummary>,
    pub(super) detached_sessions: Vec<SessionSummary>,
    pub(super) snapshots: HashMap<SessionId, Arc<TerminalSnapshot>>,
    pub(super) session_inspections: HashMap<SessionId, SessionInspection>,
    pub(super) session_inspection_errors: HashMap<SessionId, String>,
    pub(super) process_metrics_system: System,
    pub(super) shell_integration: HashMap<SessionId, TerminalShellIntegrationState>,
}

impl WorkspaceState {
    /// The per-window view for the active project. Invariant: `active_project` always names a
    /// project that has a view (maintained by `reconcile_project_views` and the always-present Home
    /// project), so this never returns `None` in practice — matching the old direct-index behaviour.
    pub(super) fn workspace(&self) -> &WindowProjectView {
        self.project_views
            .get(&self.active_project)
            .expect("active_project must always have a window view")
    }

    pub(super) fn workspace_mut(&mut self) -> &mut WindowProjectView {
        self.project_views
            .get_mut(&self.active_project)
            .expect("active_project must always have a window view")
    }

    /// The shared `Project` (id/name/root) for the active project, from the local mirror.
    pub(super) fn active_project_data(&self) -> &Project {
        self.projects
            .iter()
            .find(|project| project.id == self.active_project)
            .expect("active_project must exist in the project mirror")
    }
}

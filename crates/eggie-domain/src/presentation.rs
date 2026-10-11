//! Presentation locations are independent of terminal process lifetimes.
use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowProjectView {
    pub layout: LayoutNode,
    pub active_group_id: GroupId,
}

impl WindowProjectView {
    pub fn empty() -> Self {
        let layout = LayoutNode::empty();
        Self {
            active_group_id: layout.first_group_id(),
            layout,
        }
    }

    pub fn repair_active_group(&mut self) {
        if self.layout.find_group(self.active_group_id).is_none() {
            self.active_group_id = self.layout.first_group_id();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPresentation {
    pub id: WindowId,
    pub number: u32,
    pub project_picker: bool,
    pub active_project: ProjectId,
    pub views: HashMap<ProjectId, WindowProjectView>,
    pub collapsed_projects: HashSet<ProjectId>,
    pub left_sidebar_collapsed: bool,
    pub right_sidebar_collapsed: bool,
    pub left_sidebar_width: f32,
    pub right_sidebar_width: f32,
    pub frame: Option<WindowFrame>,
}

impl WindowPresentation {
    pub fn new(id: WindowId, project_id: ProjectId) -> Self {
        Self {
            id,
            number: 0,
            project_picker: false,
            active_project: project_id,
            views: HashMap::from([(project_id, WindowProjectView::empty())]),
            collapsed_projects: HashSet::new(),
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            left_sidebar_width: 230.,
            right_sidebar_width: 300.,
            frame: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BackgroundTerminal {
    pub project_id: ProjectId,
    pub item: TabItem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemLocation {
    pub window_id: WindowId,
    pub project_id: ProjectId,
    pub group_id: GroupId,
    pub item_id: ItemId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentationTarget {
    pub window_id: WindowId,
    pub project_id: ProjectId,
    pub group_id: GroupId,
    pub insertion_index: Option<usize>,
    pub split: Option<Direction>,
}

/// All open windows are saved together; background terminals retain their tab metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresentationWorkspace {
    pub version: u32,
    pub next_window_number: u32,
    pub osc_cursors: HashMap<SessionId, u64>,
    pub windows: BTreeMap<WindowId, WindowPresentation>,
    pub background: BTreeMap<SessionId, BackgroundTerminal>,
    pub active_window: Option<WindowId>,
}

impl Default for PresentationWorkspace {
    fn default() -> Self {
        Self {
            version: 2,
            next_window_number: 1,
            osc_cursors: HashMap::new(),
            windows: BTreeMap::new(),
            background: BTreeMap::new(),
            active_window: None,
        }
    }
}

impl PresentationWorkspace {
    pub fn project_session_ids(&self, project_id: ProjectId) -> HashSet<SessionId> {
        self.windows
            .values()
            .filter_map(|w| w.views.get(&project_id))
            .flat_map(|v| v.layout.terminal_items())
            .filter_map(|item| item.session_id)
            .chain(
                self.background
                    .iter()
                    .filter(|(_, t)| t.project_id == project_id)
                    .map(|(id, _)| *id),
            )
            .collect()
    }
    pub fn locate(&self, session_id: SessionId) -> Option<ItemLocation> {
        for window in self.windows.values() {
            for (&project_id, view) in &window.views {
                if let Some((group_id, item_id)) = view.layout.find_terminal_item(session_id) {
                    return Some(ItemLocation {
                        window_id: window.id,
                        project_id,
                        group_id,
                        item_id,
                    });
                }
            }
        }
        None
    }

    pub fn item(&self, location: ItemLocation) -> Option<&TabItem> {
        self.windows
            .get(&location.window_id)?
            .views
            .get(&location.project_id)?
            .layout
            .find_group(location.group_id)?
            .items
            .iter()
            .find(|item| item.id == location.item_id)
    }

    fn insert(&mut self, target: PresentationTarget, item: TabItem) -> Option<ItemLocation> {
        let window = self.windows.get_mut(&target.window_id)?;
        let view = window.views.get_mut(&target.project_id)?;
        let group = view.layout.find_group_mut(target.group_id)?;
        let item_id = item.id;
        let group_id = if let Some(direction) = target.split {
            view.layout
                .split_group_direction(target.group_id, direction, item)?
        } else {
            group.insert_item(target.insertion_index.unwrap_or(group.items.len()), item);
            target.group_id
        };
        view.active_group_id = group_id;
        window.active_project = target.project_id;
        window.collapsed_projects.remove(&target.project_id);
        self.active_window = Some(target.window_id);
        Some(ItemLocation {
            window_id: target.window_id,
            project_id: target.project_id,
            group_id,
            item_id,
        })
    }

    /// Prepare both layouts in a candidate before committing, so a failed drop cannot lose a tab.
    pub fn move_item(
        &mut self,
        source: ItemLocation,
        target: PresentationTarget,
    ) -> Option<ItemLocation> {
        if source.project_id != target.project_id {
            return None;
        }
        let mut candidate = self.clone();
        let source_view = candidate
            .windows
            .get_mut(&source.window_id)?
            .views
            .get_mut(&source.project_id)?;
        let item = source_view
            .layout
            .close_item(source.group_id, source.item_id)?;
        source_view.repair_active_group();
        let placed = candidate.insert(target, item)?;
        *self = candidate;
        Some(placed)
    }

    pub fn restore_background(
        &mut self,
        session_id: SessionId,
        target: PresentationTarget,
    ) -> Option<ItemLocation> {
        let mut candidate = self.clone();
        let terminal = candidate.background.remove(&session_id)?;
        if terminal.project_id != target.project_id || candidate.locate(session_id).is_some() {
            return None;
        }
        let placed = candidate.insert(target, terminal.item)?;
        *self = candidate;
        Some(placed)
    }

    pub fn send_to_background(&mut self, source: ItemLocation) -> bool {
        let Some(item) = self.item(source).cloned() else {
            return false;
        };
        let Some(session_id) = item.session_id else {
            return false;
        };
        let view = self
            .windows
            .get_mut(&source.window_id)
            .unwrap()
            .views
            .get_mut(&source.project_id)
            .unwrap();
        view.layout.close_item(source.group_id, source.item_id);
        view.repair_active_group();
        self.background.insert(
            session_id,
            BackgroundTerminal {
                project_id: source.project_id,
                item,
            },
        );
        true
    }

    pub fn close_window(&mut self, window_id: WindowId) -> Option<WindowPresentation> {
        let window = self.windows.remove(&window_id)?;
        for (&project_id, view) in &window.views {
            for item in view.layout.terminal_items() {
                if let Some(session_id) = item.session_id {
                    self.background.insert(
                        session_id,
                        BackgroundTerminal {
                            project_id,
                            item: item.clone(),
                        },
                    );
                }
            }
        }
        if self.active_window == Some(window_id) {
            self.active_window = self.windows.keys().next().copied();
        }
        Some(window)
    }

    pub fn remove_session(&mut self, session_id: SessionId) {
        self.osc_cursors.remove(&session_id);
        self.background.remove(&session_id);
        for window in self.windows.values_mut() {
            for view in window.views.values_mut() {
                if let Some((group, item)) = view.layout.find_terminal_item(session_id) {
                    view.layout.close_item(group, item);
                    view.repair_active_group();
                }
            }
        }
    }

    /// Repair invalid or duplicate saved placements while retaining ended tabs as explicit history.
    pub fn reconcile_projects(&mut self, projects: &[Project]) {
        let known: HashSet<_> = projects.iter().map(|p| p.id).collect();
        let mut seen = HashSet::new();
        for window in self.windows.values_mut() {
            let removed: Vec<_> = window
                .views
                .keys()
                .copied()
                .filter(|id| !known.contains(id))
                .collect();
            for id in removed {
                if let Some(view) = window.views.remove(&id) {
                    for item in view.layout.terminal_items() {
                        if let Some(session_id) = item.session_id {
                            self.background
                                .entry(session_id)
                                .or_insert(BackgroundTerminal {
                                    project_id: id,
                                    item: item.clone(),
                                });
                        }
                    }
                }
            }
            for project in projects {
                window
                    .views
                    .entry(project.id)
                    .or_insert_with(WindowProjectView::empty);
            }
            for view in window.views.values_mut() {
                let duplicate: Vec<_> = view
                    .layout
                    .terminal_items()
                    .filter_map(|item| item.session_id.filter(|id| !seen.insert(*id)))
                    .collect();
                for session_id in duplicate {
                    if let Some((group, item)) = view.layout.find_terminal_item(session_id) {
                        view.layout.close_item(group, item);
                    }
                }
                view.repair_active_group();
            }
            if !known.contains(&window.active_project)
                && let Some(project) = projects.first()
            {
                window.active_project = project.id;
            }
            window.collapsed_projects.retain(|id| known.contains(id));
        }
        self.background.retain(|id, _| !seen.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (
        PresentationWorkspace,
        ItemLocation,
        PresentationTarget,
        SessionId,
    ) {
        let project = ProjectId::new_v4();
        let mut a = WindowPresentation::new(WindowId::new_v4(), project);
        let b = WindowPresentation::new(WindowId::new_v4(), project);
        let session = SessionId::new_v4();
        let mut tab = TabItem::terminal_with_profile(session, "shell", ProfileId::new_v4());
        tab.custom_title = Some("Building".into());
        let group = a.views[&project].active_group_id;
        let source = ItemLocation {
            window_id: a.id,
            project_id: project,
            group_id: group,
            item_id: tab.id,
        };
        a.views
            .get_mut(&project)
            .unwrap()
            .layout
            .find_group_mut(group)
            .unwrap()
            .add_item(tab);
        let target = PresentationTarget {
            window_id: b.id,
            project_id: project,
            group_id: b.views[&project].active_group_id,
            insertion_index: None,
            split: None,
        };
        let mut ws = PresentationWorkspace::default();
        ws.windows.insert(a.id, a);
        ws.windows.insert(b.id, b);
        (ws, source, target, session)
    }
    #[test]
    fn move_preserves_identity_profile_and_title() {
        let (mut ws, source, target, session) = fixture();
        let item = ws.item(source).unwrap().clone();
        let placed = ws.move_item(source, target).unwrap();
        assert_eq!(ws.item(placed), Some(&item));
        assert_eq!(ws.locate(session), Some(placed));
        assert!(ws.item(source).is_none());
    }
    #[test]
    fn invalid_destination_does_not_change_source() {
        let (mut ws, source, mut target, _) = fixture();
        let before = ws.clone();
        target.group_id = GroupId::new_v4();
        assert!(ws.move_item(source, target).is_none());
        assert_eq!(ws, before);
        target.window_id = WindowId::new_v4();
        assert!(ws.move_item(source, target).is_none());
        assert_eq!(ws, before);
    }
    #[test]
    fn close_then_restore_retains_original_terminal() {
        let (mut ws, source, target, session) = fixture();
        let item = ws.item(source).unwrap().clone();
        ws.close_window(source.window_id).unwrap();
        assert_eq!(ws.background[&session].item, item);
        let placed = ws.restore_background(session, target).unwrap();
        assert_eq!(ws.item(placed), Some(&item));
        assert!(!ws.background.contains_key(&session));
    }
    #[test]
    fn failed_background_restore_is_atomic() {
        let (mut ws, source, mut target, session) = fixture();
        ws.send_to_background(source);
        let before = ws.clone();
        target.project_id = ProjectId::new_v4();
        assert!(ws.restore_background(session, target).is_none());
        assert_eq!(ws, before);
    }
    #[test]
    fn split_transfer_repairs_active_group() {
        let (mut ws, source, mut target, session) = fixture();
        target.split = Some(Direction::Right);
        let placed = ws.move_item(source, target).unwrap();
        assert_ne!(placed.group_id, target.group_id);
        assert_eq!(ws.locate(session), Some(placed));
        for w in ws.windows.values() {
            for v in w.views.values() {
                assert!(v.layout.find_group(v.active_group_id).is_some());
            }
        }
    }
    #[test]
    fn ended_tabs_and_multiple_windows_round_trip() {
        let (ws, _, _, session) = fixture();
        // Liveness is not an ownership rule: an ended session remains an explicit saved tab.
        let encoded = serde_json::to_vec(&ws).unwrap();
        let loaded: PresentationWorkspace = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(loaded, ws);
        assert!(loaded.locate(session).is_some());
        assert_eq!(loaded.windows.len(), 2);
    }
}

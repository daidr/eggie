//! 跨启动的工作区恢复状态。
//!
//! 把窗口的工作区（各项目的布局树、活跃项目、侧栏状态）持久化到 `workspace.json`，
//! 下次冷启动时重建布局并把 daemon 中幸存的会话认领回原来的 tab/分屏。已不存在的会话
//! （被终止、或 daemon 本身重启过）会被 prune；一个都不剩时调用方回落为开新终端。
//!
//! v1 只持久化单一快照（多窗口时最后写入者胜，恢复一个窗口）；崩溃/强杀不落盘，
//! 回落为既有的冷启动认领行为。

use std::collections::{HashMap, HashSet};

use eggie_domain::{GroupId, LayoutNode, Project, ProjectId};
use serde::{Deserialize, Serialize};

use eggie_storage as persist;

const WORKSPACE_FILE: &str = "workspace.json";

/// 侧栏默认宽度（serde 缺省与 `SidebarRestoreState::default` 的单一真源）。
pub(crate) const LEFT_SIDEBAR_DEFAULT_WIDTH: f32 = 230.;
pub(crate) const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 300.;

/// Per-window, per-project view state. The `Project` itself (id/name/root) lives in the shared
/// [`crate::project_store::ProjectStore`] so it syncs across windows; the layout tree and the
/// active group are local to this window because sessions are not shared between windows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WindowProjectView {
    pub layout: LayoutNode,
    pub active_group_id: GroupId,
}

impl WindowProjectView {
    /// A view for a project that currently has no terminal in this window: an empty layout whose
    /// single empty group is the active group (and a valid drop target for the first new terminal).
    pub fn empty() -> Self {
        let layout = LayoutNode::empty();
        let active_group_id = layout.first_group_id();
        Self {
            layout,
            active_group_id,
        }
    }
}

/// 落盘的工作区快照。除 `active_project`/`views` 外的字段都带 serde default，
/// 以便未来加字段时旧文件仍能读取。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WorkspaceRestoreState {
    pub active_project: ProjectId,
    pub views: HashMap<ProjectId, WindowProjectView>,
    #[serde(default)]
    pub left_sidebar_collapsed: bool,
    #[serde(default)]
    pub right_sidebar_collapsed: bool,
    #[serde(default = "default_left_sidebar_width")]
    pub left_sidebar_width: f32,
    #[serde(default = "default_right_sidebar_width")]
    pub right_sidebar_width: f32,
}

fn default_left_sidebar_width() -> f32 {
    LEFT_SIDEBAR_DEFAULT_WIDTH
}

fn default_right_sidebar_width() -> f32 {
    RIGHT_SIDEBAR_DEFAULT_WIDTH
}

/// 恢复时应用到窗口的侧栏状态（从 [`WorkspaceRestoreState`] 提取，便于在 views 被 move 走后使用）。
#[derive(Clone, Copy, Debug, PartialEq)]
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

impl WorkspaceRestoreState {
    /// 读取上次保存的工作区状态。文件缺失或损坏都返回 `None`（调用方回落为默认行为）。
    pub fn load() -> Option<Self> {
        persist::load_json(&persist::data_file_path(WORKSPACE_FILE))
    }

    /// 原子写入磁盘。失败只打日志、不阻断退出流程（重启时回落为默认行为）。
    pub fn save(&self) {
        if let Err(error) = persist::save_json_atomic(&persist::data_file_path(WORKSPACE_FILE), self)
        {
            eprintln!("failed to persist workspace state: {error:#}");
        }
    }

    /// 提取侧栏状态（views 会被消费，先取出轻量副本）。
    pub fn sidebar_states(&self) -> SidebarRestoreState {
        SidebarRestoreState {
            left_collapsed: self.left_sidebar_collapsed,
            right_collapsed: self.right_sidebar_collapsed,
            left_width: self.left_sidebar_width,
            right_width: self.right_sidebar_width,
        }
    }

    /// 收集持久化布局中引用的全部会话 id（恢复时逐个向 daemon 认领）。
    pub fn persisted_session_ids(&self) -> impl Iterator<Item = eggie_domain::SessionId> + '_ {
        self.views
            .values()
            .flat_map(|view| view.layout.terminal_items())
            .filter_map(|item| item.session_id)
    }

    /// 恢复前的清洗：
    /// - 丢弃已不在项目列表中的项目视图（项目被用户删除）；
    /// - prune 掉会话已不存在的 tab（`live` 是认领成功的会话 id 集合）；
    /// - 若活跃项目的视图已没有存活 tab，则改选第一个还有存活 tab 的项目；
    /// - 没有任何存活 tab 时返回 `false`（调用方应回落为开新终端）。
    pub fn prune_to_live(&mut self, live: &HashSet<eggie_domain::SessionId>, projects: &[Project]) -> bool {
        let known: HashSet<ProjectId> = projects.iter().map(|project| project.id).collect();
        self.views.retain(|project_id, _| known.contains(project_id));

        for view in self.views.values_mut() {
            let dead: Vec<_> = view
                .layout
                .terminal_items()
                .filter(|item| item.session_id.is_some_and(|id| !live.contains(&id)))
                .filter_map(|item| item.session_id)
                .collect();
            for session_id in dead {
                // find_terminal_item 定位 (group, item)；close_item 会修好 active_item_id。
                if let Some((group_id, item_id)) = view.layout.find_terminal_item(session_id) {
                    view.layout.close_item(group_id, item_id);
                }
            }
            // close_item can remove an emptied group from the tree; if the active group was one of
            // them, point at the first remaining group (always valid — close_item keeps a placeholder).
            // Mirrors the runtime path in `prune_session_from_layouts`.
            if view.layout.find_group(view.active_group_id).is_none() {
                view.active_group_id = view.layout.first_group_id();
            }
        }

        let has_live_tabs = |view: &WindowProjectView| {
            view.layout
                .terminal_items()
                .any(|item| item.session_id.is_some_and(|id| live.contains(&id)))
        };

        if !self
            .views
            .get(&self.active_project)
            .is_some_and(has_live_tabs)
            && let Some(project_id) = projects
                .iter()
                .map(|project| project.id)
                .find(|id| self.views.get(id).is_some_and(has_live_tabs))
        {
            self.active_project = project_id;
        }

        self.views.values().any(has_live_tabs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eggie_domain::TabItem;

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "eggie-workspace-restore-test-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);

        let project = Project::new("Test".to_owned());
        let session = eggie_domain::SessionId::new_v4();
        let mut views = HashMap::new();
        views.insert(
            project.id,
            WindowProjectView {
                layout: LayoutNode::group(TabItem::terminal(session, "zsh")),
                active_group_id: GroupId::new_v4(),
            },
        );
        let state = WorkspaceRestoreState {
            active_project: project.id,
            views,
            left_sidebar_collapsed: true,
            right_sidebar_collapsed: false,
            left_sidebar_width: 250.,
            right_sidebar_width: 320.,
        };

        // 直接走 persist 原语验证序列化往返（save() 写死应用数据目录，不适合测试）。
        persist::save_json_atomic(&path, &state).expect("save must succeed");
        let loaded: WorkspaceRestoreState = persist::load_json(&path).expect("must load");
        assert_eq!(loaded.active_project, state.active_project);
        assert_eq!(loaded.left_sidebar_collapsed, true);
        assert_eq!(loaded.right_sidebar_width, 320.);
        assert_eq!(loaded.views.len(), 1);
        assert!(loaded.persisted_session_ids().any(|id| id == session));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn prunes_dead_tabs_and_unknown_projects() {
        let live_session = eggie_domain::SessionId::new_v4();
        let dead_session = eggie_domain::SessionId::new_v4();
        let project = Project::new("Keep".to_owned());
        let deleted_project = Project::new("Gone".to_owned());

        let mut views = HashMap::new();
        let mut keep_layout = LayoutNode::group(TabItem::terminal(live_session, "live"));
        let group_id = keep_layout.first_group_id();
        keep_layout
            .find_group_mut(group_id)
            .unwrap()
            .add_item(TabItem::terminal(dead_session, "dead"));
        views.insert(
            project.id,
            WindowProjectView {
                layout: keep_layout,
                active_group_id: group_id,
            },
        );
        views.insert(
            deleted_project.id,
            WindowProjectView {
                layout: LayoutNode::group(TabItem::terminal(dead_session, "dead")),
                active_group_id: GroupId::new_v4(),
            },
        );

        let mut state = WorkspaceRestoreState {
            active_project: deleted_project.id, // 指向已删除项目，应被纠正
            views,
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            left_sidebar_width: 230.,
            right_sidebar_width: 230.,
        };

        let live = HashSet::from([live_session]);
        assert!(state.prune_to_live(&live, &[project.clone()]));
        assert_eq!(state.views.len(), 1, "deleted project's view must be dropped");
        assert_eq!(state.active_project, project.id, "active project must fall back to one with live tabs");
        let view = &state.views[&project.id];
        let remaining: Vec<_> = view.layout.terminal_items().collect();
        assert_eq!(remaining.len(), 1, "dead tab must be pruned");
        assert_eq!(remaining[0].session_id, Some(live_session));
    }

    #[test]
    fn prune_returns_false_when_nothing_survives() {
        let session = eggie_domain::SessionId::new_v4();
        let project = Project::new("P".to_owned());
        let mut views = HashMap::new();
        views.insert(
            project.id,
            WindowProjectView {
                layout: LayoutNode::group(TabItem::terminal(session, "dead")),
                active_group_id: GroupId::new_v4(),
            },
        );
        let mut state = WorkspaceRestoreState {
            active_project: project.id,
            views,
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            left_sidebar_width: 230.,
            right_sidebar_width: 230.,
        };
        // daemon 重启过：没有任何会话幸存。
        assert!(!state.prune_to_live(&HashSet::new(), &[project]));
    }

    #[test]
    fn prune_repairs_dangling_active_group_id() {
        use eggie_domain::Direction;

        let live_session = eggie_domain::SessionId::new_v4();
        let dead_session = eggie_domain::SessionId::new_v4();
        let project = Project::new("P".to_owned());

        // Build a split: group A (active, holds the dead session) | group B (holds the live one).
        let mut layout = LayoutNode::group(TabItem::terminal(dead_session, "dead"));
        let group_a = layout.first_group_id();
        let group_b = layout
            .split_group_direction(group_a, Direction::Right, TabItem::terminal(live_session, "live"))
            .expect("split must create group B");

        let mut views = HashMap::new();
        views.insert(
            project.id,
            WindowProjectView {
                layout,
                active_group_id: group_a, // active group holds the dead session
            },
        );
        let mut state = WorkspaceRestoreState {
            active_project: project.id,
            views,
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            left_sidebar_width: 230.,
            right_sidebar_width: 230.,
        };

        let live = HashSet::from([live_session]);
        assert!(state.prune_to_live(&live, &[project.clone()]));
        let view = &state.views[&project.id];
        // The dead session's group was pruned away; active_group_id must not dangle.
        assert!(
            view.layout.find_group(view.active_group_id).is_some(),
            "active_group_id must point at a group that still exists"
        );
        assert_eq!(view.active_group_id, group_b);
        // The live session's tab survives.
        assert!(view
            .layout
            .terminal_items()
            .any(|item| item.session_id == Some(live_session)));
    }

    #[test]
    fn prune_returns_false_for_empty_views() {
        let project = Project::new("P".to_owned());
        let mut state = WorkspaceRestoreState {
            active_project: project.id,
            views: HashMap::new(),
            left_sidebar_collapsed: false,
            right_sidebar_collapsed: false,
            left_sidebar_width: 230.,
            right_sidebar_width: 230.,
        };
        // No views at all (corrupt/empty file): nothing to restore.
        assert!(!state.prune_to_live(&HashSet::new(), &[project]));
    }
}

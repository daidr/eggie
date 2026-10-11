use super::*;
use eggie_domain::{GroupId, ProjectId};
use gpui::{AppContext, TestAppContext};

fn manager(cx: &mut TestAppContext) -> (Entity<WindowManager>, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!("eggie-window-test-{}.json", WindowId::new_v4()));
    (cx.new(|_| WindowManager::load_from(path.clone())), path)
}

fn setup(
    cx: &mut TestAppContext,
    manager: &Entity<WindowManager>,
) -> (
    WindowId,
    WindowId,
    SessionId,
    ItemLocation,
    PresentationTarget,
) {
    let project = ProjectId::new_v4();
    let mut a = WindowPresentation::new(WindowId::new_v4(), project);
    let b = WindowPresentation::new(WindowId::new_v4(), project);
    let session = SessionId::new_v4();
    let tab =
        TabItem::terminal_with_profile(session, "original", eggie_domain::ProfileId::new_v4());
    let group = a.views[&project].active_group_id;
    let source = ItemLocation {
        window_id: a.id,
        project_id: project,
        group_id: group,
        item_id: tab.id,
    };
    let target = PresentationTarget {
        window_id: b.id,
        project_id: project,
        group_id: b.views[&project].active_group_id,
        insertion_index: None,
        split: None,
    };
    a.views
        .get_mut(&project)
        .unwrap()
        .layout
        .find_group_mut(group)
        .unwrap()
        .add_item(tab);
    manager.update(cx, |m, cx| {
        m.capture(a.clone(), None, cx).unwrap();
        m.capture(b.clone(), None, cx).unwrap();
    });
    (a.id, b.id, session, source, target)
}

#[gpui::test]
fn a_stale_window_cannot_reclaim_a_moved_terminal(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (a, b, session, source, target) = setup(cx, &manager);
    let (stale, revision) =
        manager.read_with(cx, |m, _| (m.state.windows[&a].clone(), m.revisions[&a]));
    manager.update(cx, |m, cx| {
        m.move_item(source, target, cx).unwrap();
        assert!(m.capture(stale, Some(revision), cx).is_err());
        assert_eq!(m.state.locate(session).unwrap().window_id, b);
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn unrelated_windows_keep_their_revision_and_number(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (a, b, _, _, _) = setup(cx, &manager);
    manager.update(cx, |m, cx| {
        let revision = m.revisions[&b];
        let number = m.state.windows[&b].number;
        let mut changed = m.state.windows[&a].clone();
        changed.left_sidebar_width = 310.;
        let expected = m.revisions[&a];
        m.capture(changed, Some(expected), cx).unwrap();
        assert_eq!(m.revisions[&b], revision);
        assert_eq!(m.state.windows[&b].number, number);
        assert_ne!(m.state.windows[&a].number, number);
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn close_undo_and_termination_have_distinct_lifetimes(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (a, _, session, _, _) = setup(cx, &manager);
    manager.update(cx, |m, cx| {
        m.close(a, cx);
        assert!(m.state.background.contains_key(&session));
        assert!(m.undo(cx));
        assert_eq!(m.state.locate(session).unwrap().window_id, a);
        m.close(a, cx);
        m.remove_session(session, cx);
        assert!(m.undo(cx));
        assert!(m.state.locate(session).is_none());
        assert!(!m.state.background.contains_key(&session));
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn one_atomic_snapshot_restores_every_window_and_background_tab(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (a, b, session, _, _) = setup(cx, &manager);
    manager.update(cx, |m, cx| {
        m.close(a, cx);
        m.state.active_window = Some(b);
        m.persist();
    });
    let loaded = WindowManager::load_from(path.clone());
    assert_eq!(loaded.state.windows.len(), 1);
    assert_eq!(loaded.state.active_window, Some(b));
    assert!(loaded.state.background.contains_key(&session));
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn failed_transfers_do_not_create_undo_entries(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (_, _, _, source, mut target) = setup(cx, &manager);
    target.group_id = GroupId::new_v4();
    manager.update(cx, |m, cx| {
        assert!(m.move_item(source, target, cx).is_none());
        assert!(!m.undo(cx));
    });
    std::fs::remove_file(path).ok();
}
#[gpui::test]
fn undo_preserves_later_windows_terminals_and_event_cursors(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (a, _, session, source, target) = setup(cx, &manager);
    manager.update(cx, |m, cx| {
        m.move_item(source, target, cx).unwrap();
        let extra = WindowPresentation::new(WindowId::new_v4(), source.project_id);
        let extra_id = extra.id;
        m.capture(extra, None, cx).unwrap();
        let new_session = SessionId::new_v4();
        let view = m
            .state
            .windows
            .get_mut(&a)
            .unwrap()
            .views
            .get_mut(&source.project_id)
            .unwrap();
        view.layout
            .find_group_mut(view.active_group_id)
            .unwrap()
            .add_item(TabItem::terminal(new_session, "later"));
        m.state.osc_cursors.insert(session, 42);
        assert!(m.undo(cx));
        assert!(m.state.windows.contains_key(&extra_id));
        assert!(
            m.state.locate(new_session).is_some() || m.state.background.contains_key(&new_session)
        );
        assert_eq!(m.state.osc_cursors[&session], 42);
        assert_eq!(m.state.locate(session).unwrap().window_id, a);
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn a_stale_catalog_cannot_resurrect_a_terminated_terminal(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (_, _, session, source, _) = setup(cx, &manager);
    let summary = SessionSummary {
        id: session,
        project_id: source.project_id,
        title: "shell".into(),
        initial_directory: "/tmp".into(),
        current_directory: "/tmp".into(),
        reported_location: None,
        shell_pid: 100,
        current_process: eggie_protocol::ProcessSummary {
            pid: 100,
            name: "sh".into(),
            command_line: None,
        },
        active_command: None,
        program_status: None,
        status: eggie_protocol::SessionStatus::Running,
    };
    manager.update(cx, |m, cx| {
        m.set_sessions(vec![summary.clone()], cx);
        m.remove_session(session, cx);
        m.set_sessions(vec![summary], cx);
        assert!(m.sessions.is_empty());
        assert!(m.state.locate(session).is_none());
        assert!(!m.state.background.contains_key(&session));
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn undoing_a_tear_out_closes_the_window_created_by_that_gesture(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (a, b, session, source, target) = setup(cx, &manager);
    manager.update(cx, |m, cx| {
        m.move_item(source, target, cx).unwrap();
        m.include_created_window_in_undo(b, source);
        assert!(m.undo(cx));
        assert!(!m.state.windows.contains_key(&b));
        assert_eq!(m.state.locate(session).unwrap().window_id, a);
        assert_eq!(m.state.active_window, Some(a));
    });
    std::fs::remove_file(path).ok();
}

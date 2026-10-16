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
        remote: None,
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

fn session_summary(id: SessionId, project: ProjectId, exited: bool) -> SessionSummary {
    SessionSummary {
        id,
        project_id: project,
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
        remote: None,
        status: if exited {
            eggie_protocol::SessionStatus::Exited { code: Some(0) }
        } else {
            eggie_protocol::SessionStatus::Running
        },
    }
}

/// Exercise the real async acknowledgement path without touching the user's daemon.
fn exit_client(
    count: usize,
    response: DaemonResponse,
) -> (
    DaemonClient,
    std::thread::JoinHandle<Vec<SessionId>>,
    std::path::PathBuf,
) {
    use std::os::unix::net::UnixListener;
    let path = std::path::PathBuf::from(format!("/tmp/egg-exit-{}.sock", SessionId::new_v4()));
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut ids = Vec::new();
        while ids.len() < count {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut buffer = Vec::new();
                    let request = eggie_ipc::wire::read_wire_message::<ClientRequest>(
                        &mut stream,
                        &mut buffer,
                    )
                    .unwrap()
                    .unwrap();
                    let ClientRequest::Terminate { session_id } = request else {
                        panic!("unexpected request {request:?}")
                    };
                    ids.push(session_id);
                    eggie_ipc::wire::write_wire_message(&mut stream, &mut buffer, &response)
                        .unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "cleanup request timed out"
                    );
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("{error}"),
            }
        }
        ids
    });
    (DaemonClient::new(path.clone(), "exit-test"), worker, path)
}

#[gpui::test]
fn automatic_exit_removal_releases_only_finished_tabs_and_repairs_selection(
    cx: &mut TestAppContext,
) {
    let (manager, path) = manager(cx);
    let (a, b, session, source, _) = setup(cx, &manager);
    let sibling = SessionId::new_v4();
    let background = SessionId::new_v4();
    let summaries = vec![
        session_summary(session, source.project_id, true),
        session_summary(sibling, source.project_id, false),
        session_summary(background, source.project_id, true),
    ];
    let (client, worker, socket) = exit_client(2, DaemonResponse::Ok);
    manager.update(cx, |m, cx| {
        let mut window = m.state.windows[&a].clone();
        let group = window
            .views
            .get_mut(&source.project_id)
            .unwrap()
            .layout
            .find_group_mut(source.group_id)
            .unwrap();
        group.add_item(TabItem::terminal(sibling, "sibling"));
        group.active_item_id = Some(source.item_id);
        m.capture(window, Some(m.revisions[&a]), cx).unwrap();
        m.set_sessions(summaries.clone(), cx);
        m.reap_exited_sessions(client.clone(), cx);
        // A second poll must not enqueue duplicate cleanup while awaiting acknowledgement.
        m.reap_exited_sessions(client, cx);
        assert_eq!(m.pending_termination.len(), 2);
    });
    cx.run_until_parked();
    let removed = worker.join().unwrap();
    assert_eq!(removed.len(), 2);
    assert!(removed.contains(&session) && removed.contains(&background));
    manager.update(cx, |m, cx| {
        assert!(m.pending_termination.is_empty());
        assert!(m.state.locate(session).is_none());
        assert!(!m.state.background.contains_key(&background));
        assert_eq!(
            m.state.windows[&a].views[&source.project_id]
                .layout
                .find_group(source.group_id)
                .unwrap()
                .active_item()
                .unwrap()
                .session_id,
            Some(sibling)
        );
        assert!(m.state.windows.contains_key(&b));
        m.set_sessions(summaries, cx);
        assert_eq!(
            m.sessions.len(),
            1,
            "stale catalogs cannot resurrect removed tabs"
        );
    });
    std::fs::remove_file(socket).ok();
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn keeping_finished_tabs_and_switching_to_removal_work_across_windows(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (_, b, session, source, target) = setup(cx, &manager);
    let (client, worker, socket) = exit_client(1, DaemonResponse::Ok);
    manager.update(cx, |m, cx| {
        m.move_item(source, target, cx).unwrap();
        m.set_terminal_exit_behavior(TerminalExitBehavior::KeepTab, client.clone(), cx);
        m.set_sessions(vec![session_summary(session, source.project_id, true)], cx);
        m.reap_exited_sessions(client.clone(), cx);
        assert!(m.pending_termination.is_empty());
        assert_eq!(m.state.locate(session).unwrap().window_id, b);
        // A successful empty catalog can mean a daemon restart; absence is not a confirmed exit.
        m.set_sessions(Vec::new(), cx);
        m.set_terminal_exit_behavior(TerminalExitBehavior::RemoveTab, client.clone(), cx);
        assert!(m.state.locate(session).is_some());
        assert!(m.pending_termination.is_empty());
        m.set_sessions(vec![session_summary(session, source.project_id, true)], cx);
        m.reap_exited_sessions(client, cx);
    });
    cx.run_until_parked();
    assert_eq!(worker.join().unwrap(), vec![session]);
    manager.update(cx, |m, cx| {
        assert!(m.state.locate(session).is_none());
        assert!(m.state.windows.contains_key(&b));
        assert!(m.undo(cx));
        assert!(
            m.state.locate(session).is_none(),
            "undo must not revive a finished process"
        );
    });
    std::fs::remove_file(socket).ok();
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn failed_exit_cleanup_keeps_the_tab_for_retry(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (_, _, session, source, _) = setup(cx, &manager);
    let (client, worker, socket) = exit_client(
        1,
        DaemonResponse::Error {
            message: "cleanup failed".into(),
        },
    );
    manager.update(cx, |m, cx| {
        m.set_sessions(vec![session_summary(session, source.project_id, true)], cx);
        m.reap_exited_sessions(client, cx);
    });
    cx.run_until_parked();
    assert_eq!(worker.join().unwrap(), vec![session]);
    manager.read_with(cx, |m, _| {
        assert!(m.pending_termination.is_empty());
        assert!(m.state.locate(session).is_some());
        assert!(m.error.as_deref().unwrap().ends_with("cleanup failed"));
    });
    let (retry, worker, retry_socket) = exit_client(1, DaemonResponse::Ok);
    manager.update(cx, |m, cx| m.reap_exited_sessions(retry, cx));
    cx.run_until_parked();
    assert_eq!(worker.join().unwrap(), vec![session]);
    manager.read_with(cx, |m, _| assert!(m.state.locate(session).is_none()));
    std::fs::remove_file(retry_socket).ok();
    std::fs::remove_file(socket).ok();
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn late_port_snapshots_cannot_undo_a_completed_operation(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let snapshot = eggie_protocol::PortMappingsSnapshot {
        revision: 2,
        registry_revision: 1,
        connection_id: "ssh".into(),
        destination: "test".into(),
        target: eggie_domain::RemoteTarget::new("test".into()),
        connected: true,
        mappings: Vec::new(),
        services: Vec::new(),
        discovery_error: None,
    };
    manager.update(cx, |m, _| {
        m.merge_port_snapshot(snapshot.clone());
        let mut stale = snapshot.clone();
        stale.revision = 1;
        stale.connected = false;
        m.merge_port_snapshot(stale);
        assert!(m.port_connections[0].connected);
        assert_eq!(m.port_connections[0].revision, 2);
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn reconnect_preserves_tab_identity_and_rejects_stale_catalogs(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (window, _, previous, location, _) = setup(cx, &manager);
    let replacement = SessionId::new_v4();
    manager.update(cx, |manager, cx| {
        let profile = manager.state.item(location).unwrap().profile_id;
        manager
            .state
            .windows
            .get_mut(&window)
            .unwrap()
            .views
            .get_mut(&location.project_id)
            .unwrap()
            .layout
            .find_group_mut(location.group_id)
            .unwrap()
            .items
            .iter_mut()
            .find(|item| item.id == location.item_id)
            .unwrap()
            .custom_title = Some("Build server".into());
        let session = session_summary(replacement, location.project_id, false);
        manager.set_sessions(
            vec![
                session_summary(previous, location.project_id, true),
                session.clone(),
            ],
            cx,
        );
        manager.replace_session(previous, session.clone(), cx);
        let placed = manager.state.locate(replacement).unwrap();
        assert_eq!(placed.window_id, window);
        assert_eq!(placed.item_id, location.item_id);
        assert_eq!(placed.group_id, location.group_id);
        assert_eq!(manager.state.item(placed).unwrap().profile_id, profile);
        assert_eq!(
            manager.state.item(placed).unwrap().custom_title.as_deref(),
            Some("Build server")
        );
        assert!(!manager.state.background.contains_key(&replacement));
        manager.set_sessions(
            vec![
                session_summary(previous, location.project_id, true),
                session,
            ],
            cx,
        );
        assert!(
            !manager
                .sessions
                .iter()
                .any(|session| session.id == previous)
        );
        assert_eq!(manager.state.locate(replacement).unwrap(), placed);
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn failed_ssh_tabs_survive_automatic_exit_cleanup(cx: &mut TestAppContext) {
    let (manager, path) = manager(cx);
    let (_, _, id, location, _) = setup(cx, &manager);
    let client = DaemonClient::new("/tmp/eggie-no-requests-expected.sock".into(), "test");
    manager.update(cx, |manager, cx| {
        let mut session = session_summary(id, location.project_id, true);
        session.remote = Some(eggie_protocol::RemoteSessionInfo {
            failure: None,
            target: eggie_domain::RemoteTarget::new("devbox".into()),
            phase: eggie_protocol::RemoteSessionPhase::Failed,
            initial_directory: None,
        });
        manager.set_sessions(vec![session], cx);
        manager.set_terminal_exit_behavior(TerminalExitBehavior::RemoveTab, client, cx);
        assert!(manager.pending_termination.is_empty());
        assert!(manager.state.locate(id).is_some());
    });
    std::fs::remove_file(path).ok();
}

#[gpui::test]
fn reconnect_acknowledgement_replaces_once_and_releases_the_previous_process(
    cx: &mut TestAppContext,
) {
    use std::os::unix::net::UnixListener;
    let (manager, path) = manager(cx);
    let (_, _, previous, location, _) = setup(cx, &manager);
    let next = session_summary(SessionId::new_v4(), location.project_id, false);
    let next_id = next.id;
    let socket = std::path::PathBuf::from(format!("/tmp/egg-retry-{}.sock", SessionId::new_v4()));
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut count = 0;
        while count < 2 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut buffer = Vec::new();
                    let request = eggie_ipc::wire::read_wire_message::<ClientRequest>(
                        &mut stream,
                        &mut buffer,
                    )
                    .unwrap()
                    .unwrap();
                    let response = if count == 0 {
                        assert_eq!(
                            request,
                            ClientRequest::ReconnectSession {
                                session_id: previous
                            }
                        );
                        DaemonResponse::SessionCreated {
                            session: next.clone(),
                        }
                    } else {
                        assert_eq!(
                            request,
                            ClientRequest::Terminate {
                                session_id: previous
                            }
                        );
                        DaemonResponse::Ok
                    };
                    eggie_ipc::wire::write_wire_message(&mut stream, &mut buffer, &response)
                        .unwrap();
                    count += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("{error}"),
            }
        }
    });
    let client = DaemonClient::new(socket.clone(), "reconnect-test");
    manager.update(cx, |manager, cx| {
        manager.reconnect(previous, client.clone(), cx);
        manager.reconnect(previous, client, cx);
    });
    cx.run_until_parked();
    worker.join().unwrap();
    manager.update(cx, |manager, _| {
        assert!(manager.pending_termination.is_empty());
        assert_eq!(
            manager.state.locate(next_id).unwrap().item_id,
            location.item_id
        );
        assert!(manager.state.locate(previous).is_none());
    });
    std::fs::remove_file(socket).ok();
    std::fs::remove_file(path).ok();
}

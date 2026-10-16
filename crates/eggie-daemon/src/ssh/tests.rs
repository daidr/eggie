use super::*;
use connection::SshTransport;
use eggie_protocol::*;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Output};

#[derive(Default)]
struct FakeSsh {
    available: AtomicBool,
    commands: Mutex<Vec<Vec<String>>>,
    fail: Mutex<Option<(String, String)>>,
}
impl SshTransport for FakeSsh {
    fn run(&self, args: &[String]) -> Result<Output> {
        self.commands.lock().push(args.to_vec());
        let operation = args
            .iter()
            .position(|a| a == "-O")
            .map(|i| args[i + 1].as_str());
        let mut status = 0;
        let mut error = String::new();
        if operation == Some("check") && !self.available.load(Ordering::Acquire) {
            status = 255;
            error = "Control socket connect: Connection refused".into();
        }
        let mut fail = self.fail.lock();
        if let Some((op, message)) = fail.as_ref()
            && Some(op.as_str()) == operation
        {
            status = 255;
            error = message.clone();
            fail.take();
        }
        if operation == Some("exit") {
            self.available.store(false, Ordering::Release);
        }
        let stdout = if operation == Some("forward")
            && args.iter().any(|a| a.starts_with("-R127.0.0.1:0:"))
        {
            b"49161\n".to_vec()
        } else {
            Vec::new()
        };
        Ok(Output {
            status: ExitStatus::from_raw(status << 8),
            stdout,
            stderr: error.into_bytes(),
        })
    }
}
fn pool() -> (SshConnections, Arc<FakeSsh>, Arc<SshConnection>) {
    let pool = SshConnections::default();
    let fake = Arc::new(FakeSsh::default());
    fake.available.store(true, Ordering::Release);
    let mut connection = SshConnection::new(
        RemoteTarget::new("ssh-test.invalid".into()),
        pool.store.clone(),
        Vec::new(),
    )
    .unwrap();
    connection.set_test_transport(fake.clone());
    let connection = Arc::new(connection);
    pool.connections.lock().push(connection.clone());
    (pool, fake, connection)
}
fn request() -> PortMappingRequest {
    PortMappingRequest {
        kind: PortForwardKind::Local,
        local_port: None,
        remote_port: Some(3000),
        target_host: "127.0.0.1".into(),
        service: PortServiceKind::Auto,
        remember: false,
    }
}
#[test]
fn automatic_ports_fall_back_without_changing_the_remote_target() {
    let (_, _, connection) = pool();
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    let mut request = request();
    request.remote_port = Some(port);
    let mapping = connection.create(request.clone()).unwrap();
    assert_eq!(mapping.state, PortMappingState::Connected);
    assert_ne!(mapping.local_port, Some(port));
    assert_eq!(mapping.remote_port, Some(port));
    let duplicate = connection.create(request).unwrap();
    assert_eq!(duplicate.id, mapping.id);
    assert_eq!(connection.snapshot().mappings.len(), 1);
}
#[test]
fn changing_a_services_type_reuses_the_mapping_and_updates_its_browser_action() {
    let (_, _, connection) = pool();
    let mut request = request();
    request.remote_port = Some(18081);
    let initial = connection.create(request.clone()).unwrap();
    assert!(initial.browser_url().is_none());
    request.service = PortServiceKind::Http;
    let changed = connection.create(request).unwrap();
    assert_eq!(initial.id, changed.id);
    assert!(changed.browser_url().is_some());
    assert_eq!(connection.snapshot().mappings.len(), 1);
}
#[test]
fn an_explicit_occupied_port_is_preserved_as_an_actionable_failure() {
    let (_, fake, connection) = pool();
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut request = request();
    request.local_port = Some(occupied.local_addr().unwrap().port());
    let failed = connection.create(request).unwrap();
    assert_eq!(failed.state, PortMappingState::Failed);
    assert_eq!(failed.error.unwrap().kind, PortMappingErrorKind::PortInUse);
    assert!(
        !fake
            .commands
            .lock()
            .iter()
            .any(|args| args.iter().any(|a| a == "forward"))
    );
}
#[test]
fn ssh_bind_races_are_retried_for_automatic_ports() {
    let (_, fake, connection) = pool();
    *fake.fail.lock() = Some(("forward".into(), "bind: Address already in use".into()));
    let mapping = connection.create(request()).unwrap();
    assert_eq!(mapping.state, PortMappingState::Connected);
    assert_eq!(
        fake.commands
            .lock()
            .iter()
            .filter(|args| args.iter().any(|a| a == "forward"))
            .count(),
        2
    );
}
#[test]
fn reverse_and_socks_mappings_use_structured_targets_and_allocated_ports() {
    let (_, fake, connection) = pool();
    let mut reverse = request();
    reverse.kind = PortForwardKind::Remote;
    reverse.remote_port = None;
    reverse.local_port = Some(3030);
    reverse.target_host = "::1".into();
    let mapping = connection.create(reverse).unwrap();
    assert_eq!(mapping.remote_port, Some(49161));
    assert_eq!(mapping.local_port, Some(3030));
    assert!(
        fake.commands
            .lock()
            .iter()
            .any(|args| args.iter().any(|a| a == "-R127.0.0.1:0:[::1]:3030"))
    );
    assert_eq!(
        connection.stop(&mapping.id, false).unwrap().state,
        PortMappingState::Stopped
    );
    let mut socks = request();
    socks.kind = PortForwardKind::Dynamic;
    socks.remote_port = None;
    let mapping = connection.create(socks).unwrap();
    assert_eq!(mapping.state, PortMappingState::Connected);
    assert!(mapping.browser_url().is_none());
}
#[test]
fn stop_failure_keeps_the_active_mapping_visible_and_retryable() {
    let (_, fake, connection) = pool();
    let mapping = connection.create(request()).unwrap();
    *fake.fail.lock() = Some(("cancel".into(), "Permission denied".into()));
    let failed = connection.stop(&mapping.id, true).unwrap();
    assert_eq!(failed.state, PortMappingState::Connected);
    assert!(failed.error.is_some());
    assert_eq!(connection.snapshot().mappings.len(), 1);
    assert_eq!(
        connection.stop(&mapping.id, true).unwrap().state,
        PortMappingState::Stopped
    );
    assert!(connection.snapshot().mappings.is_empty());
}
#[test]
fn closing_one_tab_cannot_drop_another_tabs_shared_transport_or_active_mapping() {
    let (pool, fake, connection) = pool();
    let tab = pool.connection(&connection.target).unwrap();
    assert!(Arc::ptr_eq(
        &pool.connection(&connection.target).unwrap(),
        &connection
    ));
    drop(tab);
    assert!(pool.by_id(&connection.id).is_ok());
    let mapping = connection.create(request()).unwrap();
    assert!(pool.by_id(&connection.id).is_ok());
    connection.stop(&mapping.id, true).unwrap();
    // ControlPersist expires idle masters; listing sessions must never race a new tab or mapping.
    fake.available.store(false, Ordering::Release);
    connection.expire_test_cache();
    assert!(!pool.by_id(&connection.id).unwrap().snapshot().connected);
    let mut other = connection.target.clone();
    other.user = Some("other".into());
    assert_ne!(pool.connection(&other).unwrap().id, connection.id);
}
#[test]
fn disconnect_and_reconnect_restore_desired_mappings_but_never_stopped_ones() {
    let (_, fake, connection) = pool();
    let first = connection.create(request()).unwrap();
    let mut other = request();
    other.remote_port = Some(3001);
    let stopped = connection.create(other).unwrap();
    connection.stop(&stopped.id, false).unwrap();
    fake.available.store(false, Ordering::Release);
    connection.expire_test_cache();
    connection.maintain();
    assert_eq!(
        connection.mapping(&first.id).unwrap().state,
        PortMappingState::Disconnected
    );
    fake.available.store(true, Ordering::Release);
    connection.expire_test_cache();
    connection.maintain();
    assert_eq!(
        connection.mapping(&first.id).unwrap().state,
        PortMappingState::Connected
    );
    assert_eq!(
        connection.mapping(&stopped.id).unwrap().state,
        PortMappingState::Stopped
    );
}
#[test]
fn remembered_intents_survive_restart_and_explicit_stop_remains_stopped() {
    let directory = std::env::temp_dir().join(format!("eggie-ports-test-{}", SessionId::new_v4()));
    let path = directory.join("mappings.json");
    let pool = SshConnections::load(Some(path.clone()));
    let fake = Arc::new(FakeSsh::default());
    fake.available.store(true, Ordering::Release);
    let target = RemoteTarget::new("ssh-save-test.invalid".into());
    let mut transport = SshConnection::new(target.clone(), pool.store.clone(), Vec::new()).unwrap();
    transport.set_test_transport(fake);
    let mut remembered = request();
    remembered.remember = true;
    let active = transport.create(remembered.clone()).unwrap();
    let saved = SshConnections::load(Some(path.clone()));
    let restored = saved.connection(&target).unwrap();
    assert_eq!(
        restored.mapping(&active.id).unwrap().state,
        PortMappingState::Disconnected
    );
    transport.stop(&active.id, false).unwrap();
    let saved = SshConnections::load(Some(path.clone()));
    assert_eq!(
        saved
            .connection(&target)
            .unwrap()
            .mapping(&active.id)
            .unwrap()
            .state,
        PortMappingState::Stopped
    );
    transport.remember(&active.id, false).unwrap();
    let saved = SshConnections::load(Some(path));
    assert!(
        saved
            .connection(&target)
            .unwrap()
            .snapshot()
            .mappings
            .is_empty()
    );
    fs::remove_dir_all(directory).ok();
}
#[test]
fn cannot_report_a_remembered_mapping_when_the_intent_store_is_unwritable() {
    let directory = std::env::temp_dir().join(format!("eggie-ports-test-{}", SessionId::new_v4()));
    fs::create_dir_all(&directory).unwrap();
    let store = Arc::new(IntentStore {
        registry_revision: AtomicU64::new(1),
        path: Some(directory.clone()),
        saved: Mutex::new(Vec::new()),
    });
    let mut connection = SshConnection::new(
        RemoteTarget::new("ssh-test.invalid".into()),
        store,
        Vec::new(),
    )
    .unwrap();
    let fake = Arc::new(FakeSsh::default());
    fake.available.store(true, Ordering::Release);
    connection.set_test_transport(fake);
    let mut remembered = request();
    remembered.remember = true;
    let mapping = connection.create(remembered).unwrap();
    assert_eq!(mapping.state, PortMappingState::Connected);
    assert_eq!(
        mapping.error.unwrap().kind,
        PortMappingErrorKind::PersistenceFailed
    );
    let stopped = connection.stop(&mapping.id, true).unwrap();
    assert_eq!(
        stopped.error.unwrap().kind,
        PortMappingErrorKind::PersistenceFailed
    );
    assert_eq!(connection.snapshot().mappings.len(), 1);
    fs::remove_dir_all(directory).ok();
}

#[test]
fn process_inspection_does_not_mix_shells_on_the_same_master() {
    let output = "ROOT 10\nSELF 99\nPID PPID %CPU RSS COMM\n10 1 0 100 sshd\n11 10 0 100 zsh\n12 10 0 100 zsh\n20 11 1 100 node\n21 12 1 100 python\n99 10 0 100 sh\n===EGGIE-LSOF===\nSESSION 11\n";
    let (processes, _) = process_monitor::parse_remote_inspection(output);
    assert_eq!(
        processes.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![11, 20]
    );
}

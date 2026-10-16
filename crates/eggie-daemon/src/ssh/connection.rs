use super::*;
use eggie_protocol::{PortMappingState, PortMappingsSnapshot};
use mappings::MappingRecord;
use std::process::{Command, Output};

pub(super) trait SshTransport: Send + Sync {
    fn run(&self, args: &[String]) -> Result<Output>;
    fn requires_socket(&self) -> bool {
        false
    }
}
struct SystemSsh;
impl SshTransport for SystemSsh {
    fn requires_socket(&self) -> bool {
        true
    }
    fn run(&self, args: &[String]) -> Result<Output> {
        let mut command = Command::new("ssh");
        command.args(args);
        super::command::run(command, Duration::from_secs(8))
    }
}

pub(crate) struct SshConnection {
    pub target: RemoteTarget,
    pub path: PathBuf,
    pub id: String,
    pub(super) revision: AtomicU64,
    pub(super) mappings: Mutex<Vec<MappingRecord>>,
    pub(super) operation: Mutex<()>,
    pub(super) store: Arc<IntentStore>,
    transport: Arc<dyn SshTransport>,
    pub(super) connected: AtomicBool,
    inspection: Mutex<Option<(Instant, Result<String, String>)>>,
    discovery: Mutex<(Vec<eggie_protocol::RemoteService>, Option<String>)>,
    checked: Mutex<Instant>,
    keepalive: Mutex<Instant>,
}
impl SshConnection {
    pub(super) fn new(
        target: RemoteTarget,
        store: Arc<IntentStore>,
        saved: Vec<SavedMapping>,
    ) -> Result<Self> {
        let id = SessionId::new_v4().simple().to_string();
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = control_path(&home, &id[..16]).unwrap_or_else(|| {
            PathBuf::from(format!(
                "/tmp/eggie-{}/ssh/{}",
                unsafe { libc::getuid() },
                &id[..16]
            ))
        });
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            target,
            path,
            id,
            revision: AtomicU64::new(1),
            mappings: Mutex::new(
                saved
                    .into_iter()
                    .filter(|s| s.request.validate().is_ok())
                    .map(MappingRecord::restore)
                    .collect(),
            ),
            operation: Mutex::new(()),
            store,
            transport: Arc::new(SystemSsh),
            connected: AtomicBool::new(false),
            inspection: Mutex::new(None),
            discovery: Mutex::new((Vec::new(), None)),
            checked: Mutex::new(Instant::now() - Duration::from_secs(60)),
            keepalive: Mutex::new(Instant::now()),
        })
    }
    pub(super) fn request(&self, operation: &str, option: Option<String>) -> Result<Output> {
        if operation == "check" && self.transport.requires_socket() && !self.path.exists() {
            bail!("SSH connection unavailable");
        }
        let mut args = vec![
            "-S".into(),
            self.path.display().to_string(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-O".into(),
            operation.into(),
        ];
        if let Some(option) = option {
            args.push(option);
        }
        if let Some(port) = self.target.port {
            args.extend(["-p".into(), port.to_string()]);
        }
        args.extend(["--".into(), self.target.destination()]);
        self.transport.run(&args)
    }
    pub fn run(&self, command: &str) -> Result<String> {
        if self.transport.requires_socket() {
            anyhow::ensure!(
                self.request("check", None)?.status.success(),
                "SSH connection unavailable"
            );
        }
        let mut args = vec![
            "-S".into(),
            self.path.display().to_string(),
            "-o".into(),
            "ControlMaster=no".into(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "ConnectTimeout=5".into(),
            "-o".into(),
            "ServerAliveInterval=3".into(),
            "-o".into(),
            "ServerAliveCountMax=1".into(),
        ];
        if let Some(port) = self.target.port {
            args.extend(["-p".into(), port.to_string()]);
        }
        args.extend(["--".into(), self.target.destination(), command.into()]);
        let output = self.transport.run(&args)?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8(output.stdout).context("invalid SSH output")
    }
    pub fn inspection_output(&self) -> Result<String> {
        let mut cache = self.inspection.lock();
        if let Some((time, result)) = &*cache
            && time.elapsed() < Duration::from_secs(2)
        {
            return result.clone().map_err(|error| anyhow!(error));
        }
        let result = self.run(r#"printf 'ROOT %s\nSELF %s\n' "$PPID" "$$"; ps -u "$(id -u)" -o pid,ppid,pcpu,rss,comm; printf '%s\n' '===EGGIE-LSOF==='; if command -v lsof >/dev/null 2>&1; then lsof -nP -a -u "$(id -un)" -iTCP -sTCP:LISTEN -FpcPn 2>/dev/null || true; elif command -v ss >/dev/null 2>&1; then printf '%s\n' '===EGGIE-SS==='; ss -H -ltnp 2>/dev/null || true; else printf '%s\n' '===EGGIE-DISCOVERY-UNAVAILABLE==='; fi"#).map_err(|e| e.to_string());
        let discovery = match &result {
            Ok(output) => (
                parse_services(output),
                output
                    .contains("===EGGIE-DISCOVERY-UNAVAILABLE===")
                    .then(|| "discovery_unavailable".into()),
            ),
            Err(error) => (Vec::new(), Some(error.clone())),
        };
        {
            let mut current = self.discovery.lock();
            if *current != discovery {
                *current = discovery;
                self.revision.fetch_add(1, Ordering::Release);
            }
        }
        *cache = Some((Instant::now(), result.clone()));
        result.map_err(|error| anyhow!(error))
    }
    pub fn snapshot(&self) -> PortMappingsSnapshot {
        self.maintain();
        let records = self.mappings.lock();
        let (services, discovery_error) = self.discovery.lock().clone();
        PortMappingsSnapshot {
            revision: self.revision.load(Ordering::Acquire),
            registry_revision: self.store.registry_revision.load(Ordering::Acquire),
            connection_id: self.id.clone(),
            destination: self.target.consent_key(),
            target: self.target.clone(),
            connected: self.connected.load(Ordering::Acquire),
            mappings: records
                .iter()
                .map(|record| record.mapping.clone())
                .collect(),
            services,
            discovery_error,
        }
    }
    pub fn maintain(&self) {
        let Some(_operation) = self.operation.try_lock() else {
            return;
        };
        let mut checked = self.checked.lock();
        if checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        *checked = Instant::now();
        drop(checked);
        let connected = self
            .request("check", None)
            .is_ok_and(|o| o.status.success());
        self.update_connected(connected);
        if !connected {
            return;
        }
        let reconnect = self
            .mappings
            .lock()
            .iter()
            .filter(|r| r.enabled && r.mapping.state == PortMappingState::Disconnected)
            .map(|r| r.mapping.id.clone())
            .collect::<Vec<_>>();
        for id in reconnect {
            let _ = self.start_locked(&id);
        }
        // A short ControlPersist bounds orphan masters after a daemon crash. Active mappings keep
        // one lightweight channel alive periodically, even with no terminal tab open.
        if self.has_enabled_mappings() && self.keepalive.lock().elapsed() >= Duration::from_secs(20)
        {
            let _ = self.run("true");
            *self.keepalive.lock() = Instant::now();
        }
    }
    pub(super) fn update_connected(&self, connected: bool) {
        let mut records = self.mappings.lock();
        let mut changed = self.connected.swap(connected, Ordering::AcqRel) != connected;
        if !connected {
            for record in records.iter_mut() {
                if record.mapping.state == PortMappingState::Connected {
                    record.mapping.state = PortMappingState::Disconnected;
                    record.spec = None;
                    changed = true;
                }
            }
        }
        if changed {
            self.revision.fetch_add(1, Ordering::Release);
        }
        drop(records);
        if changed && !connected {
            *self.inspection.lock() = None;
            *self.discovery.lock() = (Vec::new(), None);
        }
    }
}

fn parse_services(output: &str) -> Vec<eggie_protocol::RemoteService> {
    use eggie_protocol::RemoteService;
    let mut services = Vec::new();
    if let Some((_, data)) = output.split_once("===EGGIE-SS===") {
        let owned: HashSet<u32> = output
            .split("===EGGIE-LSOF===")
            .next()
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_whitespace().next()?.parse().ok())
            .collect();
        for line in data.lines() {
            let pid = line
                .split("pid=")
                .nth(1)
                .and_then(|pid| pid.split(',').next())
                .and_then(|pid| pid.parse::<u32>().ok());
            if !pid.is_some_and(|pid| owned.contains(&pid)) {
                continue;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            let Some(endpoint) = fields.get(3) else {
                continue;
            };
            let Some((address, port)) = endpoint.rsplit_once(':') else {
                continue;
            };
            let Ok(port) = port.parse::<u16>() else {
                continue;
            };
            let process = line
                .split("((\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .unwrap_or("TCP");
            services.push(RemoteService {
                port,
                address: address.trim_matches(['[', ']']).into(),
                process: process.into(),
            });
        }
    } else if let Some((_, data)) = output.split_once("===EGGIE-LSOF===") {
        let mut names = HashMap::new();
        let mut pid = None;
        for line in data.lines() {
            if let Some(id) = line.strip_prefix('p') {
                pid = id.parse::<u32>().ok();
            }
            if let Some(name) = line.strip_prefix('c')
                && let Some(pid) = pid
            {
                names.insert(pid, name.to_owned());
            }
        }
        for port in process_monitor::parse_lsof_ports(data) {
            services.push(RemoteService {
                port: port.port,
                address: port.address.trim_matches(['[', ']']).into(),
                process: names
                    .get(&port.pid)
                    .cloned()
                    .unwrap_or_else(|| "TCP".into()),
            });
        }
    }
    services.retain(|s| s.port != 0 && s.port != 22 && s.process != "sshd" && s.process != "ssh");
    services
        .sort_by(|a, b| (&a.port, &a.process, &a.address).cmp(&(&b.port, &b.process, &b.address)));
    services.dedup_by(|a, b| a.port == b.port && a.process == b.process);
    services
}

#[cfg(test)]
impl SshConnection {
    pub(super) fn set_test_transport(&mut self, transport: Arc<dyn SshTransport>) {
        self.transport = transport;
    }
    pub(super) fn expire_test_cache(&self) {
        *self.checked.lock() = Instant::now() - Duration::from_secs(60);
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    #[test]
    fn lsof_and_ss_find_services_with_no_dependency_on_one_shells_process_tree() {
        let services = parse_services(
            "ROOT 1\n===EGGIE-LSOF===\np42\ncnode\nPTCP\nn127.0.0.1:3000\np43\ncpostgres\nPTCP\nn[::1]:5432\np10\ncsshd\nPTCP\nn*:22\n",
        );
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].process, "node");
        let services = parse_services(
            "PID PPID %CPU RSS COMM\n42 1 0 100 node\n===EGGIE-LSOF===\n===EGGIE-SS===\nLISTEN 0 128 [::1]:3000 *:* users:((\"node\",pid=42,fd=12))\nLISTEN 0 128 127.0.0.1:5432 *:*\nLISTEN 0 128 127.0.0.1:9000 *:* users:((\"other-user\",pid=43,fd=12))\n",
        );
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].address, "::1");
    }
}

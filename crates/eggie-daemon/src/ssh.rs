//! Shared authenticated SSH transports and their user-visible port mappings.
mod command;
pub(super) mod config;
mod connection;
#[cfg(test)]
mod integration_tests;
mod mappings;
#[cfg(test)]
mod tests;
use super::*;
pub(super) use connection::SshConnection;
use mappings::SavedMapping;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct SavedConnection {
    target: RemoteTarget,
    mappings: Vec<SavedMapping>,
}

struct IntentStore {
    path: Option<PathBuf>,
    registry_revision: AtomicU64,
    saved: Mutex<Vec<SavedConnection>>,
}
impl IntentStore {
    fn save(&self, target: &RemoteTarget, mappings: Vec<SavedMapping>) -> Result<()> {
        let mut saved = self.saved.lock();
        saved.retain(|item| &item.target != target);
        if !mappings.is_empty() {
            saved.push(SavedConnection {
                target: target.clone(),
                mappings,
            });
        }
        if let Some(path) = &self.path {
            eggie_storage::save_json_atomic(path, &*saved)?;
        }
        Ok(())
    }
}

pub(super) struct SshConnections {
    connections: Mutex<Vec<Arc<SshConnection>>>,
    store: Arc<IntentStore>,
}
impl Default for SshConnections {
    fn default() -> Self {
        Self::load(None)
    }
}
impl SshConnections {
    pub fn load(path: Option<PathBuf>) -> Self {
        let saved = path
            .as_ref()
            .map(
                |p| match eggie_storage::load_json_outcome::<Vec<SavedConnection>>(p) {
                    eggie_storage::LoadOutcome::Loaded(saved) => saved,
                    eggie_storage::LoadOutcome::Missing => Vec::new(),
                    eggie_storage::LoadOutcome::Corrupt(_) => {
                        eggie_storage::quarantine_corrupt_file(p);
                        Vec::new()
                    }
                },
            )
            .unwrap_or_default();
        let store = Arc::new(IntentStore {
            registry_revision: AtomicU64::new(1),
            path,
            saved: Mutex::new(saved.clone()),
        });
        let connections = saved
            .iter()
            .filter(|saved| saved.target.is_usable() && !saved.mappings.is_empty())
            .filter_map(|saved| {
                SshConnection::new(saved.target.clone(), store.clone(), saved.mappings.clone())
                    .map(Arc::new)
                    .ok()
            })
            .collect();
        Self {
            connections: Mutex::new(connections),
            store,
        }
    }

    pub fn connection(&self, target: &RemoteTarget) -> Result<Arc<SshConnection>> {
        let mut connections = self.connections.lock();
        if let Some(connection) = connections.iter().find(|c| &c.target == target) {
            return Ok(connection.clone());
        }
        let saved = self
            .store
            .saved
            .lock()
            .iter()
            .find(|c| &c.target == target)
            .map(|c| c.mappings.clone())
            .unwrap_or_default();
        let connection = Arc::new(SshConnection::new(
            target.clone(),
            self.store.clone(),
            saved,
        )?);
        connections.push(connection.clone());
        self.store.registry_revision.fetch_add(1, Ordering::Release);
        Ok(connection)
    }
    pub fn by_id(&self, id: &str) -> Result<Arc<SshConnection>> {
        self.connections
            .lock()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .context("SSH connection unavailable")
    }
    pub fn snapshots(&self) -> (u64, Vec<eggie_protocol::PortMappingsSnapshot>) {
        let connections = self.connections.lock();
        let revision = self.store.registry_revision.load(Ordering::Acquire);
        let connections = connections.clone();
        (revision, connections.iter().map(|c| c.snapshot()).collect())
    }
    pub fn start_maintenance(this: &Arc<Self>) {
        let weak = Arc::downgrade(this);
        thread::Builder::new()
            .name("eggie-ssh".into())
            .spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(2));
                    let Some(this) = weak.upgrade() else { break };
                    let connections = this.connections.lock().clone();
                    drop(this);
                    for connection in connections {
                        connection.maintain();
                    }
                }
            })
            .expect("failed to start SSH maintenance");
    }
}

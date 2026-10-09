use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use eggie_config::{ActionContext, ConfigSnapshot, LuaConfig, QueuedAction};
use parking_lot::{Condvar, Mutex};

pub(super) struct ConfigService {
    path: PathBuf,
    current: Mutex<CurrentConfig>,
    changed: Condvar,
}

struct CurrentConfig {
    revision: u64,
    status_revision: u64,
    error: Option<String>,
    runtime: LuaConfig,
    previous: VecDeque<(u64, LuaConfig)>,
}

impl ConfigService {
    pub(super) fn new(path: PathBuf) -> Self {
        let mut diagnostic = None;
        let runtime = LuaConfig::load(&path).unwrap_or_else(|error| {
            diagnostic = Some(format!("{error:#}"));
            eprintln!("Eggie Lua configuration error: {error:#}");
            LuaConfig::from_source(&path, "", path.is_file())
                .expect("empty Lua configuration is valid")
        });
        Self {
            path,
            current: Mutex::new(CurrentConfig {
                revision: 1,
                status_revision: 1,
                error: diagnostic,
                runtime,
                previous: VecDeque::new(),
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn snapshot(&self) -> ConfigSnapshot {
        let current = self.current.lock();
        snapshot(&current, &self.path)
    }

    pub(super) fn reload(&self) -> Result<ConfigSnapshot> {
        // Parsing can run arbitrary user Lua. It never holds the current config
        // lock, and an error leaves the existing VM and snapshot untouched.
        let candidate = match LuaConfig::load(&self.path) {
            Ok(candidate) => candidate,
            Err(error) => {
                let mut current = self.current.lock();
                current.error = Some(format!("{error:#}"));
                current.status_revision = current.status_revision.wrapping_add(1).max(1);
                self.changed.notify_all();
                return Err(error);
            }
        };
        let mut current = self.current.lock();
        let old_revision = current.revision;
        let old_runtime = std::mem::replace(&mut current.runtime, candidate);
        current.previous.push_back((old_revision, old_runtime));
        if current.previous.len() > 8 {
            current.previous.pop_front();
        }
        current.revision = current.revision.wrapping_add(1).max(1);
        current.status_revision = current.status_revision.wrapping_add(1).max(1);
        current.error = None;
        let snapshot = snapshot(&current, &self.path);
        self.changed.notify_all();
        Ok(snapshot)
    }

    pub(super) fn wait_after(
        &self,
        after_revision: u64,
        timeout: Duration,
    ) -> Option<ConfigSnapshot> {
        let deadline = Instant::now() + timeout;
        let mut current = self.current.lock();
        while current.status_revision <= after_revision {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            self.changed.wait_for(&mut current, deadline - now);
        }
        Some(snapshot(&current, &self.path))
    }

    pub(super) fn invoke(
        &self,
        revision: u64,
        name: &str,
        context: &ActionContext,
        args: &serde_json::Value,
    ) -> Result<Vec<QueuedAction>> {
        let mut current = self.current.lock();
        if revision == 0 || revision == current.revision {
            return current.runtime.invoke(name, context, args);
        }
        if let Some((_, runtime)) = current
            .previous
            .iter_mut()
            .find(|(older, _)| *older == revision)
        {
            return runtime.invoke(name, context, args);
        }
        bail!("configuration revision {revision} is no longer available")
    }
}

fn snapshot(current: &CurrentConfig, path: &PathBuf) -> ConfigSnapshot {
    let mut snapshot = current.runtime.snapshot(current.revision);
    snapshot.status_revision = current.status_revision;
    snapshot.exists = path.is_file();
    snapshot.error = current.error.clone();
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_reload_preserves_actions_and_reports_error() {
        let path = std::env::temp_dir().join(format!(
            "eggie-config-test-{}.lua",
            eggie_domain::SessionId::new_v4()
        ));
        std::fs::write(
            &path,
            "eggie.bind('cmd+d', 'pane.split', { direction = 'right' })",
        )
        .unwrap();
        let service = ConfigService::new(path.clone());
        let original = service.snapshot();
        assert!(original.error.is_none());
        std::fs::write(&path, "eggie.bind('cmd+d', 'missing')").unwrap();
        assert!(service.reload().is_err());
        let failed = service
            .wait_after(original.status_revision, Duration::ZERO)
            .unwrap();
        assert_eq!(failed.revision, original.revision);
        assert!(failed.status_revision > original.status_revision);
        assert!(failed.error.as_deref().unwrap().contains("unknown action"));
        assert_eq!(failed.registered, original.registered);
        std::fs::write(
            &path,
            "eggie.bind('cmd+d', 'pane.split', { direction = 'down' })",
        )
        .unwrap();
        let recovered = service.reload().unwrap();
        assert!(recovered.revision > original.revision);
        assert!(recovered.error.is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_initial_file_keeps_its_path_and_diagnostic() {
        let path = std::env::temp_dir().join(format!(
            "eggie-config-test-{}.lua",
            eggie_domain::SessionId::new_v4()
        ));
        std::fs::write(&path, "eggie.bind('cmd+d', 'missing')").unwrap();
        let snapshot = ConfigService::new(path.clone()).snapshot();
        assert!(snapshot.exists);
        assert_eq!(snapshot.path, path);
        assert!(
            snapshot
                .error
                .as_deref()
                .unwrap()
                .contains("unknown action")
        );
        assert!(snapshot.registered.bindings.is_empty());
        std::fs::remove_file(path).unwrap();
    }
}

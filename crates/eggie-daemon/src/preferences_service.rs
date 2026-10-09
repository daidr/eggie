use std::{fs, path::PathBuf};

use anyhow::{Context, Result, bail};
use eggie_config::PreferencesSnapshot;
use parking_lot::Mutex;

pub(super) struct PreferencesService {
    path: PathBuf,
    current: Mutex<PreferencesSnapshot>,
}

impl PreferencesService {
    pub(super) fn new() -> Self {
        Self::new_at(eggie_config::state_dir())
    }

    pub(super) fn new_at(directory: PathBuf) -> Self {
        let path = directory.join("preferences.json");
        let value = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("invalid Eggie preferences {}: {error}", path.display());
                serde_json::Value::Object(Default::default())
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // One-time migration. The old settings file is never read once
                // preferences.json exists, and it is kept as a backup.
                let legacy = directory.join("settings.json");
                match fs::read(&legacy) {
                    Ok(bytes) => {
                        let value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                            eprintln!(
                                "invalid legacy Eggie settings {}: {error}",
                                legacy.display()
                            );
                            serde_json::Value::Object(Default::default())
                        });
                        if let Err(error) = save(&path, &value) {
                            eprintln!("failed to migrate Eggie preferences: {error:#}");
                        }
                        value
                    }
                    Err(_) => serde_json::Value::Object(Default::default()),
                }
            }
            Err(error) => {
                eprintln!(
                    "failed to read Eggie preferences {}: {error}",
                    path.display()
                );
                serde_json::Value::Object(Default::default())
            }
        };
        Self {
            path,
            current: Mutex::new(PreferencesSnapshot { revision: 1, value }),
        }
    }

    pub(super) fn get(&self) -> PreferencesSnapshot {
        self.current.lock().clone()
    }

    pub(super) fn set(
        &self,
        base_revision: u64,
        value: serde_json::Value,
    ) -> Result<PreferencesSnapshot> {
        let mut current = self.current.lock();
        if current.revision != base_revision {
            bail!("preferences changed since revision {base_revision}; refresh before editing");
        }
        if !value.is_object() {
            bail!("preferences must be a JSON object");
        }
        save(&self.path, &value)?;
        current.revision = current.revision.wrapping_add(1).max(1);
        current.value = value;
        Ok(current.clone())
    }
}

fn save(path: &PathBuf, value: &serde_json::Value) -> Result<()> {
    let parent = path.parent().context("preferences path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

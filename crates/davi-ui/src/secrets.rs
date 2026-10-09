//! Values of `vars:secret` environment variables.
//!
//! Environment files only list secret *names*, so a collection can be
//! committed without leaking tokens. The values live here instead: a JSON file
//! in the user's config directory (readable only by the user on Unix), keyed
//! by collection directory and environment name. It is not encrypted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use davi_core::env::Environment;
use serde::{Deserialize, Serialize};

use crate::settings::config_dir;

type Values = BTreeMap<String, String>;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SecretStore {
    #[serde(default)]
    collections: BTreeMap<PathBuf, BTreeMap<String, Values>>,
}

impl SecretStore {
    pub fn load() -> Self {
        store_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Fill in the values of `environment`'s secret variables.
    pub fn fill(&self, root: &Path, environment: &mut Environment) {
        let Some(values) = self
            .collections
            .get(root)
            .and_then(|envs| envs.get(&environment.name))
        else {
            return;
        };
        for var in environment.variables.iter_mut().filter(|v| v.secret) {
            if let Some(value) = values.get(&var.name) {
                var.value.clone_from(value);
            }
        }
    }

    /// Remember the secret values of `environment`, replacing any stored
    /// under `previous_name` (when it was renamed). Does not persist.
    pub fn store(&mut self, root: &Path, previous_name: Option<&str>, environment: &Environment) {
        let envs = self.collections.entry(root.to_path_buf()).or_default();
        if let Some(previous) = previous_name {
            envs.remove(previous);
        }
        let values: Values = environment
            .variables
            .iter()
            .filter(|v| v.secret && !v.value.is_empty())
            .map(|v| (v.name.clone(), v.value.clone()))
            .collect();
        if values.is_empty() {
            envs.remove(&environment.name);
        } else {
            envs.insert(environment.name.clone(), values);
        }
    }

    pub fn remove(&mut self, root: &Path, name: &str) {
        if let Some(envs) = self.collections.get_mut(root) {
            envs.remove(name);
        }
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.collections.retain(|_, envs| !envs.is_empty());
        let Some(path) = store_path() else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        write_private(&path, json.as_bytes())
    }
}

fn store_path() -> Option<PathBuf> {
    Some(config_dir()?.join("secrets.json"))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // `mode` only applies on creation; tighten a pre-existing file too.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

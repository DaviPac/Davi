//! Small persisted app settings (recent collections), stored as JSON in the
//! platform config directory.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 10;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub recent_collections: Vec<PathBuf>,
}

impl Settings {
    pub fn load() -> Self {
        settings_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("failed to save settings to {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("failed to serialize settings: {e}"),
        }
    }

    /// Move `path` to the front of the recent list and persist.
    pub fn remember_collection(&mut self, path: PathBuf) {
        self.recent_collections.retain(|p| p != &path);
        self.recent_collections.insert(0, path);
        self.recent_collections.truncate(MAX_RECENT);
        self.save();
    }

    pub fn last_collection(&self) -> Option<PathBuf> {
        self.recent_collections.iter().find(|p| p.is_dir()).cloned()
    }
}

fn settings_path() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = if cfg!(target_os = "windows") {
        env("APPDATA")?.join("Davi")
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support/Davi")
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))?
            .join("davi")
    };
    Some(dir.join("settings.json"))
}

//! User preferences, stored at `%LOCALAPPDATA%\Wield\settings.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    /// Look for a new version when Wield starts (at most every 12 hours).
    pub auto_check: bool,
    /// Download updates in the background and install them when Wield closes.
    pub auto_install: bool,
    pub last_check_unix: u64,
    /// A version the user chose to skip, e.g. "0.6.0".
    pub skipped_version: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { dark: true, auto_check: true, auto_install: false, last_check_unix: 0, skipped_version: None }
    }
}

pub fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Wield"))
}

fn path() -> Option<PathBuf> {
    Some(data_dir()?.join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, serde_json::to_string_pretty(self).unwrap_or_default());
        }
    }

    /// Loads, changes and saves in one step so concurrent fields aren't lost.
    pub fn update(f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = Self::load();
        f(&mut s);
        s.save();
        s
    }
}

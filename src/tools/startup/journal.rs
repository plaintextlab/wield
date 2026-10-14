//! Persistent record of every change Wield makes, so each one can be reverted exactly.
//! Stored at `%LOCALAPPDATA%\Wield\startup-journal.json`.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::model::SourceKind;

/// The exact state before a change.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PrevState {
    Approved { machine: bool, subkey: String, value: String, bytes: Option<Vec<u8>> },
    Service { name: String, start_type: u32, delayed: bool },
    Task { path: String, enabled: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// Unix seconds.
    pub when: u64,
    pub item_id: String,
    pub item_name: String,
    pub source: SourceKind,
    pub action: String,
    pub previous: PrevState,
    #[serde(default)]
    pub reverted: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Journal {
    pub entries: Vec<Entry>,
}

fn path() -> Option<PathBuf> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    Some(PathBuf::from(base).join("Wield").join("startup-journal.json"))
}

impl Journal {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = path().ok_or("LOCALAPPDATA is not set")?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(p, json).map_err(|e| e.to_string())
    }

    pub fn location() -> String {
        path().map(|p| p.display().to_string()).unwrap_or_default()
    }

    pub fn record(entry: Entry) -> Result<(), String> {
        let mut j = Self::load();
        j.entries.push(entry);
        j.save()
    }

    /// Lowercase names of services whose start type Wield has changed.
    pub fn services_changed_by_wield(&self) -> HashSet<String> {
        self.entries
            .iter()
            .filter_map(|e| match &e.previous {
                PrevState::Service { name, .. } => Some(name.to_ascii_lowercase()),
                _ => None,
            })
            .collect()
    }

    /// The most recent automatic start configuration Wield saw for a service: (start type, delayed).
    pub fn original_service_start(&self, name: &str) -> Option<(u32, bool)> {
        self.entries.iter().rev().find_map(|e| match &e.previous {
            PrevState::Service { name: n, start_type, delayed } if n.eq_ignore_ascii_case(name) && *start_type == 2 => {
                Some((*start_type, *delayed))
            }
            _ => None,
        })
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "2026-10-14 21:05" in UTC — avoids pulling in a timezone crate.
pub fn format_time(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3600, (secs % 3600) / 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_time() {
        assert_eq!(super::format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(super::format_time(1_760_475_900), "2025-10-14 21:05 UTC");
    }
}

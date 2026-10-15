//! Reads and applies privacy tweaks, keeping a backup of every original value.
//!
//! The first time Wield touches a value it stores the original in
//! `%LOCALAPPDATA%\Wield\privacy-backup.json`, so "Restore original settings" puts the machine
//! back exactly as it was, even if that differed from the Windows default.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ};
use winreg::{RegKey, RegValue};

use super::catalog::{Change, Data, Hive, Tweak};
use crate::tools::startup::{services, tasks};
use crate::win::sysver::OsInfo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    On,
    Off,
    /// Some of the tweak's changes are applied.
    Partial,
    /// Not available on this machine; the reason is shown to the user.
    Unavailable(String),
}

fn hive_key(h: Hive) -> RegKey {
    RegKey::predef(match h {
        Hive::User => HKEY_CURRENT_USER,
        Hive::Machine => HKEY_LOCAL_MACHINE,
    })
}

fn hive_label(h: Hive) -> &'static str {
    match h {
        Hive::User => "HKCU",
        Hive::Machine => "HKLM",
    }
}

/// Raw registry value: (type, bytes). `None` when the value is absent.
type Raw = Option<(u32, Vec<u8>)>;

fn read_raw(h: Hive, path: &str, name: &str) -> Raw {
    let key = hive_key(h).open_subkey_with_flags(path, KEY_READ).ok()?;
    key.get_raw_value(name).ok().map(|v| (v.vtype as u32, v.bytes.into_owned()))
}

fn resolve(d: Data, os: &OsInfo) -> Data {
    match d {
        Data::TelemetryMin => Data::Dword(if os.is_enterprise_class() { 0 } else { 1 }),
        other => other,
    }
}

fn raw_matches(raw: &Raw, want: Data, os: &OsInfo) -> bool {
    let Some((ty, bytes)) = raw else { return false };
    match (resolve(want, os), *ty) {
        (Data::Dword(w), t) if t == REG_DWORD as u32 && bytes.len() >= 4 => {
            let v = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            // For the telemetry level anything at or below the target counts as private.
            if matches!(want, Data::TelemetryMin) { v <= w } else { v == w }
        }
        (Data::Str(w), t) if t == REG_SZ as u32 || t == 2 => {
            let words: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&words).trim_end_matches('\0').eq_ignore_ascii_case(w)
        }
        _ => false,
    }
}

fn write_data(h: Hive, path: &str, name: &str, data: Option<Data>, os: &OsInfo) -> Result<(), String> {
    match data.map(|d| resolve(d, os)) {
        None => {
            let Ok(key) = hive_key(h).open_subkey_with_flags(path, KEY_READ | KEY_WRITE) else { return Ok(()) };
            match key.delete_value(name) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
        Some(d) => {
            let (key, _) = hive_key(h).create_subkey_with_flags(path, KEY_READ | KEY_WRITE).map_err(|e| e.to_string())?;
            match d {
                Data::Dword(v) => key.set_value(name, &v),
                Data::Str(s) => key.set_value(name, &s.to_string()),
                Data::TelemetryMin => unreachable!("resolved above"),
            }
            .map_err(|e| e.to_string())
        }
    }
}

fn write_raw(h: Hive, path: &str, name: &str, raw: &Raw) -> Result<(), String> {
    match raw {
        None => {
            let Ok(key) = hive_key(h).open_subkey_with_flags(path, KEY_READ | KEY_WRITE) else { return Ok(()) };
            match key.delete_value(name) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
        Some((ty, bytes)) => {
            let (key, _) = hive_key(h).create_subkey_with_flags(path, KEY_READ | KEY_WRITE).map_err(|e| e.to_string())?;
            let vtype = match *ty {
                1 => REG_SZ,
                4 => REG_DWORD,
                2 => winreg::enums::REG_EXPAND_SZ,
                3 => winreg::enums::REG_BINARY,
                11 => winreg::enums::REG_QWORD,
                7 => winreg::enums::REG_MULTI_SZ,
                _ => winreg::enums::REG_BINARY,
            };
            key.set_raw_value(name, &RegValue { bytes: bytes.clone().into(), vtype }).map_err(|e| e.to_string())
        }
    }
}

/// Per-change state: Some(true) private, Some(false) not, None not present on this machine.
/// Needs COM on the calling thread for scheduled tasks.
fn change_states(c: &Change, os: &OsInfo) -> Vec<Option<bool>> {
    match *c {
        Change::Reg { hive, path, name, private, .. } => vec![Some(raw_matches(&read_raw(hive, path, name), private, os))],
        Change::Service { name, private_start, .. } => vec![services::current_start(name).ok().map(|(s, _)| s == private_start)],
        Change::Tasks(paths) => paths.iter().map(|p| tasks::is_enabled(p).ok().map(|enabled| !enabled)).collect(),
    }
}

/// Needs COM on the calling thread.
pub fn state(t: &Tweak, os: &OsInfo) -> State {
    if t.min_build > 0 && os.build_number() < t.min_build {
        return State::Unavailable(format!("Needs Windows build {} or newer.", t.min_build));
    }
    let states: Vec<bool> = t.changes.iter().flat_map(|c| change_states(c, os)).flatten().collect();
    if states.is_empty() {
        return State::Unavailable("Not present on this PC.".into());
    }
    match (states.iter().all(|s| *s), states.iter().any(|s| *s)) {
        (true, _) => State::On,
        (false, true) => State::Partial,
        _ => State::Off,
    }
}

// ------------------------------------------------------------------ backup

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Original {
    Reg { machine: bool, path: String, name: String, value: Option<(u32, Vec<u8>)> },
    Service { name: String, start: u32, delayed: bool },
    Task { path: String, enabled: bool },
}

#[derive(Default, Serialize, Deserialize)]
pub struct Backup {
    /// Keyed by a stable identifier, e.g. `reg|HKCU|path|name`. Only the first value seen is kept.
    pub originals: BTreeMap<String, Original>,
}

fn backup_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Wield").join("privacy-backup.json"))
}

impl Backup {
    pub fn load() -> Self {
        backup_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = backup_path().ok_or("LOCALAPPDATA is not set")?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(p, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }

    pub fn location() -> String {
        backup_path().map(|p| p.display().to_string()).unwrap_or_default()
    }

    fn remember(&mut self, c: &Change) {
        match *c {
            Change::Reg { hive, path, name, .. } => {
                let key = format!("reg|{}|{path}|{name}", hive_label(hive)).to_lowercase();
                self.originals.entry(key).or_insert_with(|| Original::Reg {
                    machine: hive == Hive::Machine,
                    path: path.into(),
                    name: name.into(),
                    value: read_raw(hive, path, name),
                });
            }
            Change::Service { name, .. } => {
                if let Ok((start, delayed)) = services::current_start(name) {
                    self.originals.entry(format!("svc|{name}").to_lowercase()).or_insert(Original::Service { name: name.into(), start, delayed });
                }
            }
            Change::Tasks(paths) => {
                for p in paths {
                    if let Ok(enabled) = tasks::is_enabled(p) {
                        self.originals.entry(format!("task|{p}").to_lowercase()).or_insert(Original::Task { path: (*p).into(), enabled });
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ apply

fn apply_change(c: &Change, on: bool, os: &OsInfo) -> Result<(), String> {
    match *c {
        Change::Reg { hive, path, name, private, default } => {
            write_data(hive, path, name, if on { Some(private) } else { default }, os).map_err(|e| format!("{}\\{path}\\{name}: {e}", hive_label(hive)))
        }
        Change::Service { name, private_start, default_start, default_delayed } => match services::current_start(name) {
            // A service that doesn't exist on this edition is simply skipped.
            Err(_) => Ok(()),
            Ok(_) if on => services::set_start(name, private_start, false).map_err(|e| format!("{name}: {e}")),
            Ok(_) => services::set_start(name, default_start, default_delayed).map_err(|e| format!("{name}: {e}")),
        },
        Change::Tasks(paths) => {
            let mut first_err = None;
            for p in paths {
                if tasks::is_enabled(p).is_ok() {
                    if let Err(e) = tasks::set_enabled(p, !on) {
                        first_err.get_or_insert(format!("{p}: {e}"));
                    }
                }
            }
            first_err.map_or(Ok(()), Err)
        }
    }
}

/// Turns a tweak on (privacy value) or off (Windows default). Needs COM on the calling thread.
/// All changes are attempted; the first error is returned.
pub fn set(t: &Tweak, on: bool, os: &OsInfo) -> Result<(), String> {
    let mut backup = Backup::load();
    for c in t.changes {
        backup.remember(c);
    }
    backup.save().map_err(|e| format!("Could not save the backup, nothing was changed: {e}"))?;

    let mut first_err = None;
    for c in t.changes {
        if let Err(e) = apply_change(c, on, os) {
            first_err.get_or_insert(e);
        }
    }
    first_err.map_or(Ok(()), Err)
}

/// Restores every value Wield ever changed to what it was before. Needs COM on the calling thread.
/// Returns (restored, errors).
pub fn restore_all() -> (usize, Vec<String>) {
    let mut backup = Backup::load();
    let mut errors = Vec::new();
    let mut restored = 0;
    let mut remaining = BTreeMap::new();
    for (key, orig) in std::mem::take(&mut backup.originals) {
        let res = match &orig {
            Original::Reg { machine, path, name, value } => {
                write_raw(if *machine { Hive::Machine } else { Hive::User }, path, name, value)
            }
            Original::Service { name, start, delayed } => services::set_start(name, *start, *delayed),
            Original::Task { path, enabled } => tasks::set_enabled(path, *enabled),
        };
        match res {
            Ok(()) => restored += 1,
            Err(e) => {
                errors.push(format!("{key}: {e}"));
                remaining.insert(key, orig);
            }
        }
    }
    // Keep whatever could not be restored (e.g. needs admin) so it can be retried.
    backup.originals = remaining;
    let _ = backup.save();
    (restored, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::privacy::catalog::TWEAKS;

    #[test]
    fn reads_every_tweak_state() {
        let os = crate::win::sysver::os_info();
        crate::win::with_com(move || {
            for t in TWEAKS {
                let _ = state(t, &os);
            }
        });
    }

    #[test]
    fn hkcu_value_roundtrip() {
        let os = OsInfo::default();
        let path = r"Software\WieldSelfTest";
        write_data(Hive::User, path, "V", Some(Data::Dword(0)), &os).unwrap();
        assert!(raw_matches(&read_raw(Hive::User, path, "V"), Data::Dword(0), &os));
        write_data(Hive::User, path, "S", Some(Data::Str("Deny")), &os).unwrap();
        assert!(raw_matches(&read_raw(Hive::User, path, "S"), Data::Str("Deny"), &os));
        write_data(Hive::User, path, "V", None, &os).unwrap();
        assert!(read_raw(Hive::User, path, "V").is_none());
        let saved = read_raw(Hive::User, path, "S");
        write_raw(Hive::User, path, "S", &None).unwrap();
        write_raw(Hive::User, path, "S", &saved).unwrap();
        assert!(raw_matches(&read_raw(Hive::User, path, "S"), Data::Str("Deny"), &os));
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(path);
    }

    #[test]
    fn telemetry_level_by_edition() {
        let pro = OsInfo { edition_id: "Professional".into(), ..Default::default() };
        let ltsc = OsInfo { edition_id: "IoTEnterpriseS".into(), ..Default::default() };
        assert_eq!(resolve(Data::TelemetryMin, &pro), Data::Dword(1));
        assert_eq!(resolve(Data::TelemetryMin, &ltsc), Data::Dword(0));
        let one: Raw = Some((REG_DWORD as u32, 1u32.to_le_bytes().to_vec()));
        assert!(raw_matches(&one, Data::TelemetryMin, &pro));
        assert!(!raw_matches(&one, Data::TelemetryMin, &ltsc));
    }
}

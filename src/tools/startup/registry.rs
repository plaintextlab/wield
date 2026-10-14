//! Run keys, Startup folders and Winlogon entries.
//!
//! Enabling/disabling uses the same `Explorer\StartupApproved` values that Task Manager and
//! Settings > Apps > Startup write, so changes show up there too and are fully reversible.

use std::path::PathBuf;

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_BINARY};
use winreg::{RegKey, RegValue};

use super::model::{Locator, SourceKind, StartupItem};
use crate::win::fileinfo::{expand_env, resolve_shortcut};

const APPROVED_BASE: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

fn hive(machine: bool) -> RegKey {
    RegKey::predef(if machine { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
}

fn hive_name(machine: bool) -> &'static str {
    if machine { "HKLM" } else { "HKCU" }
}

/// Raw `StartupApproved` data for a value, if Explorer has recorded one.
pub fn read_approved(machine: bool, subkey: &str, value: &str) -> Option<Vec<u8>> {
    let key = hive(machine).open_subkey_with_flags(format!(r"{APPROVED_BASE}\{subkey}"), KEY_READ).ok()?;
    key.get_raw_value(value).ok().map(|v| v.bytes.into_owned())
}

/// The first byte is even (0x02/0x06) when enabled and odd (0x03/0x07) when disabled.
fn approved_enabled(data: Option<&[u8]>) -> bool {
    data.and_then(|d| d.first()).is_none_or(|b| b & 1 == 0)
}

/// Writes the enabled/disabled marker. Disabled entries carry the FILETIME they were disabled at.
pub fn write_approved(machine: bool, subkey: &str, value: &str, enabled: bool) -> std::io::Result<()> {
    let mut bytes = vec![0u8; 12];
    if enabled {
        bytes[0] = 0x02;
    } else {
        bytes[0] = 0x03;
        bytes[4..12].copy_from_slice(&filetime_now().to_le_bytes());
    }
    write_approved_raw(machine, subkey, value, Some(&bytes))
}

/// Restores an exact previous value (or removes it when there was none).
pub fn write_approved_raw(machine: bool, subkey: &str, value: &str, bytes: Option<&[u8]>) -> std::io::Result<()> {
    let (key, _) = hive(machine).create_subkey_with_flags(format!(r"{APPROVED_BASE}\{subkey}"), KEY_READ | KEY_WRITE)?;
    match bytes {
        Some(b) => key.set_raw_value(value, &RegValue { bytes: b.to_vec().into(), vtype: REG_BINARY }),
        None => match key.delete_value(value) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
    }
}

fn filetime_now() -> u64 {
    // FILETIME counts 100ns intervals since 1601-01-01.
    const EPOCH_DIFF_SECS: u64 = 11_644_473_600;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (now.as_secs() + EPOCH_DIFF_SECS) * 10_000_000 + u64::from(now.subsec_nanos() / 100)
}

struct RunLocation {
    machine: bool,
    path: &'static str,
    approved: Option<&'static str>,
    readonly: Option<&'static str>,
}

const RUN_LOCATIONS: &[RunLocation] = &[
    RunLocation { machine: false, path: r"Software\Microsoft\Windows\CurrentVersion\Run", approved: Some("Run"), readonly: None },
    RunLocation { machine: true, path: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run", approved: Some("Run"), readonly: None },
    RunLocation {
        machine: true,
        path: r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Run",
        approved: Some("Run32"),
        readonly: None,
    },
    RunLocation {
        machine: false,
        path: r"Software\Microsoft\Windows\CurrentVersion\RunOnce",
        approved: None,
        readonly: Some("RunOnce entries run a single time at the next sign-in and then remove themselves."),
    },
    RunLocation {
        machine: true,
        path: r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce",
        approved: None,
        readonly: Some("RunOnce entries run a single time at the next sign-in and then remove themselves."),
    },
    RunLocation {
        machine: false,
        path: r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run",
        approved: None,
        readonly: Some("Enforced by Group Policy. Change it with the Group Policy editor or your administrator."),
    },
    RunLocation {
        machine: true,
        path: r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run",
        approved: None,
        readonly: Some("Enforced by Group Policy. Change it with the Group Policy editor or your administrator."),
    },
];

pub fn collect_run_keys(out: &mut Vec<StartupItem>) {
    for loc in RUN_LOCATIONS {
        let Ok(key) = hive(loc.machine).open_subkey_with_flags(loc.path, KEY_READ) else { continue };
        let location = format!(r"{}\{}", hive_name(loc.machine), loc.path);
        for (name, _) in key.enum_values().flatten() {
            let Ok(command) = key.get_value::<String, _>(&name) else { continue };
            if command.trim().is_empty() {
                continue;
            }
            let display = if name.is_empty() { "(Default)".to_string() } else { name.clone() };
            let id = format!("run|{location}|{name}");
            let mut item = match loc.approved {
                Some(sub) => {
                    let mut it = StartupItem::new(
                        id,
                        display,
                        command,
                        SourceKind::RegistryRun,
                        location.clone(),
                        Locator::RunValue { machine: loc.machine, approved_subkey: sub.into(), value_name: name.clone() },
                    );
                    it.enabled = approved_enabled(read_approved(loc.machine, sub, &name).as_deref());
                    it.status_text = if it.enabled { "Enabled" } else { "Disabled" }.into();
                    it.needs_admin = loc.machine;
                    it
                }
                None => {
                    let mut it = StartupItem::new(id, display, command, SourceKind::RegistryRun, location.clone(), Locator::ReadOnly)
                        .read_only(loc.readonly.unwrap_or("Read-only"));
                    it.status_text = if loc.path.contains("RunOnce") { "Runs once" } else { "Policy" }.into();
                    it
                }
            };
            item.facts.push(("Scope".into(), if loc.machine { "All users" } else { "Current user" }.into()));
            out.push(item);
        }
    }
}

fn startup_folder(machine: bool) -> Option<PathBuf> {
    let base = std::env::var(if machine { "ProgramData" } else { "APPDATA" }).ok()?;
    Some(PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
}

/// Needs COM initialized on the calling thread (for resolving shortcuts).
pub fn collect_startup_folders(out: &mut Vec<StartupItem>) {
    for machine in [false, true] {
        let Some(dir) = startup_folder(machine) else { continue };
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();
            if !path.is_file() || file_name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let is_link = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
            let command = if is_link {
                match resolve_shortcut(&path) {
                    Some((target, args)) if args.is_empty() => format!("\"{target}\""),
                    Some((target, args)) => format!("\"{target}\" {args}"),
                    None => path.display().to_string(),
                }
            } else {
                format!("\"{}\"", path.display())
            };
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(file_name.clone());
            let mut item = StartupItem::new(
                format!("folder|{}|{file_name}", hive_name(machine)),
                name,
                command,
                SourceKind::StartupFolder,
                dir.display().to_string(),
                Locator::StartupFile { machine, file_name: file_name.clone() },
            );
            item.enabled = approved_enabled(read_approved(machine, "StartupFolder", &file_name).as_deref());
            item.status_text = if item.enabled { "Enabled" } else { "Disabled" }.into();
            item.needs_admin = machine;
            item.facts.push(("Shortcut file".into(), path.display().to_string()));
            item.facts.push(("Scope".into(), if machine { "All users" } else { "Current user" }.into()));
            out.push(item);
        }
    }
}

/// Shell, Userinit and AppInit_DLLs: what Windows launches to build the desktop itself.
pub fn collect_winlogon(out: &mut Vec<StartupItem>) {
    let winlogon = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
    let defaults: [(&str, &[&str]); 2] = [
        ("Shell", &["explorer.exe"]),
        ("Userinit", &[r"c:\windows\system32\userinit.exe,", r"c:\windows\system32\userinit.exe"]),
    ];
    for machine in [true, false] {
        let Ok(key) = hive(machine).open_subkey_with_flags(winlogon, KEY_READ) else { continue };
        for (value, expected) in defaults {
            let Ok(command) = key.get_value::<String, _>(value) else { continue };
            if !machine && command.trim().is_empty() {
                continue;
            }
            let normalized = expand_env(command.trim()).to_ascii_lowercase();
            let is_default = expected.contains(&normalized.as_str());
            let mut item = StartupItem::new(
                format!("winlogon|{}|{value}", hive_name(machine)),
                format!("Winlogon {value}"),
                command,
                SourceKind::Winlogon,
                format!(r"{}\{winlogon}", hive_name(machine)),
                Locator::ReadOnly,
            )
            .read_only("Core sign-in component. Wield never modifies it; edit it manually only if you know exactly why.");
            item.status_text = if is_default { "Windows default" } else { "MODIFIED" }.into();
            if !is_default {
                item.facts.push((
                    "Warning".into(),
                    "This value differs from the Windows default. Custom shells do this on purpose, but so does malware. \
                     Verify the program below is something you installed."
                        .into(),
                ));
            }
            out.push(item);
        }
    }

    if let Ok(key) = hive(true).open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Windows", KEY_READ) {
        let dlls: String = key.get_value("AppInit_DLLs").unwrap_or_default();
        let load: u32 = key.get_value("LoadAppInit_DLLs").unwrap_or(0);
        if !dlls.trim().is_empty() {
            let mut item = StartupItem::new(
                "winlogon|HKLM|AppInit_DLLs".into(),
                "AppInit_DLLs".into(),
                dlls,
                SourceKind::Winlogon,
                r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Windows".into(),
                Locator::ReadOnly,
            )
            .read_only("DLLs injected into every GUI process. Wield does not modify this value.");
            item.enabled = load == 1;
            item.status_text = if load == 1 { "Active" } else { "Inactive (LoadAppInit_DLLs = 0)" }.into();
            item.facts.push(("Warning".into(), "AppInit_DLLs is a legacy mechanism often abused by malware.".into()));
            out.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes and removes a throwaway HKCU StartupApproved value; touches no real entry.
    #[test]
    fn approved_roundtrip() {
        let name = "WieldSelfTest-DoesNotExist";
        write_approved(false, "Run", name, false).unwrap();
        let data = read_approved(false, "Run", name).unwrap();
        assert_eq!(data.len(), 12);
        assert!(!approved_enabled(Some(&data)));
        write_approved(false, "Run", name, true).unwrap();
        assert!(approved_enabled(read_approved(false, "Run", name).as_deref()));
        write_approved_raw(false, "Run", name, None).unwrap();
        assert!(read_approved(false, "Run", name).is_none());
    }
}

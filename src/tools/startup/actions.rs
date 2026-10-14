//! Enabling / disabling entries, reverting journal entries and creating restore points.

use std::os::windows::process::CommandExt;

use super::journal::{self, Entry, Journal, PrevState};
use super::model::{Locator, StartupItem};
use super::{registry, services, tasks};
use crate::win::ComGuard;
use crate::win::elevation::is_elevated;

const SERVICE_AUTO: u32 = 2;
const SERVICE_MANUAL: u32 = 3;
const SERVICE_DISABLED: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceMode {
    /// Start on demand: the safe way to stop a service from starting at boot.
    Manual,
    /// Never start, even when something asks for it.
    Disabled,
}

/// Result of a successful change: the new enabled flag and status text.
pub struct Applied {
    pub enabled: bool,
    pub status_text: String,
    pub service_start: Option<(u32, bool)>,
}

fn friendly(err: String) -> String {
    let lower = err.to_lowercase();
    if lower.contains("access is denied") || lower.contains("access denied") || lower.contains("0x80070005") {
        if is_elevated() {
            "Access denied. Windows protects this entry; even administrators can't change it this way.".into()
        } else {
            "Access denied. Restart Wield as administrator to change this entry.".into()
        }
    } else {
        err
    }
}

/// Runs `f` on a short-lived worker thread with COM initialized, and waits for it.
fn with_com<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(move || {
        let _com = ComGuard::new();
        f()
    })
    .join()
    .expect("worker thread panicked")
}

pub fn set_enabled(item: &StartupItem, enable: bool, mode: ServiceMode) -> Result<Applied, String> {
    let item = item.clone();
    with_com(move || apply(&item, enable, mode)).map_err(friendly)
}

fn apply(item: &StartupItem, enable: bool, mode: ServiceMode) -> Result<Applied, String> {
    let verb = if enable { "Enabled" } else { "Disabled" };
    let (previous, applied) = match &item.locator {
        Locator::RunValue { machine, approved_subkey, value_name } => {
            let prev = registry::read_approved(*machine, approved_subkey, value_name);
            registry::write_approved(*machine, approved_subkey, value_name, enable).map_err(|e| e.to_string())?;
            (
                PrevState::Approved { machine: *machine, subkey: approved_subkey.clone(), value: value_name.clone(), bytes: prev },
                Applied { enabled: enable, status_text: verb.into(), service_start: None },
            )
        }
        Locator::StartupFile { machine, file_name } => {
            let prev = registry::read_approved(*machine, "StartupFolder", file_name);
            registry::write_approved(*machine, "StartupFolder", file_name, enable).map_err(|e| e.to_string())?;
            (
                PrevState::Approved { machine: *machine, subkey: "StartupFolder".into(), value: file_name.clone(), bytes: prev },
                Applied { enabled: enable, status_text: verb.into(), service_start: None },
            )
        }
        Locator::Service { name } => {
            let (start, delayed) = services::current_start(name)?;
            let (new_start, new_delayed) = if enable {
                Journal::load().original_service_start(name).unwrap_or((SERVICE_AUTO, false))
            } else if mode == ServiceMode::Disabled {
                (SERVICE_DISABLED, false)
            } else {
                (SERVICE_MANUAL, false)
            };
            services::set_start(name, new_start, new_delayed)?;
            (
                PrevState::Service { name: name.clone(), start_type: start, delayed },
                Applied {
                    enabled: new_start == SERVICE_AUTO,
                    status_text: services::start_type_label(new_start, new_delayed).into(),
                    service_start: Some((new_start, new_delayed)),
                },
            )
        }
        Locator::Task { path } => {
            let prev = tasks::is_enabled(path)?;
            tasks::set_enabled(path, enable)?;
            (PrevState::Task { path: path.clone(), enabled: prev }, Applied { enabled: enable, status_text: verb.into(), service_start: None })
        }
        Locator::ReadOnly => return Err(item.readonly_reason.clone().unwrap_or_else(|| "This entry is read-only.".into())),
    };

    let action = match &applied.service_start {
        Some((s, d)) => format!("Start type → {}", services::start_type_label(*s, *d)),
        None => verb.to_string(),
    };
    // The change itself succeeded; a journal failure is reported but not fatal.
    if let Err(e) = Journal::record(Entry {
        when: journal::now(),
        item_id: item.id.clone(),
        item_name: item.name.clone(),
        source: item.source,
        action,
        previous,
        reverted: false,
    }) {
        return Err(format!("The change was applied, but the undo history could not be saved: {e}"));
    }
    Ok(applied)
}

/// Restores the exact state recorded in journal entry `index`.
pub fn revert(index: usize) -> Result<String, String> {
    with_com(move || {
        let mut j = Journal::load();
        let entry = j.entries.get(index).cloned().ok_or("History entry not found")?;
        match &entry.previous {
            PrevState::Approved { machine, subkey, value, bytes } => {
                registry::write_approved_raw(*machine, subkey, value, bytes.as_deref()).map_err(|e| e.to_string())?
            }
            PrevState::Service { name, start_type, delayed } => services::set_start(name, *start_type, *delayed)?,
            PrevState::Task { path, enabled } => tasks::set_enabled(path, *enabled)?,
        }
        j.entries[index].reverted = true;
        j.save()?;
        Ok(entry.item_name)
    })
    .map_err(friendly)
}

/// Creates a System Restore point. Needs administrator rights and System Protection on C:.
/// Windows allows only one restore point per 24 h by default; extra requests are silently skipped.
pub fn create_restore_point() -> Result<(), String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Checkpoint-Computer -Description 'Wield - before startup changes' -RestorePointType MODIFY_SETTINGS -ErrorAction Stop",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.to_lowercase().contains("cannot be created") {
            return Err(stderr.trim().to_string());
        }
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("Unknown error").trim().to_string();
        Err(if msg.is_empty() { "Restore point creation failed. Is System Protection enabled?".into() } else { msg })
    }
}

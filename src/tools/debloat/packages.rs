//! Store / MSIX app packages via the WinRT `PackageManager`.
//! Every function here blocks; call them from a worker thread in the MTA (`with_mta`).

use std::collections::HashSet;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::{PackageManager, PackageTypes, RemovalOptions};
use windows::core::HSTRING;

use super::catalog::{self, Verdict};
use crate::win::shell;

#[derive(Clone, Debug)]
pub struct AppPackage {
    /// `Publisher.AppName`, e.g. `Microsoft.BingNews`.
    pub name: String,
    pub full_name: String,
    pub family_name: String,
    pub display_name: String,
    pub publisher: String,
    pub description: String,
    pub version: String,
    pub installed_path: Option<PathBuf>,
    pub size: Option<u64>,
    /// Signed as part of Windows: cannot be removed.
    pub system: bool,
    pub source: &'static str,
    /// Staged for new user accounts (only known when running as administrator).
    pub provisioned: bool,
    pub verdict: Verdict,
    pub reason: String,
}

impl AppPackage {
    pub fn removable(&self) -> bool {
        !self.system
    }
    pub fn haystack(&self) -> String {
        format!("{} {} {} {}", self.display_name, self.name, self.publisher, self.description).to_lowercase()
    }
}

fn s(r: windows::core::Result<HSTRING>) -> String {
    r.map(|h| h.to_string()).unwrap_or_default()
}

fn dir_size(p: &Path, budget: &mut usize) -> u64 {
    let Ok(entries) = std::fs::read_dir(p) else { return 0 };
    let mut total = 0;
    for e in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            total += dir_size(&e.path(), budget);
        } else if let Ok(m) = e.metadata() {
            total += m.len();
        }
    }
    total
}

const VENDOR_HINTS: &[&str] = &["intel", "realtek", "nvidia", "advanced micro devices", "synaptics", "dolby", "dts", "waves", "elan", "qualcomm"];

fn classify(name: &str, publisher: &str, system: bool, publisher_id: &str) -> (Verdict, String) {
    if let Some((v, r)) = catalog::lookup(name) {
        return (v, r.into());
    }
    if system {
        return (Verdict::Keep, "Part of Windows. Windows protects it from removal.".into());
    }
    let p = publisher.to_lowercase();
    if VENDOR_HINTS.iter().any(|v| p.contains(v)) {
        return (Verdict::Caution, "Hardware vendor app. It may hold settings for your display, audio or touchpad.".into());
    }
    if publisher_id.contains("CN=Microsoft") {
        return (
            Verdict::Caution,
            "Microsoft app that isn't in Wield's list. Windows features may use it; remove only if you know what it is.".into(),
        );
    }
    (Verdict::Optional, "Third-party app. Windows doesn't need it; remove it if you don't use it.".into())
}

fn to_app(p: &Package, provisioned: &HashSet<String>) -> Option<AppPackage> {
    let id = p.Id().ok()?;
    let name = s(id.Name());
    let full_name = s(id.FullName());
    let family_name = s(id.FamilyName());
    let publisher_id = s(id.Publisher());
    let v = id.Version().ok();
    let version = v.map(|v| format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision)).unwrap_or_default();
    let display = s(p.DisplayName());
    // Unresolved resource strings look like "ms-resource:AppName".
    let display_name = if display.is_empty() || display.starts_with("ms-resource:") { name.clone() } else { display };
    let mut publisher = s(p.PublisherDisplayName());
    if publisher.is_empty() || publisher.starts_with("ms-resource:") {
        publisher = publisher_id.split(',').next().unwrap_or("").trim_start_matches("CN=").to_string();
    }
    let mut description = s(p.Description());
    if description.starts_with("ms-resource:") {
        description.clear();
    }
    let kind = p.SignatureKind().unwrap_or(PackageSignatureKind::None);
    let system = kind == PackageSignatureKind::System;
    let source = match kind {
        PackageSignatureKind::Store => "Microsoft Store",
        PackageSignatureKind::System => "Windows",
        PackageSignatureKind::Enterprise => "Enterprise",
        PackageSignatureKind::Developer => "Sideloaded",
        _ => "Unsigned",
    };
    let installed_path = p.InstalledPath().ok().map(|h| PathBuf::from(h.to_string())).filter(|p| !p.as_os_str().is_empty());
    let size = installed_path.as_ref().map(|path| dir_size(path, &mut 20_000));
    let (verdict, reason) = classify(&name, &publisher, system, &publisher_id);
    Some(AppPackage {
        provisioned: provisioned.contains(&family_name.to_lowercase()),
        name,
        full_name,
        family_name,
        display_name,
        publisher,
        description,
        version,
        installed_path,
        size,
        system,
        source,
        verdict,
        reason,
    })
}

/// Lists the main (non-framework, non-resource) packages installed for the current user.
pub fn scan(elevated: bool) -> Result<Vec<AppPackage>, String> {
    let pm = PackageManager::new().map_err(|e| e.message())?;
    let provisioned: HashSet<String> = if elevated {
        pm.FindProvisionedPackages()
            .map(|v| v.into_iter().filter_map(|p| p.Id().ok().map(|id| s(id.FamilyName()).to_lowercase())).collect())
            .unwrap_or_default()
    } else {
        HashSet::new()
    };
    let packages = pm
        .FindPackagesByUserSecurityIdWithPackageTypes(&HSTRING::new(), PackageTypes::Main)
        .map_err(|e| format!("Cannot list apps: {}", e.message()))?;
    let mut apps: Vec<AppPackage> = packages.into_iter().filter_map(|p| to_app(&p, &provisioned)).collect();
    apps.sort_by(|a, b| a.verdict.cmp(&b.verdict).then(a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase())));
    Ok(apps)
}

fn deployment_error(e: windows::core::Error) -> String {
    let msg = e.message();
    let lower = msg.to_lowercase();
    if lower.contains("0x80073cfa") || lower.contains("part of windows") {
        "Windows protects this app; it can't be removed.".into()
    } else if lower.contains("access is denied") || lower.contains("0x80070005") {
        "Access denied. Restart Wield as administrator to remove apps for all users.".into()
    } else {
        msg
    }
}

/// Removes a package. `all_users` needs administrator rights; `deprovision` also stops Windows
/// from installing it for new user accounts (and after feature updates).
pub fn remove(full_name: &str, family_name: &str, all_users: bool, deprovision: bool) -> Result<(), String> {
    let pm = PackageManager::new().map_err(|e| e.message())?;
    if deprovision {
        // Not provisioned is fine: there is nothing to deprovision.
        if let Ok(op) = pm.DeprovisionPackageForAllUsersAsync(&HSTRING::from(family_name)) {
            let _ = op.join();
        }
    }
    let options = if all_users { RemovalOptions::RemoveForAllUsers } else { RemovalOptions::None };
    pm.RemovePackageWithOptionsAsync(&HSTRING::from(full_name), options).map_err(deployment_error)?.join().map_err(deployment_error)?;
    Ok(())
}

/// Reinstalls a removed app: re-registers it from its folder when Windows still has the files,
/// otherwise opens its Microsoft Store page. Returns a description of what happened.
pub fn reinstall(entry: &RemovedApp) -> Result<String, String> {
    if let Some(manifest) = entry.installed_path.as_ref().map(|p| p.join("AppxManifest.xml")).filter(|m| m.is_file()) {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!("Add-AppxPackage -DisableDevelopmentMode -Register -Path '{}' -ErrorAction Stop", manifest.display().to_string().replace('\'', "''")))
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(format!("Reinstalled \"{}\".", entry.display_name));
        }
    }
    shell::open(&format!("ms-windows-store://pdp/?PFN={}", entry.family_name));
    Ok(format!("Opened \"{}\" in the Microsoft Store. Click Get or Install there.", entry.display_name))
}

// ------------------------------------------------------------------ journal

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemovedApp {
    pub when: u64,
    pub display_name: String,
    pub name: String,
    pub family_name: String,
    pub full_name: String,
    pub installed_path: Option<PathBuf>,
    pub all_users: bool,
    pub deprovisioned: bool,
    #[serde(default)]
    pub reinstalled: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct RemovalLog {
    pub removed: Vec<RemovedApp>,
}

fn log_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var("LOCALAPPDATA").ok()?).join("Wield").join("debloat-journal.json"))
}

impl RemovalLog {
    pub fn load() -> Self {
        log_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) -> Result<(), String> {
        let p = log_path().ok_or("LOCALAPPDATA is not set")?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(p, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
    pub fn location() -> String {
        log_path().map(|p| p.display().to_string()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_installed_apps() {
        let apps = crate::win::with_mta(|| super::scan(false)).expect("scan");
        assert!(!apps.is_empty(), "every Windows install has Store apps");
        for a in &apps {
            assert!(!a.full_name.is_empty());
        }
    }
}

#[cfg(test)]
mod live {
    /// Prints every app with its verdict. Run with `cargo test live_apps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_apps() {
        let apps = crate::win::with_mta(|| super::scan(false)).unwrap();
        for a in &apps {
            println!(
                "{:<22} {:<6} {:<42.42} {:<28.28} {:>9}",
                a.verdict.label(),
                if a.system { "SYS" } else { "" },
                a.display_name,
                a.name,
                a.size.map(crate::win::fileinfo::format_bytes).unwrap_or_default()
            );
        }
        println!("{} apps", apps.len());
    }
}

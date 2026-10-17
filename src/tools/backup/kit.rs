//! Reinstall kit: what you need to get back up and running after a clean Windows install or
//! when restoring an image — drivers, Wi-Fi networks, your app list, BitLocker recovery key and
//! the firmware product key, with a README explaining how to use each.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

use super::sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Drivers,
    WiFi,
    Apps,
    BitLockerKey,
    ProductKey,
}

impl Part {
    pub const ALL: [Part; 5] = [Part::Drivers, Part::WiFi, Part::Apps, Part::BitLockerKey, Part::ProductKey];
    pub fn label(self) -> &'static str {
        match self {
            Part::Drivers => "Drivers (all third-party drivers)",
            Part::WiFi => "Wi-Fi networks and passwords",
            Part::Apps => "List of installed apps (+ winget restore file)",
            Part::BitLockerKey => "BitLocker recovery key",
            Part::ProductKey => "Windows product key stored in the firmware",
        }
    }
    pub fn needs_admin(self) -> bool {
        matches!(self, Part::Drivers | Part::BitLockerKey | Part::WiFi)
    }
    pub fn sensitive(self) -> bool {
        matches!(self, Part::WiFi | Part::BitLockerKey | Part::ProductKey)
    }
}

pub enum Progress {
    Step(String),
    Done(Result<String, String>),
}

pub fn kit_dir(dest: &Path) -> PathBuf {
    dest.join("WieldBackup").join(super::files::computer_name()).join("Reinstall kit")
}

/// Installed programs from the uninstall registry (64-bit, 32-bit and per-user).
pub fn installed_programs() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (hive, path) in [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Uninstall"),
    ] {
        let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) else { continue };
        for name in key.enum_keys().flatten() {
            let Ok(k) = key.open_subkey_with_flags(&name, KEY_READ) else { continue };
            let display: String = k.get_value("DisplayName").unwrap_or_default();
            let system: u32 = k.get_value("SystemComponent").unwrap_or(0);
            if display.trim().is_empty() || system == 1 || k.get_value::<String, _>("ParentKeyName").is_ok() {
                continue;
            }
            out.push((display, k.get_value("DisplayVersion").unwrap_or_default(), k.get_value("Publisher").unwrap_or_default()));
        }
    }
    out.sort_by_key(|a| a.0.to_lowercase());
    out.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    out
}

fn export_part(p: Part, dir: &Path) -> Result<String, String> {
    match p {
        Part::Drivers => {
            let d = dir.join("Drivers");
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            let (code, out) = sys::run("pnputil.exe", &["/export-driver", "*", &d.display().to_string()])?;
            if code != 0 {
                return Err(sys::first_error(&out));
            }
            let n = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
            Ok(format!("{n} driver package(s) exported"))
        }
        Part::WiFi => {
            let d = dir.join("WiFi");
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            let (_, out) = sys::run("netsh.exe", &["wlan", "export", "profile", "key=clear", &format!("folder={}", d.display())])?;
            let n = std::fs::read_dir(&d).map(|r| r.filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "xml")).count()).unwrap_or(0);
            if n == 0 {
                let _ = std::fs::remove_dir(&d);
                return Err(if out.to_lowercase().contains("wlansvc") || out.to_lowercase().contains("not running") {
                    "This PC has no Wi-Fi adapter.".into()
                } else {
                    "No Wi-Fi networks to export.".into()
                });
            }
            Ok(format!("{n} Wi-Fi network(s) exported"))
        }
        Part::Apps => {
            let apps = installed_programs();
            let mut csv = String::from("Name,Version,Publisher\r\n");
            for (n, v, p) in &apps {
                let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
                csv.push_str(&format!("{},{},{}\r\n", q(n), q(v), q(p)));
            }
            std::fs::write(dir.join("installed-apps.csv"), csv).map_err(|e| e.to_string())?;
            // winget can reinstall most apps in one go, where it's available.
            let json = dir.join("winget-apps.json");
            let winget = sys::run("winget.exe", &["export", "-o", &json.display().to_string(), "--accept-source-agreements", "--disable-interactivity"]);
            let extra = if winget.is_ok() && json.is_file() { " and a winget restore file" } else { "" };
            Ok(format!("{} app(s) listed{extra}", apps.len()))
        }
        Part::BitLockerKey => {
            let drive = sys::system_drive();
            let (code, out) = sys::run("manage-bde.exe", &["-protectors", "-get", &drive, "-Type", "RecoveryPassword"])?;
            if code != 0 {
                return Err(if out.contains("No key protectors") || out.to_lowercase().contains("not bitlocker") || out.contains("ERROR: An error occurred (code 0x80310000)") {
                    "BitLocker isn't on for this drive, so there's no recovery key.".into()
                } else {
                    sys::first_error(&out)
                });
            }
            let ids: Vec<&str> = out.lines().map(str::trim).filter(|l| l.starts_with("ID:") || l.starts_with("Password:") || l.chars().next().is_some_and(|c| c.is_ascii_digit()) && l.matches('-').count() == 7).collect();
            if ids.is_empty() {
                return Err("No recovery password found.".into());
            }
            std::fs::write(
                dir.join(format!("BitLocker recovery key {}.txt", drive.trim_end_matches(':'))),
                format!("BitLocker recovery key for {drive} on {}\r\nKeep this file private. Anyone with it can unlock the drive.\r\n\r\n{}\r\n", super::files::computer_name(), ids.join("\r\n")),
            )
            .map_err(|e| e.to_string())?;
            Ok("BitLocker recovery key saved".into())
        }
        Part::ProductKey => {
            let key = sys::powershell("(Get-CimInstance SoftwareLicensingService).OA3xOriginalProductKey")?;
            let key = key.trim();
            if key.is_empty() {
                return Err("No product key is stored in this PC's firmware (digital license or volume license).".into());
            }
            std::fs::write(dir.join("Windows product key.txt"), format!("Windows product key from this PC's firmware:\r\n{key}\r\n")).map_err(|e| e.to_string())?;
            Ok("Firmware product key saved".into())
        }
    }
}

const README: &str = "WIELD REINSTALL KIT\r\n===================\r\n\r\n\
Drivers\\            After reinstalling Windows, run as administrator:\r\n                    pnputil /add-driver \"<this folder>\\Drivers\\*.inf\" /subdirs /install\r\n                    (or use Wield > Backup & Restore > Reinstall kit > Restore drivers)\r\n\
WiFi\\               Run for each file:  netsh wlan add profile filename=\"<file>.xml\"\r\n\
installed-apps.csv  Everything that was installed, to reinstall by hand.\r\n\
winget-apps.json    Reinstall most apps at once:  winget import -i winget-apps.json\r\n\
BitLocker key       Needed to unlock the drive, e.g. after restoring a system image.\r\n\r\n\
These files can contain passwords and keys. Keep this folder private.\r\n";

pub fn export(parts: Vec<Part>, dest: PathBuf, tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let dir = kit_dir(&dest);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        send(Progress::Done(Err(format!("Can't write to {}: {e}", dir.display()))));
        return;
    }
    let _ = std::fs::write(dir.join("README.txt"), README);
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for p in parts {
        send(Progress::Step(p.label().into()));
        match export_part(p, &dir) {
            Ok(m) => done.push(m),
            Err(e) => failed.push(format!("{}: {e}", p.label())),
        }
    }
    let msg = format!("Saved to {}. {}", dir.display(), done.join("; "));
    send(Progress::Done(if failed.is_empty() { Ok(msg) } else { Err(format!("{msg}. Not saved: {}", failed.join("; "))) }));
}

/// Installs every driver from a kit's Drivers folder.
pub fn restore_drivers(dir: &Path) -> Result<String, String> {
    let pattern = format!("{}\\*.inf", dir.display());
    let (code, out) = sys::run("pnputil.exe", &["/add-driver", &pattern, "/subdirs", "/install"])?;
    // 3010 = success, restart required.
    if code == 0 || code == 3010 {
        let added = out.lines().find(|l| l.contains("Added driver packages") || l.contains("Total driver packages")).unwrap_or("").trim().to_string();
        Ok(format!("Drivers installed. {added}{}", if code == 3010 { " Restart to finish." } else { "" }))
    } else {
        Err(sys::first_error(&out))
    }
}

/// Imports every Wi-Fi profile from a kit's WiFi folder.
pub fn restore_wifi(dir: &Path) -> Result<String, String> {
    let mut n = 0;
    for e in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        if e.path().extension().is_some_and(|x| x == "xml") {
            let (code, _) = sys::run("netsh.exe", &["wlan", "add", "profile", &format!("filename={}", e.path().display()), "user=all"])?;
            if code == 0 {
                n += 1;
            }
        }
    }
    if n == 0 { Err("No Wi-Fi profiles were imported.".into()) } else { Ok(format!("{n} Wi-Fi network(s) restored.")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_installed_programs() {
        let apps = installed_programs();
        assert!(!apps.is_empty());
        assert!(apps.iter().all(|a| !a.0.is_empty()));
    }

    #[test]
    fn exports_app_list_without_admin() {
        let dest = std::env::temp_dir().join("wield-kit-test");
        let _ = std::fs::remove_dir_all(&dest);
        let (tx, rx) = std::sync::mpsc::channel();
        export(vec![Part::Apps], dest.clone(), tx, eframe::egui::Context::default());
        let r = rx.try_iter().find_map(|m| if let Progress::Done(r) = m { Some(r) } else { None }).unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert!(kit_dir(&dest).join("installed-apps.csv").is_file());
        assert!(kit_dir(&dest).join("README.txt").is_file());
        let _ = std::fs::remove_dir_all(dest);
    }
}

//! Changing menus safely: hide/show, Shift-only, delete (always backed up first), full backups
//! and restore, the Windows 11 classic menu switch and restarting Explorer.
//!
//! Hiding never deletes anything. For commands it adds `LegacyDisable` to a per-user copy of the
//! key (HKCU, merged over the machine-wide one), so it works without administrator rights and is
//! undone by removing that value. Shell extensions and app entries are hidden by adding their
//! CLSID to Explorer's per-user "Blocked" list.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::reg::{self, Hive, KeyDump, classes, write_err};
use super::scan::{BLOCKED, Entry, Kind, SCOPES, sendto_dir, sendto_hidden_dir};

pub fn data_dir() -> PathBuf {
    crate::settings::data_dir().unwrap_or_else(std::env::temp_dir).join("ctxmenu")
}

/// Removes `value` from a key; drops the per-user key again if Wield's flag was all that was in it.
fn clear_flag(hive: Hive, path: &str, value: &str) -> Result<(), String> {
    let Some(k) = hive.open(path) else { return Ok(()) };
    if !reg::has_value(&k, value) {
        return Ok(());
    }
    let k = hive.open_write(path).map_err(|e| write_err(hive, e))?;
    k.delete_value(value).map_err(|e| write_err(hive, e))?;
    if hive == Hive::User && k.enum_values().count() == 0 && k.enum_keys().count() == 0 {
        let _ = reg::delete_path(hive, path);
    }
    Ok(())
}

fn set_flag(path: &str, value: &str) -> Result<(), String> {
    Hive::User.create(path).and_then(|k| k.set_value(value, &"")).map_err(|e| write_err(Hive::User, e))
}

fn still_flagged(path: &str, value: &str) -> bool {
    reg::hkcr(path.trim_start_matches(r"Software\Classes\")).is_some_and(|k| reg::has_value(&k, value))
}

/// Hides or shows an entry. Returns a message for the user.
pub fn set_hidden(e: &Entry, hide: bool) -> Result<String, String> {
    match e.kind {
        Kind::Command => {
            let path = e.reg_path().unwrap();
            if hide {
                set_flag(&path, "LegacyDisable")?;
            } else {
                clear_flag(Hive::User, &path, "LegacyDisable")?;
                clear_flag(Hive::Machine, &path, "LegacyDisable")?;
                if still_flagged(&path, "LegacyDisable") {
                    return Err("Another program hid this entry for all users; showing it needs administrator rights.".into());
                }
            }
        }
        Kind::Extension | Kind::App => {
            let path = e.reg_path();
            // Some tools disable a handler by prefixing its CLSID with '-'.
            if !hide {
                if let Some(p) = &path {
                    for h in Hive::BOTH {
                        if let Some(v) = h.open(p).and_then(|k| reg::get_str(&k, "")).filter(|v| v.trim().starts_with("-{")) {
                            h.open_write(p).and_then(|k| k.set_value("", &v.trim().trim_start_matches('-').to_string())).map_err(|e| write_err(h, e))?;
                        }
                    }
                }
            }
            let clsid = e.clsid.clone().ok_or("This entry has no class ID to block.")?;
            if hide {
                Hive::User.create(BLOCKED).and_then(|k| k.set_value(&clsid, &format!("Hidden by Wield: {}", e.name))).map_err(|er| write_err(Hive::User, er))?;
            } else {
                for h in Hive::BOTH {
                    if let Some(k) = h.open(BLOCKED) {
                        if let Some(name) = k.enum_values().flatten().map(|(n, _)| n).find(|n| n.eq_ignore_ascii_case(&clsid)) {
                            h.open_write(BLOCKED).and_then(|k| k.delete_value(&name)).map_err(|er| write_err(h, er))?;
                        }
                    }
                }
            }
        }
        Kind::SendTo => {
            let (from, to) = if hide { (sendto_dir(), sendto_hidden_dir()) } else { (sendto_hidden_dir(), sendto_dir()) };
            std::fs::create_dir_all(&to).map_err(|er| er.to_string())?;
            std::fs::rename(from.join(&e.key), to.join(&e.key)).map_err(|er| er.to_string())?;
        }
        Kind::New => return Err("Items in the New menu can be removed (and restored later), not hidden.".into()),
    }
    Ok(format!("\"{}\" is {}.", e.name, if hide { "hidden" } else { "shown again" }))
}

pub fn set_shift_only(e: &Entry, on: bool) -> Result<String, String> {
    if e.kind != Kind::Command {
        return Err("Only commands can be moved behind Shift.".into());
    }
    let path = e.reg_path().unwrap();
    if on {
        set_flag(&path, "Extended")?;
    } else {
        clear_flag(Hive::User, &path, "Extended")?;
        clear_flag(Hive::Machine, &path, "Extended")?;
        if still_flagged(&path, "Extended") {
            return Err("This entry is set up for all users; changing it needs administrator rights.".into());
        }
    }
    Ok(format!("\"{}\" {}.", e.name, if on { "now only shows with Shift+right-click" } else { "shows in the normal menu again" }))
}

// ------------------------------------------------------------------ deleting and the recycle list

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Deleted {
    pub when: u64,
    pub name: String,
    pub kind: Kind,
    pub place: String,
    /// (hive, full key path, snapshot)
    pub keys: Vec<(Hive, String, KeyDump)>,
    /// For Send To items: where the file was moved.
    pub file: Option<(PathBuf, PathBuf)>,
}

fn deleted_path() -> PathBuf {
    data_dir().join("deleted.json")
}

pub fn deleted() -> Vec<Deleted> {
    std::fs::read_to_string(deleted_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_deleted(v: &[Deleted]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(deleted_path(), serde_json::to_string_pretty(v).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Deletes an entry after saving an exact copy of it, so it can be restored from the Deleted list.
pub fn delete(e: &Entry, elevated: bool) -> Result<String, String> {
    if !e.can_delete() {
        return Err("Windows' own entries can be hidden but not deleted.".into());
    }
    let mut rec = Deleted { when: crate::tools::backup::sys::now_unix(), name: e.name.clone(), kind: e.kind, place: e.place(), keys: Vec::new(), file: None };
    if e.kind == Kind::SendTo {
        let from = if e.hidden { sendto_hidden_dir() } else { sendto_dir() }.join(&e.key);
        let to = data_dir().join("SendTo-deleted").join(format!("{}-{}", rec.when, e.key));
        std::fs::create_dir_all(to.parent().unwrap()).map_err(|er| er.to_string())?;
        std::fs::rename(&from, &to).map_err(|er| er.to_string())?;
        rec.file = Some((sendto_dir().join(&e.key), to));
    } else {
        let path = e.reg_path().unwrap();
        if e.in_machine && !elevated {
            return Err("This entry is installed for all users; deleting it needs administrator rights. You can hide it instead.".into());
        }
        for h in Hive::BOTH {
            if let Some(d) = reg::dump_path(h, &path) {
                rec.keys.push((h, path.clone(), d));
            }
        }
        if rec.keys.is_empty() {
            return Err("The entry is already gone.".into());
        }
        // Save the copy before touching anything.
        let mut all = deleted();
        all.push(rec.clone());
        save_deleted(&all)?;
        for (h, p, _) in &rec.keys {
            if let Err(er) = reg::delete_path(*h, p) {
                // Put back whatever was removed and forget the record.
                for (h2, p2, d2) in &rec.keys {
                    let _ = reg::restore_path(*h2, p2, d2, true);
                }
                all.pop();
                let _ = save_deleted(&all);
                return Err(write_err(*h, er));
            }
        }
        return Ok(format!("\"{}\" deleted. You can restore it from Backups & deleted.", e.name));
    }
    let mut all = deleted();
    all.push(rec);
    save_deleted(&all)?;
    Ok(format!("\"{}\" deleted. You can restore it from Backups & deleted.", e.name))
}

pub fn restore_deleted(index: usize, elevated: bool) -> Result<String, String> {
    let mut all = deleted();
    let rec = all.get(index).cloned().ok_or("That item is no longer in the list.")?;
    if rec.keys.iter().any(|(h, _, _)| *h == Hive::Machine) && !elevated {
        return Err("This entry was installed for all users; restoring it needs administrator rights.".into());
    }
    for (h, p, d) in &rec.keys {
        reg::restore_path(*h, p, d, true).map_err(|e| write_err(*h, e))?;
    }
    if let Some((orig, saved)) = &rec.file {
        if orig.exists() {
            return Err(format!("{} already exists in Send To.", orig.display()));
        }
        std::fs::rename(saved, orig).map_err(|e| e.to_string())?;
    }
    all.remove(index);
    save_deleted(&all)?;
    Ok(format!("\"{}\" is back.", rec.name))
}

/// Removes a record from the Deleted list for good (the entry stays deleted).
pub fn forget_deleted(index: usize) -> Result<(), String> {
    let mut all = deleted();
    if index < all.len() {
        let rec = all.remove(index);
        if let Some((_, saved)) = rec.file {
            let _ = std::fs::remove_file(saved);
        }
    }
    save_deleted(&all)
}

// ------------------------------------------------------------------ full backups

pub const CLASSIC_KEY: &str = r"Software\Classes\CLSID\{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backup {
    pub when: u64,
    pub label: String,
    /// (hive, path, snapshot or None if the key didn't exist, exact) — exact keys are restored
    /// precisely even if that means removing things; other keys keep entries added later unless asked.
    pub keys: Vec<(Hive, String, Option<KeyDump>, bool)>,
    pub sendto: Vec<String>,
}

pub fn backups_dir() -> PathBuf {
    data_dir().join("backups")
}

/// Every key that makes up the menus right now.
fn backup_keys(entries: &[Entry]) -> Vec<(Hive, String, bool)> {
    let mut scopes: Vec<String> = SCOPES.iter().map(|s| s.0.to_string()).collect();
    scopes.extend(entries.iter().filter(|e| matches!(e.kind, Kind::Command | Kind::Extension)).map(|e| e.scope.clone()));
    scopes.sort_by_key(|s| s.to_lowercase());
    scopes.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let mut keys = Vec::new();
    for s in scopes {
        for h in Hive::BOTH {
            keys.push((h, classes(&format!(r"{s}\shell")), false));
            keys.push((h, classes(&format!(r"{s}\shellex\ContextMenuHandlers")), false));
        }
    }
    for e in entries.iter().filter(|e| e.kind == Kind::New) {
        for h in Hive::BOTH {
            keys.push((h, classes(&e.key), false));
        }
    }
    for h in Hive::BOTH {
        keys.push((h, BLOCKED.to_string(), true));
    }
    keys.push((Hive::User, CLASSIC_KEY.to_string(), true));
    keys
}

pub fn create_backup(entries: &[Entry], label: &str) -> Result<PathBuf, String> {
    let when = crate::tools::backup::sys::now_unix();
    let mut keys = Vec::new();
    for (h, p, exact) in backup_keys(entries) {
        let d = reg::dump_path(h, &p);
        // Keep the file small: only record missing keys where it matters.
        if d.is_some() || exact {
            keys.push((h, p, d, exact));
        }
    }
    let dir = backups_dir().join(when.to_string());
    std::fs::create_dir_all(dir.join("SendTo")).map_err(|e| e.to_string())?;
    let mut sendto = Vec::new();
    if let Ok(rd) = std::fs::read_dir(sendto_dir()) {
        for f in rd.flatten().filter(|f| f.path().is_file()) {
            let name = f.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let dest = dir.join("SendTo").join(&name);
            if std::fs::copy(f.path(), &dest).is_ok() {
                // A read-only copy would stop the backup folder from being deleted later.
                if let Ok(m) = std::fs::metadata(&dest) {
                    let mut p = m.permissions();
                    #[allow(clippy::permissions_set_readonly_false)]
                    p.set_readonly(false);
                    let _ = std::fs::set_permissions(&dest, p);
                }
                sendto.push(name);
            }
        }
    }
    let b = Backup { when, label: label.to_string(), keys, sendto };
    std::fs::write(dir.join("menus.json"), serde_json::to_string(&b).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Backups in Wield's folder, newest first: (folder, backup).
pub fn backups() -> Vec<(PathBuf, Backup)> {
    let mut v: Vec<(PathBuf, Backup)> = std::fs::read_dir(backups_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|d| read_backup(&d.path()).map(|b| (d.path(), b)))
        .collect();
    v.sort_by(|a, b| b.1.when.cmp(&a.1.when));
    v
}

/// Deletes a backup folder. Retries briefly: antivirus scanning just-written files can delay it.
pub fn delete_backup(dir: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..40 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(last)
}

pub fn read_backup(dir: &Path) -> Option<Backup> {
    std::fs::read_to_string(dir.join("menus.json")).ok().and_then(|s| serde_json::from_str(&s).ok())
}

pub struct RestoreReport {
    pub restored: usize,
    pub skipped_admin: usize,
    pub errors: Vec<String>,
}

/// Puts every menu back the way it was in the backup. A safety backup of the current state is made first.
pub fn restore_backup(dir: &Path, entries: &[Entry], remove_new: bool, elevated: bool) -> Result<RestoreReport, String> {
    let b = read_backup(dir).ok_or("This folder doesn't contain a Wield menu backup (menus.json).")?;
    create_backup(entries, "Automatic, before restoring a backup")?;
    let mut rep = RestoreReport { restored: 0, skipped_admin: 0, errors: Vec::new() };
    for (h, p, d, exact) in &b.keys {
        if *h == Hive::Machine && !elevated {
            // Only count it if something actually differs.
            if reg::dump_path(*h, p).as_ref() != d.as_ref() {
                rep.skipped_admin += 1;
            }
            continue;
        }
        let r = match d {
            Some(d) => reg::restore_path(*h, p, d, *exact || remove_new),
            None if *exact || remove_new => reg::delete_path(*h, p),
            None => Ok(()),
        };
        // Entries hidden or moved behind Shift after the backup: clear those flags too.
        if r.is_ok() && !*exact && !remove_new && p.ends_with(r"\shell") {
            if let Some(k) = h.open(p) {
                let subs: Vec<String> = k.enum_keys().flatten().collect();
                for s in subs.iter().filter(|s| !d.as_ref().is_some_and(|d| d.subkeys.iter().any(|x| x.name.eq_ignore_ascii_case(s)))) {
                    for flag in ["LegacyDisable", "Extended"] {
                        let _ = clear_flag(*h, &format!(r"{p}\{s}"), flag);
                    }
                }
            }
        }
        match r {
            Ok(()) => rep.restored += 1,
            Err(e) => rep.errors.push(format!("{}\\{p}: {}", h.short(), write_err(*h, e))),
        }
    }
    let src = dir.join("SendTo");
    for name in &b.sendto {
        let target = sendto_dir().join(name);
        if !target.exists() {
            let _ = std::fs::remove_file(sendto_hidden_dir().join(name));
            if let Err(e) = std::fs::copy(src.join(name), &target) {
                rep.errors.push(format!("Send To {name}: {e}"));
            }
        }
    }
    if remove_new {
        if let Ok(rd) = std::fs::read_dir(sendto_dir()) {
            for f in rd.flatten() {
                let n = f.file_name().to_string_lossy().to_string();
                if f.path().is_file() && !n.eq_ignore_ascii_case("desktop.ini") && !b.sendto.iter().any(|s| s.eq_ignore_ascii_case(&n)) {
                    let _ = std::fs::create_dir_all(sendto_hidden_dir());
                    let _ = std::fs::rename(f.path(), sendto_hidden_dir().join(&n));
                }
            }
        }
    }
    Ok(rep)
}

// ------------------------------------------------------------------ Windows 11 menu and Explorer

/// True when Windows 11's compact menu is replaced by the classic full menu.
pub fn classic_menu() -> bool {
    Hive::User.exists(&format!(r"{CLASSIC_KEY}\InprocServer32"))
}

pub fn set_classic_menu(on: bool) -> Result<(), String> {
    if on {
        Hive::User.create(&format!(r"{CLASSIC_KEY}\InprocServer32")).and_then(|k| k.set_value("", &"")).map_err(|e| e.to_string())
    } else {
        reg::delete_path(Hive::User, CLASSIC_KEY).map_err(|e| e.to_string())
    }
}

/// Restarts Explorer so menu changes that it caches (shell extensions, Windows 11 menu) apply.
pub fn restart_explorer() -> Result<(), String> {
    use crate::tools::backup::sys::run;
    run("taskkill.exe", &["/f", "/im", "explorer.exe"])?;
    // Windows restarts the shell by itself (AutoRestartShell); start it only if it didn't.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if run("tasklist.exe", &["/fi", "imagename eq explorer.exe", "/nh"]).is_ok_and(|(_, o)| o.to_lowercase().contains("explorer.exe")) {
            return Ok(());
        }
    }
    std::process::Command::new(crate::win::fileinfo::expand_env(r"%SystemRoot%\explorer.exe")).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Opens Regedit at a key.
pub fn open_in_regedit(hive: Hive, path: &str) {
    let full = format!(r"Computer\{}\{path}", if hive == Hive::Machine { "HKEY_LOCAL_MACHINE" } else { "HKEY_CURRENT_USER" });
    let _ = Hive::User.create(r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit").and_then(|k| k.set_value("LastKey", &full));
    crate::win::shell::open("regedit.exe");
}

#[cfg(test)]
mod tests {
    use super::super::custom::{self, Command, Custom, Targets};
    use super::super::scan;
    use super::*;

    /// A full backup undoes a hide made after it, and leaves no trace of the test entry.
    #[test]
    fn full_backup_restores_hidden_entries() {
        let _lock = super::super::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let c = custom::save(
            Custom {
                commands: vec![Command { label: "Wield backup test".into(), program: "notepad.exe".into(), ..Default::default() }],
                targets: Targets { types: ".wieldbak".into(), ..Default::default() },
                ..Default::default()
            },
            false,
        )
        .unwrap();
        Hive::User.create(r"Software\Classes\.wieldbak").unwrap();
        let find = || scan::scan_all().into_iter().find(|e| e.custom && e.name == "Wield backup test").unwrap();
        let entries = scan::scan_all();
        let t = std::time::Instant::now();
        let dir = create_backup(&entries, "test").unwrap();
        let took = t.elapsed();
        let size = std::fs::metadata(dir.join("menus.json")).unwrap().len();
        assert!(took.as_secs() < 20 && size < 30_000_000, "backup took {took:?}, {size} bytes");

        set_hidden(&find(), true).unwrap();
        assert!(find().hidden);
        let rep = restore_backup(&dir, &scan::scan_all(), false, false).unwrap();
        assert!(rep.errors.is_empty(), "{:?}", rep.errors);
        assert!(!find().hidden, "restore un-hides it");

        custom::remove(&c.id).unwrap();
        let _ = reg::delete_path(Hive::User, r"Software\Classes\.wieldbak");
        let _ = reg::delete_path(Hive::User, r"Software\Classes\SystemFileAssociations\.wieldbak");
        // Drop the test backup and the automatic one made before restoring.
        for (d, b) in backups().into_iter().take(2) {
            if b.label == "test" || b.label.starts_with("Automatic") {
                let _ = delete_backup(&d);
            }
        }
    }
}

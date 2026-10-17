//! Folder backups with robocopy (incremental, resumable, keeps timestamps and attributes).
//!
//! Layout: <destination>\WieldBackup\<PC name>\<folder name>\…, plus a manifest
//! (wield-backup.json) that remembers where each folder came from, for one-click restore.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use super::image::{History, record, save_log};
use super::sys;
use crate::win::fileinfo::expand_env;

/// The user's main folders, following OneDrive/redirected locations.
pub fn default_folders() -> Vec<(String, PathBuf)> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders").ok();
    let get = |name: &str| key.as_ref().and_then(|k| k.get_value::<String, _>(name).ok()).map(|v| PathBuf::from(expand_env(&v)));
    [
        ("Desktop", "Desktop"),
        ("Documents", "Personal"),
        ("Pictures", "My Pictures"),
        ("Videos", "My Video"),
        ("Music", "My Music"),
        ("Downloads", "{374DE290-123F-4565-9164-39C4925E467B}"),
    ]
    .into_iter()
    .filter_map(|(label, reg)| get(reg).filter(|p| p.is_dir()).map(|p| (label.to_string(), p)))
    .collect()
}

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

pub fn backup_root(dest: &Path) -> PathBuf {
    dest.join("WieldBackup").join(computer_name())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub when: u64,
    pub folders: Vec<(String, PathBuf)>,
}

pub fn read_manifest(dest: &Path) -> Option<Manifest> {
    std::fs::read_to_string(backup_root(dest).join("wield-backup.json")).ok().and_then(|s| serde_json::from_str(&s).ok())
}

pub enum Progress {
    Folder(String, usize, usize),
    Line(String),
    Done(Result<String, String>),
}

/// Pulls copied/failed counts out of robocopy's summary ("Files :  total  copied  skipped  mismatch  FAILED  extras").
pub fn parse_summary(lines: &[String]) -> Option<(u64, u64, u64)> {
    let l = lines.iter().rev().find(|l| l.trim_start().starts_with("Files :"))?;
    let nums: Vec<u64> = l.split(':').nth(1)?.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    (nums.len() >= 5).then(|| (nums[1], nums[2], nums[4]))
}

fn robocopy(src: &Path, dst: &Path, mirror: bool, newer_only: bool) -> Vec<String> {
    let mut args = vec![
        src.display().to_string(),
        dst.display().to_string(),
        if mirror { "/MIR".into() } else { "/E".into() },
        "/COPY:DAT".into(),
        "/DCOPY:DAT".into(),
        "/R:1".into(),
        "/W:1".into(),
        "/XJ".into(),
        "/MT:8".into(),
        "/NP".into(),
        "/NDL".into(),
        "/XF".into(),
        "desktop.ini".into(),
        "thumbs.db".into(),
    ];
    if newer_only {
        args.push("/XO".into());
    }
    args
}

/// Copies every folder into the backup. Robocopy exit codes below 8 mean success.
pub fn backup(folders: Vec<(String, PathBuf)>, dest: PathBuf, mirror: bool, tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let root = backup_root(&dest);
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    if let Err(e) = std::fs::create_dir_all(&root) {
        send(Progress::Done(Err(format!("Can't write to {}: {e}", root.display()))));
        return;
    }
    let mut log = Vec::new();
    let (mut copied, mut failed) = (0u64, 0u64);
    let mut errors = Vec::new();
    let n = folders.len();
    for (i, (name, src)) in folders.iter().enumerate() {
        send(Progress::Folder(name.clone(), i, n));
        let mut lines = Vec::new();
        let res = sys::run_streaming("robocopy.exe", &robocopy(src, &root.join(name), mirror, false), |l| {
            if !l.is_empty() {
                lines.push(l.to_string());
                if l.contains("New File") || l.contains("Newer") || l.contains("ERROR") {
                    let _ = tx.send(Progress::Line(l.to_string()));
                }
            }
        });
        match res {
            Ok(code) if code < 8 => {}
            Ok(code) => errors.push(format!("{name}: robocopy reported errors (code {code})")),
            Err(e) => errors.push(format!("{name}: {e}")),
        }
        if let Some((c, _, f)) = parse_summary(&lines) {
            copied += c;
            failed += f;
        }
        log.extend(lines);
    }
    let manifest = Manifest { when: sys::now_unix(), folders: folders.clone() };
    let _ = std::fs::write(root.join("wield-backup.json"), serde_json::to_string_pretty(&manifest).unwrap_or_default());
    let log_path = save_log("files", &log);
    let result = if errors.is_empty() && failed == 0 {
        Ok(format!("Backed up {n} folder(s): {copied} file(s) copied or updated."))
    } else {
        Err(format!("{copied} file(s) copied, {failed} failed. {}", errors.first().cloned().unwrap_or_else(|| "Some files were in use or not readable; see the log.".into())))
    };
    record(History { when: sys::now_unix(), kind: "Folders".into(), target: root.display().to_string(), ok: result.is_ok(), log: log_path.map(|p| p.display().to_string()) });
    send(Progress::Done(result));
}

/// Copies a backup back to the original folders. Never overwrites files that are newer on the PC.
pub fn restore(dest: PathBuf, tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let Some(m) = read_manifest(&dest) else {
        send(Progress::Done(Err(format!("No Wield backup for {} found on {}.", computer_name(), dest.display()))));
        return;
    };
    let root = backup_root(&dest);
    let n = m.folders.len();
    let mut errors = Vec::new();
    let mut log = Vec::new();
    for (i, (name, original)) in m.folders.iter().enumerate() {
        send(Progress::Folder(name.clone(), i, n));
        let res = sys::run_streaming("robocopy.exe", &robocopy(&root.join(name), original, false, true), |l| {
            if !l.is_empty() {
                log.push(l.to_string());
            }
        });
        if !matches!(res, Ok(c) if c < 8) {
            errors.push(name.clone());
        }
    }
    let _ = save_log("restore-files", &log);
    send(Progress::Done(if errors.is_empty() {
        Ok(format!("Restored {n} folder(s). Files newer on this PC were kept."))
    } else {
        Err(format!("Some folders didn't restore completely: {}", errors.join(", ")))
    }));
}

pub const TASK: &str = r"\Wield\Folder backup";

/// Daily folder backup for the current user (runs while signed in).
pub fn schedule(folders: &[(String, PathBuf)], dest: &Path, mirror: bool) -> Result<(), String> {
    let root = backup_root(dest);
    let script = crate::settings::data_dir().ok_or("No data folder")?.join("folder-backup.cmd");
    let mut body = String::from("@echo off\r\nrem Created by Wield: daily folder backup\r\n");
    for (name, src) in folders {
        let args: Vec<String> = robocopy(src, &root.join(name), mirror, false).into_iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a }).collect();
        body.push_str(&format!("robocopy {} /LOG+:\"%LOCALAPPDATA%\\Wield\\backup-logs\\scheduled-folders.log\"\r\n", args.join(" ")));
    }
    std::fs::create_dir_all(script.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&script, body).map_err(|e| e.to_string())?;
    let tr = format!("\"{}\"", script.display());
    let (code, out) = sys::run("schtasks.exe", &["/Create", "/F", "/TN", TASK, "/SC", "DAILY", "/ST", "13:00", "/TR", &tr])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

pub fn unschedule() -> Result<(), String> {
    let (code, out) = sys::run("schtasks.exe", &["/Delete", "/F", "/TN", TASK])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

pub fn scheduled() -> bool {
    sys::run("schtasks.exe", &["/Query", "/TN", TASK]).map(|(c, _)| c == 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_user_folders() {
        let f = default_folders();
        assert!(f.iter().any(|(n, _)| n == "Documents"));
    }

    #[test]
    fn backs_up_and_restores_a_folder() {
        let base = std::env::temp_dir().join("wield-files-test");
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        let dest = base.join("dest");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), b"beta").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        backup(vec![("Stuff".into(), src.clone())], dest.clone(), false, tx, eframe::egui::Context::default());
        let done = rx.try_iter().find_map(|m| if let Progress::Done(r) = m { Some(r) } else { None }).unwrap();
        assert!(done.is_ok(), "{done:?}");
        assert_eq!(std::fs::read(backup_root(&dest).join("Stuff").join("sub").join("b.txt")).unwrap(), b"beta");
        assert!(read_manifest(&dest).is_some());

        // Lose a file, change another, then restore: the lost one returns, the newer local edit stays.
        std::fs::remove_file(src.join("sub").join("b.txt")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2100));
        std::fs::write(src.join("a.txt"), b"edited later").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        restore(dest.clone(), tx, eframe::egui::Context::default());
        let done = rx.try_iter().find_map(|m| if let Progress::Done(r) = m { Some(r) } else { None }).unwrap();
        assert!(done.is_ok(), "{done:?}");
        assert_eq!(std::fs::read(src.join("sub").join("b.txt")).unwrap(), b"beta");
        assert_eq!(std::fs::read(src.join("a.txt")).unwrap(), b"edited later");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn robocopy_summary() {
        let l = vec!["               Total    Copied   Skipped  Mismatch    FAILED    Extras".into(), "    Files :         2         2         0         0         0         0".into()];
        assert_eq!(parse_summary(&l), Some((2, 0, 0)));
    }
}

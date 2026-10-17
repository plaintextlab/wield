//! Full system images with Windows' built-in `wbadmin`.
//!
//! The image (WindowsImageBackup\<PC>\…, VHDX files) contains every volume Windows needs to
//! boot. It is restored from the Windows Recovery Environment: Troubleshoot → Advanced options →
//! System Image Recovery, which works even when Windows itself no longer starts.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};

use super::sys::{self, BitLocker, DriveKind, Volume};
use crate::win::fileinfo::format_bytes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A drive letter like "E:".
    Drive(String),
    /// A network share like \\nas\backups.
    Share { path: String, user: String, password: String },
}

impl Target {
    pub fn arg(&self) -> String {
        match self {
            Target::Drive(d) => d.clone(),
            Target::Share { path, .. } => path.trim_end_matches('\\').to_string(),
        }
    }
    pub fn label(&self) -> String {
        match self {
            Target::Drive(d) => d.clone(),
            Target::Share { path, .. } => path.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Block,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub level: Level,
    pub text: String,
}

fn check(level: Level, text: impl Into<String>) -> Check {
    Check { level, text: text.into() }
}

/// What a backup will roughly need: used space on the system drive plus boot partitions.
pub fn estimated_size(volumes: &[Volume], extra: &[String]) -> u64 {
    let sys = sys::system_drive();
    volumes.iter().filter(|v| v.letter == sys || extra.contains(&v.letter)).map(Volume::used).sum::<u64>() + (1 << 30)
}

/// Everything that should be true before starting an image backup.
pub fn preflight(target: &Target, extra: &[String], volumes: &[Volume], elevated: bool, winre: Option<bool>, bitlocker: BitLocker) -> Vec<Check> {
    let mut out = Vec::new();
    if !elevated {
        out.push(check(Level::Block, "Creating a system image needs administrator rights."));
    }
    let sys = sys::system_drive();
    let need = estimated_size(volumes, extra);
    match target {
        Target::Drive(letter) => match volumes.iter().find(|v| &v.letter == letter) {
            None => out.push(check(Level::Block, format!("Drive {letter} isn't available."))),
            Some(t) => {
                if t.letter == sys {
                    out.push(check(Level::Block, "The backup can't go on the drive being backed up. Pick another drive."));
                }
                if extra.contains(&t.letter) {
                    out.push(check(Level::Block, format!("{} is both included in and the destination of the backup.", t.letter)));
                }
                if t.kind == DriveKind::Removable {
                    out.push(check(Level::Block, "Windows can't store a system image on a USB flash stick. Use an external hard drive or SSD."));
                }
                if t.kind == DriveKind::Network {
                    out.push(check(Level::Block, "Use the network share option (\\\\server\\share) instead of a mapped drive letter."));
                }
                if !t.fs.eq_ignore_ascii_case("NTFS") && !t.fs.eq_ignore_ascii_case("ReFS") {
                    out.push(check(Level::Block, format!("{} is formatted as {}. System images need NTFS.", t.letter, if t.fs.is_empty() { "unknown" } else { &t.fs })));
                }
                if t.free < need {
                    out.push(check(Level::Block, format!("Not enough space on {}: about {} needed, {} free.", t.letter, format_bytes(need), format_bytes(t.free))));
                } else if t.free < need * 2 {
                    out.push(check(Level::Warn, format!("{} free on {}. That fits this backup, but newer versions will replace older ones as space runs out.", format_bytes(t.free), t.letter)));
                } else {
                    out.push(check(Level::Ok, format!("Enough space: about {} needed, {} free.", format_bytes(need), format_bytes(t.free))));
                }
                let sys_disk = volumes.iter().find(|v| v.letter == sys).and_then(|v| v.disk);
                if sys_disk.is_some() && sys_disk == t.disk {
                    out.push(check(Level::Warn, "This drive is on the same physical disk as Windows. If that disk fails you lose both. Prefer an external drive."));
                } else if t.disk.is_some() {
                    out.push(check(Level::Ok, "The destination is on a different physical disk."));
                }
            }
        },
        Target::Share { path, .. } => {
            if !path.starts_with("\\\\") {
                out.push(check(Level::Block, "A network location must look like \\\\server\\share."));
            } else {
                out.push(check(
                    Level::Warn,
                    "On a network share each new image replaces the previous one; if a backup fails midway you may have none. Use a subfolder per backup to keep several.",
                ));
            }
        }
    }
    if sys::on_battery() {
        out.push(check(Level::Warn, "Running on battery. Plug in the charger: a backup interrupted by a dead battery is useless."));
    }
    match winre {
        Some(true) => out.push(check(Level::Ok, "The Windows Recovery Environment is enabled, so you can restore from the boot menu.")),
        Some(false) => out.push(check(Level::Warn, "The Windows Recovery Environment is off. Enable it below, or make a recovery USB, or you'll need Windows install media to restore.")),
        None => {}
    }
    if bitlocker == BitLocker::On {
        out.push(check(
            Level::Warn,
            "BitLocker is on: the image is stored unencrypted, so keep the backup drive safe (or encrypt it too). Save your recovery key in the Reinstall kit tab.",
        ));
    }
    out
}

pub enum Progress {
    Line(String),
    Percent(String, u32),
    Done(Result<String, String>),
}

/// "Creating a backup of volume Windows (C:), copied (45%)." -> ("Windows (C:)", 45)
pub fn parse_progress(line: &str) -> Option<(String, u32)> {
    let l = line.trim();
    let pct = l.rfind('%').and_then(|end| {
        let start = l[..end].rfind(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(0);
        l[start..end].parse::<u32>().ok()
    })?;
    let what = l.strip_prefix("Creating a backup of volume ").and_then(|r| r.split(", copied").next()).unwrap_or("Backing up").to_string();
    Some((what, pct.min(100)))
}

/// Runs `wbadmin start backup` and streams progress. Writes a full log next to the other Wield data.
pub fn run_backup(target: Target, extra: Vec<String>, tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let mut args = vec!["start".to_string(), "backup".into(), format!("-backupTarget:{}", target.arg()), "-allCritical".into()];
    let sys = sys::system_drive();
    let mut include = vec![sys];
    include.extend(extra);
    include.dedup();
    args.push(format!("-include:{}", include.join(",")));
    if let Target::Share { user, password, .. } = &target {
        if !user.is_empty() {
            args.push(format!("-user:{user}"));
            args.push(format!("-password:{password}"));
        }
    }
    args.push("-quiet".into());

    let mut log = Vec::new();
    let mut ok = false;
    let res = sys::run_streaming("wbadmin.exe", &args, |line| {
        if line.is_empty() {
            return;
        }
        log.push(line.to_string());
        if line.contains("successfully completed") {
            ok = true;
        }
        let msg = match parse_progress(line) {
            Some((what, p)) => Progress::Percent(what, p),
            None => Progress::Line(line.to_string()),
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
    let log_path = save_log("image", &log);
    let result = match res {
        Ok(0) if ok => Ok(format!("System image saved to {}.", target.label())),
        Ok(code) => Err(format!(
            "wbadmin stopped with code {code}: {}",
            log.iter().rev().find(|l| l.to_uppercase().starts_with("ERROR") || l.contains("failed")).cloned().unwrap_or_else(|| "see the log".into())
        )),
        Err(e) => Err(e),
    };
    record(History { when: sys::now_unix(), kind: "System image".into(), target: target.label(), ok: result.is_ok(), log: log_path.map(|p| p.display().to_string()) });
    let _ = tx.send(Progress::Done(result));
    ctx.request_repaint();
}

pub fn stop() -> Result<(), String> {
    let (code, out) = sys::run("wbadmin.exe", &["stop", "job", "-quiet"])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Version {
    pub time: String,
    pub location: String,
    pub id: String,
    pub can_recover: String,
}

impl Version {
    /// Bare Metal Recovery means the image can rebuild the PC from the boot menu.
    pub fn bootable(&self) -> bool {
        self.can_recover.to_lowercase().contains("bare metal")
    }
}

pub fn parse_versions(text: &str) -> Vec<Version> {
    let mut out = Vec::new();
    let mut cur = Version::default();
    for line in text.lines().map(str::trim) {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim().to_string();
        match k.trim() {
            "Backup time" => {
                if !cur.time.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                cur.time = v;
            }
            "Backup location" => cur.location = v,
            "Version identifier" => cur.id = v,
            "Can recover" => cur.can_recover = v,
            _ => {}
        }
    }
    if !cur.time.is_empty() {
        out.push(cur);
    }
    out.reverse();
    out
}

pub fn versions(target: &Target) -> Result<Vec<Version>, String> {
    let (code, out) = sys::run("wbadmin.exe", &["get", "versions", &format!("-backupTarget:{}", target.arg())])?;
    if code != 0 && !out.contains("Backup time") {
        if out.contains("no backups") || out.contains("No backup") {
            return Ok(Vec::new());
        }
        return Err(sys::first_error(&out));
    }
    Ok(parse_versions(&out))
}

// ------------------------------------------------------------------ recovery environment

/// Some(true) when Windows RE is enabled. Needs administrator rights.
pub fn winre_enabled() -> Result<bool, String> {
    let (code, out) = sys::run("reagentc.exe", &["/info"])?;
    if code != 0 {
        return Err(sys::first_error(&out));
    }
    parse_winre(&out).ok_or_else(|| "Unexpected output from reagentc.".into())
}

pub fn parse_winre(text: &str) -> Option<bool> {
    text.lines()
        .find(|l| l.contains("Windows RE status"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().eq_ignore_ascii_case("Enabled"))
}

pub fn enable_winre() -> Result<(), String> {
    let (code, out) = sys::run("reagentc.exe", &["/enable"])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

/// Restarts into Advanced startup (Troubleshoot → Advanced options → System Image Recovery).
pub fn restart_to_recovery() -> Result<(), String> {
    let (code, out) = sys::run("shutdown.exe", &["/r", "/o", "/f", "/t", "5"])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

// ------------------------------------------------------------------ schedule

pub const TASK: &str = r"\Wield\System image backup";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frequency {
    Weekly,
    Monthly,
}

pub fn schedule(target: &Target, freq: Frequency) -> Result<(), String> {
    let Target::Drive(d) = target else {
        return Err("Scheduled backups need a local drive; network shares would require storing your password.".into());
    };
    let run = format!("wbadmin start backup -backupTarget:{d} -allCritical -include:{} -quiet", sys::system_drive());
    let mut args = vec!["/Create", "/F", "/TN", TASK, "/RU", "SYSTEM", "/RL", "HIGHEST", "/ST", "02:00", "/TR", &run];
    match freq {
        Frequency::Weekly => args.extend(["/SC", "WEEKLY", "/D", "SUN"]),
        Frequency::Monthly => args.extend(["/SC", "MONTHLY", "/D", "1"]),
    }
    let (code, out) = sys::run("schtasks.exe", &args)?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

pub fn unschedule() -> Result<(), String> {
    let (code, out) = sys::run("schtasks.exe", &["/Delete", "/F", "/TN", TASK])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

/// (next run, last result, command) when scheduled.
pub fn scheduled() -> Option<(String, String, String)> {
    let (code, out) = sys::run("schtasks.exe", &["/Query", "/TN", TASK, "/FO", "LIST", "/V"]).ok()?;
    if code != 0 {
        return None;
    }
    let get = |k: &str| out.lines().find(|l| l.trim_start().starts_with(k)).and_then(|l| l.split_once(':')).map(|(_, v)| v.trim().to_string()).unwrap_or_default();
    Some((get("Next Run Time"), get("Last Result"), get("Task To Run")))
}

// ------------------------------------------------------------------ history & logs

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    pub when: u64,
    pub kind: String,
    pub target: String,
    pub ok: bool,
    pub log: Option<String>,
}

fn history_path() -> Option<PathBuf> {
    Some(crate::settings::data_dir()?.join("backup-history.json"))
}

pub fn history() -> Vec<History> {
    history_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn record(h: History) {
    // Tests run real backups into temp folders; keep them out of the user's history.
    if cfg!(test) {
        return;
    }
    let mut all = history();
    all.push(h);
    if let Some(p) = history_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string_pretty(&all).unwrap_or_default());
    }
}

pub fn save_log(kind: &str, lines: &[String]) -> Option<PathBuf> {
    let dir = crate::settings::data_dir()?.join("backup-logs");
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("{}-{kind}.log", sys::now_unix()));
    std::fs::write(&p, lines.join("\r\n")).ok()?;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(letter: &str, kind: DriveKind, fs: &str, total: u64, free: u64, disk: u32) -> Volume {
        Volume { letter: letter.into(), label: String::new(), fs: fs.into(), kind, total, free, disk: Some(disk) }
    }

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("Creating a backup of volume Windows (C:), copied (45%)."), Some(("Windows (C:)".into(), 45)));
        assert_eq!(parse_progress("Creating a backup of volume (C:), copied (100%)."), Some(("(C:)".into(), 100)));
        assert_eq!(parse_progress("Creating a shadow copy of the volumes specified for backup..."), None);
    }

    #[test]
    fn versions_output() {
        let t = "wbadmin 1.0 - Backup command-line tool\r\n\r\nBackup time: 10/14/2026 3:00 AM\r\nBackup location: Fixed Disk labeled Backup(E:)\r\nVersion identifier: 10/14/2026-07:00\r\nCan recover: Volume(s), File(s), Application(s), Bare Metal Recovery, System State\r\nSnapshot ID: {x}\r\n\r\n\
                 Backup time: 10/15/2026 3:00 AM\r\nBackup location: Fixed Disk labeled Backup(E:)\r\nVersion identifier: 10/15/2026-07:00\r\nCan recover: Volume(s), File(s), Application(s), Bare Metal Recovery, System State\r\n";
        let v = parse_versions(t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "10/15/2026-07:00", "newest first");
        assert!(v[0].bootable());
    }

    #[test]
    fn winre_output() {
        assert_eq!(parse_winre("Windows Recovery Environment (Windows RE) and system reset configuration\r\n    Windows RE status:         Enabled\r\n"), Some(true));
        assert_eq!(parse_winre("    Windows RE status:         Disabled\r\n"), Some(false));
    }

    #[test]
    fn preflight_blocks_bad_targets() {
        let sys = sys::system_drive();
        let vols = vec![
            vol(&sys, DriveKind::Fixed, "NTFS", 500 << 30, 300 << 30, 0),
            vol("E:", DriveKind::Fixed, "NTFS", 1000 << 30, 900 << 30, 1),
            vol("F:", DriveKind::Removable, "NTFS", 64 << 30, 60 << 30, 2),
            vol("G:", DriveKind::Fixed, "exFAT", 1000 << 30, 900 << 30, 3),
            vol("H:", DriveKind::Fixed, "NTFS", 1000 << 30, 10 << 30, 4),
            vol("D:", DriveKind::Fixed, "NTFS", 500 << 30, 400 << 30, 0),
        ];
        let blocks = |t: &str| preflight(&Target::Drive(t.into()), &[], &vols, true, Some(true), BitLocker::Off).iter().filter(|c| c.level == Level::Block).count();
        assert_eq!(blocks("E:"), 0, "good external disk");
        assert!(blocks(&sys) > 0, "can't back up onto itself");
        assert!(blocks("F:") > 0, "flash stick");
        assert!(blocks("G:") > 0, "exFAT");
        assert!(blocks("H:") > 0, "too small");
        let d = preflight(&Target::Drive("D:".into()), &[], &vols, true, Some(true), BitLocker::Off);
        assert!(d.iter().any(|c| c.level == Level::Warn && c.text.contains("same physical disk")));
        assert!(preflight(&Target::Drive("E:".into()), &[], &vols, false, None, BitLocker::On).iter().any(|c| c.level == Level::Block));
    }
}

//! System Restore: protection settings, restore points, restoring and cleaning up.
//! Everything here needs administrator rights.

use serde::Deserialize;

use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE};

use super::sys::{self, powershell};

const SR_KEY: &str = r"HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore";

#[derive(Clone, Debug, Deserialize)]
pub struct RestorePoint {
    #[serde(rename = "SequenceNumber")]
    pub seq: u32,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "RestorePointType")]
    pub kind: u32,
    /// ISO 8601 local time.
    #[serde(rename = "Time")]
    pub time: String,
}

impl RestorePoint {
    pub fn kind_label(&self) -> &'static str {
        let d = self.description.to_lowercase();
        if d.contains("windows update") || d.contains("windows modules installer") {
            return "Windows Update";
        }
        if d.contains("wield") {
            return "Wield";
        }
        match self.kind {
            0 => "App install",
            1 => "App removal",
            10 => "Driver install",
            12 => "Manual / settings",
            13 => "Cancelled",
            _ => "System",
        }
    }
}

pub fn parse_points(json: &str) -> Result<Vec<RestorePoint>, String> {
    let j = json.trim();
    if j.is_empty() {
        return Ok(Vec::new());
    }
    let mut v: Vec<RestorePoint> = if j.starts_with('[') {
        serde_json::from_str(j).map_err(|e| e.to_string())?
    } else {
        vec![serde_json::from_str(j).map_err(|e| e.to_string())?]
    };
    v.sort_by(|a, b| b.seq.cmp(&a.seq));
    Ok(v)
}

pub fn list() -> Result<Vec<RestorePoint>, String> {
    let json = powershell(
        "@(Get-ComputerRestorePoint -ErrorAction Stop | ForEach-Object { [pscustomobject]@{ SequenceNumber=[int]$_.SequenceNumber; \
         Description=[string]$_.Description; RestorePointType=[int]$_.RestorePointType; \
         Time=$_.ConvertToDateTime($_.CreationTime).ToString('yyyy-MM-dd HH:mm') } }) | ConvertTo-Json -Compress",
    )?;
    parse_points(&json)
}

/// Creates a restore point now, even if Windows' once-a-day limit would normally skip it.
/// Returns the new point after checking it really exists.
pub fn create(description: &str) -> Result<RestorePoint, String> {
    let before = list()?.first().map(|p| p.seq).unwrap_or(0);
    let desc = description.replace('\'', "''");
    powershell(&format!(
        "$k='{SR_KEY}'; $old=(Get-ItemProperty $k -Name SystemRestorePointCreationFrequency -ErrorAction SilentlyContinue).SystemRestorePointCreationFrequency; \
         Set-ItemProperty $k -Name SystemRestorePointCreationFrequency -Value 0 -Type DWord; \
         try {{ Checkpoint-Computer -Description '{desc}' -RestorePointType MODIFY_SETTINGS -ErrorAction Stop }} \
         finally {{ if ($null -eq $old) {{ Remove-ItemProperty $k -Name SystemRestorePointCreationFrequency -ErrorAction SilentlyContinue }} else {{ Set-ItemProperty $k -Name SystemRestorePointCreationFrequency -Value $old }} }}"
    ))?;
    let points = list()?;
    points
        .into_iter()
        .find(|p| p.seq > before)
        .ok_or_else(|| "Windows didn't create the restore point. Is System Protection turned on for the system drive?".into())
}

/// Schedules a restore to `seq`; it runs during the next restart.
pub fn restore(seq: u32) -> Result<(), String> {
    let out = powershell(&format!("([wmiclass]'\\\\.\\root\\default:SystemRestore').Restore({seq}).ReturnValue"))?;
    match out.trim() {
        "0" => Ok(()),
        code => Err(format!("Windows refused to start the restore (code {code}).")),
    }
}

pub fn restart_now() -> Result<(), String> {
    let (code, out) = sys::run("shutdown.exe", &["/r", "/t", "10", "/c", "Wield: restarting to finish System Restore."])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

/// Usage of shadow storage per volume, from `vssadmin list shadowstorage`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Storage {
    pub volume: String,
    pub used: String,
    pub allocated: String,
    pub max: String,
}

pub fn parse_storage(text: &str) -> Vec<Storage> {
    let mut out = Vec::new();
    let mut cur: Option<Storage> = None;
    for line in text.lines().map(str::trim) {
        let value = |l: &str| l.split_once(':').map(|(_, v)| v.trim().to_string()).unwrap_or_default();
        if line.starts_with("For volume:") {
            if let Some(c) = cur.take() {
                out.push(c);
            }
            let v = value(line);
            let letter = v.split(')').next().unwrap_or("").trim_start_matches('(').to_string();
            cur = Some(Storage { volume: letter, ..Default::default() });
        } else if let Some(c) = cur.as_mut() {
            if line.starts_with("Used Shadow Copy Storage space") {
                c.used = value(line);
            } else if line.starts_with("Allocated Shadow Copy Storage space") {
                c.allocated = value(line);
            } else if line.starts_with("Maximum Shadow Copy Storage space") {
                c.max = value(line);
            }
        }
    }
    out.extend(cur);
    out
}

pub fn storage() -> Result<Vec<Storage>, String> {
    let (code, out) = sys::run("vssadmin.exe", &["list", "shadowstorage"])?;
    if code != 0 && !out.contains("No items found") {
        return Err(sys::first_error(&out));
    }
    Ok(parse_storage(&out))
}

/// Drive letters with System Protection turned on.
///
/// Windows records this under SPP\Clients as a list of volume IDs. That key is locked down and its
/// format varies, so Wield matches each drive's volume ID and drive letter against it, and when it
/// can't be read (or names no drive) falls back to the state Wield last set, then to "System
/// Restore is on and the drive has restore point storage".
pub fn protected_drives(storage: &[Storage]) -> Vec<String> {
    let spp = spp_text();
    let listed = |letter: &str| {
        spp.as_ref().is_some_and(|t| parse_protected(t).iter().any(|d| d == letter) || sys::volume_guid(letter).is_some_and(|g| t.to_lowercase().contains(&g)))
    };
    let volumes = sys::volumes();
    let any_listed = volumes.iter().any(|v| listed(&v.letter));
    let remembered = remembered();
    let sr_on = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SystemRestore", KEY_QUERY_VALUE)
        .ok()
        .and_then(|k| k.get_value::<u32, _>("RPSessionInterval").ok())
        .is_some_and(|v| v != 0);
    let mut drives: Vec<String> = volumes
        .into_iter()
        .map(|v| v.letter)
        .filter(|d| {
            if any_listed {
                return listed(d);
            }
            match remembered.get(d) {
                Some(on) => *on,
                None => sr_on && storage.iter().any(|s| &s.volume == d),
            }
        })
        .collect();
    drives.sort();
    drives
}

/// The SPP\Clients System Restore value as text (any registry type), if it can be read.
fn spp_text() -> Option<String> {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\SPP\Clients", KEY_QUERY_VALUE).ok()?;
    let v = k.get_raw_value("{09F7EDC5-294E-4180-AF6A-FB0E6A0E9513}").ok()?;
    let units: Vec<u16> = v.bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&units).replace('\0', "\n"))
}

fn remembered_path() -> Option<std::path::PathBuf> {
    Some(crate::settings::data_dir()?.join("restore-protection.json"))
}

/// Protection state per drive as Wield last set it.
fn remembered() -> std::collections::HashMap<String, bool> {
    remembered_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn remember(drive: &str, on: bool) {
    let mut m = remembered();
    m.insert(drive.to_uppercase(), on);
    if let Some(p) = remembered_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_string(&m).unwrap_or_default());
    }
}

/// Entries look like `\\?\Volume{guid}\:(C%3A)`.
pub fn parse_protected(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let up = l.to_uppercase().replace("%3A", ":");
            let i = up.rfind(":)")?;
            let c = up[..i].chars().last()?;
            c.is_ascii_alphabetic().then(|| format!("{c}:"))
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

pub fn set_protection(drive: &str, on: bool) -> Result<(), String> {
    let cmd = if on { "Enable-ComputerRestore" } else { "Disable-ComputerRestore" };
    powershell(&format!("{cmd} -Drive '{drive}\\' -ErrorAction Stop"))?;
    remember(drive, on);
    Ok(())
}

/// Sets how much of the drive restore points may use (percent).
pub fn set_max_size(drive: &str, percent: u32) -> Result<(), String> {
    let p = percent.clamp(1, 50);
    let (code, out) = sys::run("vssadmin.exe", &["resize", "shadowstorage", &format!("/For={drive}"), &format!("/On={drive}"), &format!("/MaxSize={p}%")])?;
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

/// Deletes one restore point by removing the shadow copy created at the same moment.
pub fn delete(point: &RestorePoint) -> Result<(), String> {
    let drive = sys::system_drive();
    powershell(&format!(
        "$t=[datetime]::ParseExact('{}','yyyy-MM-dd HH:mm',$null); \
         $v=(Get-CimInstance Win32_Volume | Where-Object {{ $_.DriveLetter -eq '{drive}' }}).DeviceID; \
         $s=Get-CimInstance Win32_ShadowCopy | Where-Object {{ $_.VolumeName -eq $v -and [math]::Abs(($_.InstallDate - $t).TotalMinutes) -lt 3 }} | Select-Object -First 1; \
         if (-not $s) {{ throw 'Could not find the shadow copy for this restore point.' }}; \
         Remove-CimInstance -InputObject $s -ErrorAction Stop",
        point.time
    ))
    .map(|_| ())
}

pub fn delete_all() -> Result<(), String> {
    let drive = sys::system_drive();
    let (code, out) = sys::run("vssadmin.exe", &["delete", "shadows", &format!("/for={drive}"), "/all", "/quiet"])?;
    if code == 0 || out.contains("No items found") { Ok(()) } else { Err(sys::first_error(&out)) }
}

pub const DAILY_TASK: &str = r"\Wield\Daily restore point";

pub fn daily_scheduled() -> bool {
    sys::run("schtasks.exe", &["/Query", "/TN", DAILY_TASK]).map(|(c, _)| c == 0).unwrap_or(false)
}

/// A daily task (as SYSTEM) that creates a restore point; Windows still keeps at most one per 24 h.
pub fn set_daily(on: bool) -> Result<(), String> {
    let (code, out) = if on {
        sys::run(
            "schtasks.exe",
            &[
                "/Create",
                "/F",
                "/TN",
                DAILY_TASK,
                "/SC",
                "DAILY",
                "/ST",
                "12:00",
                "/RU",
                "SYSTEM",
                "/RL",
                "HIGHEST",
                "/TR",
                "powershell.exe -NoProfile -WindowStyle Hidden -Command \"Checkpoint-Computer -Description 'Wield daily restore point' -RestorePointType MODIFY_SETTINGS\"",
            ],
        )?
    } else {
        sys::run("schtasks.exe", &["/Delete", "/F", "/TN", DAILY_TASK])?
    };
    if code == 0 { Ok(()) } else { Err(sys::first_error(&out)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_restore_points() {
        let one = r#"{"SequenceNumber":7,"Description":"Windows Update","RestorePointType":17,"Time":"2026-10-10 03:00"}"#;
        let p = parse_points(one).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].kind_label(), "Windows Update");
        let many = r#"[{"SequenceNumber":5,"Description":"Installed X","RestorePointType":0,"Time":"2026-10-01 10:00"},{"SequenceNumber":6,"Description":"Wield - before startup changes","RestorePointType":12,"Time":"2026-10-02 10:00"}]"#;
        let p = parse_points(many).unwrap();
        assert_eq!(p[0].seq, 6, "newest first");
        assert_eq!(p[0].kind_label(), "Wield");
        assert_eq!(p[1].kind_label(), "App install");
        assert!(parse_points("").unwrap().is_empty());
    }

    #[test]
    fn parses_shadow_storage() {
        let text = "vssadmin 1.1 - Volume Shadow Copy Service administrative command-line tool\r\n\r\n\
            Shadow Copy Storage association\r\n   For volume: (C:)\\\\?\\Volume{1234}\\\r\n   Shadow Copy Storage volume: (C:)\\\\?\\Volume{1234}\\\r\n\
            \x20  Used Shadow Copy Storage space: 1.24 GB (1%)\r\n   Allocated Shadow Copy Storage space: 1.81 GB (1%)\r\n   Maximum Shadow Copy Storage space: 23.8 GB (5%)\r\n";
        let s = parse_storage(text);
        assert_eq!(s, vec![Storage { volume: "C:".into(), used: "1.24 GB (1%)".into(), allocated: "1.81 GB (1%)".into(), max: "23.8 GB (5%)".into() }]);
    }

    #[test]
    fn detects_protection_without_the_locked_key() {
        // Without admin rights the SPP key can't be read; the fallback must still answer.
        let sys = sys::system_drive();
        let with_storage = vec![Storage { volume: sys.clone(), ..Default::default() }];
        let d = protected_drives(&with_storage);
        assert!(d.iter().all(|x| x.len() == 2 && x.ends_with(':')), "{d:?}");
    }

    #[test]
    fn parses_other_protected_formats() {
        assert_eq!(parse_protected(r"\\?\Volume{abc}\:(c%3a)"), vec!["C:".to_string()]);
        assert_eq!(parse_protected("\\\\?\\Volume{abc}\\:(E:)\n\n"), vec!["E:".to_string()]);
        assert!(parse_protected(r"\\?\Volume{abc}\").is_empty());
        let g = sys::volume_guid(&sys::system_drive()).expect("system volume id");
        assert!(g.starts_with('{') && g.ends_with('}') && g.len() == 38, "{g}");
    }

    #[test]
    fn parses_protected_drives() {
        let t = "\\\\?\\Volume{abc}\\:(C%3A)\r\n\\\\?\\Volume{def}\\:(D%3A)\r\n";
        assert_eq!(parse_protected(t), vec!["C:".to_string(), "D:".to_string()]);
    }
}

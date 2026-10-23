//! Windows Update: pause/resume (the same settings Windows' own Pause button writes), searching
//! for updates, installing the ones you pick, and update history.

use serde::Deserialize;
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ};

use crate::tools::backup::sys;

const UX: &str = r"SOFTWARE\Microsoft\WindowsUpdate\UX\Settings";
const PAUSE_VALUES: [&str; 6] = [
    "PauseUpdatesStartTime",
    "PauseUpdatesExpiryTime",
    "PauseFeatureUpdatesStartTime",
    "PauseFeatureUpdatesEndTime",
    "PauseQualityUpdatesStartTime",
    "PauseQualityUpdatesEndTime",
];

/// "2026-11-20T08:30:00Z" for a Unix time (UTC).
pub fn iso(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs = unix % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// Parses the start of an ISO date ("2026-11-20T…") into a Unix time.
pub fn parse_iso(s: &str) -> Option<u64> {
    let d = s.get(..10)?;
    let mut it = d.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (it.next()??, it.next()??, it.next()??);
    let (hh, mm) = s.get(11..16).and_then(|t| t.split_once(':')).and_then(|(h, m)| Some((h.parse::<i64>().ok()?, m.parse::<i64>().ok()?))).unwrap_or((0, 0));
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60).ok()
}

/// When updates are paused until, if they are.
pub fn paused_until() -> Option<u64> {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(UX, KEY_READ).ok()?;
    let v: String = k.get_value("PauseUpdatesExpiryTime").ok()?;
    parse_iso(&v).filter(|t| *t > sys::now_unix())
}

/// Pauses all Windows updates for `days` (Windows allows up to 35). Needs administrator rights.
pub fn pause(days: u32) -> Result<String, String> {
    let days = days.clamp(1, 35);
    let now = sys::now_unix();
    let end = now + days as u64 * 86_400;
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(UX).map_err(denied)?.0;
    let (start, stop) = (iso(now), iso(end));
    for (name, v) in [
        ("PauseUpdatesStartTime", &start),
        ("PauseUpdatesExpiryTime", &stop),
        ("PauseFeatureUpdatesStartTime", &start),
        ("PauseFeatureUpdatesEndTime", &stop),
        ("PauseQualityUpdatesStartTime", &start),
        ("PauseQualityUpdatesEndTime", &stop),
    ] {
        k.set_value(name, v).map_err(denied)?;
    }
    Ok(format!("Windows updates are paused until {}.", &stop[..10]))
}

pub fn resume() -> Result<String, String> {
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(UX, KEY_ALL_ACCESS).map_err(denied)?;
    for v in PAUSE_VALUES {
        match k.delete_value(v) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(denied(e)),
            _ => {}
        }
    }
    Ok("Windows updates are resumed. Windows will check for updates soon.".into())
}

fn denied(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied { "Needs administrator rights.".into() } else { e.to_string() }
}

/// The Windows Update service's state, e.g. ("Stopped", "Manual"). "Disabled" means updates can't run at all.
pub fn service() -> (String, String) {
    let out = sys::powershell("$s=Get-CimInstance Win32_Service -Filter \"Name='wuauserv'\"; \"$($s.State)|$($s.StartMode)\"").unwrap_or_default();
    let (a, b) = out.trim().split_once('|').unwrap_or(("Unknown", "Unknown"));
    (a.to_string(), b.to_string())
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct WinUpdate {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "Kb")]
    pub kb: String,
    #[serde(rename = "Category")]
    pub category: String,
    #[serde(rename = "Severity")]
    pub severity: String,
    #[serde(rename = "Size")]
    pub size: u64,
    #[serde(rename = "Optional")]
    pub optional: bool,
}

const SEARCH: &str = r#"
$s = New-Object -ComObject Microsoft.Update.Session
$s.ClientApplicationID = 'Wield'
$q = $s.CreateUpdateSearcher()
$q.Online = $true
$r = $q.Search("IsInstalled=0 and Type='Software' and IsHidden=0")
$out = foreach ($u in $r.Updates) {
  $cat = ''
  if ($u.Categories.Count -gt 0) { $cat = [string]$u.Categories.Item(0).Name }
  [pscustomobject]@{
    Id = [string]$u.Identity.UpdateID
    Title = [string]$u.Title
    Kb = (@($u.KBArticleIDs) | ForEach-Object { "KB$_" }) -join ', '
    Category = $cat
    Severity = [string]$u.MsrcSeverity
    Size = [int64]$u.MaxDownloadSize
    Optional = -not [bool]$u.AutoSelectOnWebSites
  }
}
@($out) | ConvertTo-Json -Compress
"#;

/// Searches Windows Update for updates this PC doesn't have. Takes 10–60 seconds.
pub fn search() -> Result<Vec<WinUpdate>, String> {
    let json = sys::powershell(SEARCH).map_err(|e| if e.contains("0x8024402C") || e.contains("0x80072EE7") { "Can't reach Windows Update. Check your internet connection.".into() } else { e })?;
    let t = json.trim();
    if t.is_empty() || t == "null" {
        return Ok(Vec::new());
    }
    serde_json::from_str(t).map_err(|e| format!("Unexpected answer from Windows Update: {e}"))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct HistoryItem {
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "Title")]
    pub title: String,
    /// 2 succeeded, 3 succeeded with errors, 4 failed, 5 cancelled.
    #[serde(rename = "Result")]
    pub result: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct History {
    #[serde(rename = "LastSearch")]
    pub last_search: String,
    #[serde(rename = "LastInstall")]
    pub last_install: String,
    #[serde(rename = "Items")]
    pub items: Vec<HistoryItem>,
}

const HISTORY: &str = r#"
$s = New-Object -ComObject Microsoft.Update.Session
$q = $s.CreateUpdateSearcher()
$n = $q.GetTotalHistoryCount()
$items = @()
if ($n -gt 0) {
  $items = foreach ($h in $q.QueryHistory(0, [Math]::Min($n, 40))) {
    if ($h.Title) { [pscustomobject]@{ Date = ([datetime]$h.Date).ToLocalTime().ToString('yyyy-MM-dd HH:mm'); Title = [string]$h.Title; Result = [int]$h.ResultCode } }
  }
}
$a = (New-Object -ComObject Microsoft.Update.AutoUpdate).Results
$f = { param($d) if ($d) { ([datetime]$d).ToLocalTime().ToString('yyyy-MM-dd HH:mm') } else { '' } }
[pscustomobject]@{ LastSearch = (& $f $a.LastSearchSuccessDate); LastInstall = (& $f $a.LastInstallationSuccessDate); Items = @($items) } | ConvertTo-Json -Compress -Depth 4
"#;

pub fn history() -> Result<History, String> {
    let json = sys::powershell(HISTORY)?;
    serde_json::from_str(json.trim()).map_err(|e| format!("Unexpected update history: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_roundtrip() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_793_000_000), "2026-10-26T07:33:20Z");
        assert_eq!(parse_iso("2026-10-26T07:33:20Z"), Some(1_793_000_000 - 20));
        assert_eq!(parse_iso(&iso(1_800_000_000)).map(|t| t / 60), Some(1_800_000_000 / 60));
        assert!(parse_iso("garbage").is_none());
    }

    #[test]
    fn reads_pause_state_and_history() {
        let _ = paused_until();
        let h = history().unwrap();
        assert!(h.items.iter().all(|i| !i.title.is_empty()));
    }
}

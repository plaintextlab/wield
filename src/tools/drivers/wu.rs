//! Driver updates from Windows Update: the catalog hardware makers publish their drivers to.

use std::sync::mpsc::Sender;

use serde::Deserialize;

use crate::tools::backup::sys;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct DriverUpdate {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Title")]
    pub title: String,
    #[serde(rename = "Manufacturer")]
    pub manufacturer: String,
    #[serde(rename = "Model")]
    pub model: String,
    #[serde(rename = "Class")]
    pub class: String,
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "Size")]
    pub size: u64,
    #[serde(rename = "HardwareId")]
    pub hardware_id: String,
}

/// True when a policy keeps drivers out of Windows Update (common on work PCs).
pub fn drivers_blocked_by_policy() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate")
        .and_then(|k| k.get_value::<u32, _>("ExcludeWUDriversInQualityUpdate"))
        .is_ok_and(|v| v == 1)
}

const SEARCH: &str = r#"
$s = New-Object -ComObject Microsoft.Update.Session
$s.ClientApplicationID = 'Wield'
$q = $s.CreateUpdateSearcher()
$q.ServerSelection = 2
$q.Online = $true
$r = $q.Search("IsInstalled=0 and Type='Driver' and IsHidden=0")
$out = foreach ($u in $r.Updates) {
  $date = ''
  if ($u.DriverVerDate) { $date = ([datetime]$u.DriverVerDate).ToString('yyyy-MM-dd') }
  [pscustomobject]@{
    Id = [string]$u.Identity.UpdateID
    Title = [string]$u.Title
    Manufacturer = [string]$u.DriverManufacturer
    Model = [string]$u.DriverModel
    Class = [string]$u.DriverClass
    Date = $date
    Size = [int64]$u.MaxDownloadSize
    HardwareId = [string]$u.DriverHardwareID
  }
}
@($out) | ConvertTo-Json -Compress
"#;

/// Searches Windows Update for drivers newer than (or missing from) this PC's. Takes 10–60 seconds.
pub fn search() -> Result<Vec<DriverUpdate>, String> {
    let json = sys::powershell(SEARCH).map_err(|e| {
        if e.contains("0x8024402C") || e.contains("0x80072EE7") {
            "Can't reach Windows Update. Check your internet connection.".into()
        } else {
            e
        }
    })?;
    let t = json.trim();
    if t.is_empty() || t == "null" {
        return Ok(Vec::new());
    }
    serde_json::from_str(t).map_err(|e| format!("Unexpected answer from Windows Update: {e}"))
}

pub enum Progress {
    Line(String),
    Done(Result<String, String>),
}

/// Downloads and installs the chosen driver updates. Needs administrator rights.
pub fn install(ids: Vec<String>, tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let list = ids.iter().map(|i| format!("'{}'", i.replace('\'', ""))).collect::<Vec<_>>().join(",");
    let script = format!(
        r#"
$ids = @({list})
$s = New-Object -ComObject Microsoft.Update.Session
$s.ClientApplicationID = 'Wield'
$q = $s.CreateUpdateSearcher()
$q.ServerSelection = 2
Write-Output 'Looking up the selected drivers…'
$r = $q.Search("IsInstalled=0 and Type='Driver'")
$c = New-Object -ComObject Microsoft.Update.UpdateColl
foreach ($u in $r.Updates) {{ if ($ids -contains $u.Identity.UpdateID) {{ if (-not $u.EulaAccepted) {{ $u.AcceptEula() }}; [void]$c.Add($u) }} }}
if ($c.Count -eq 0) {{ throw 'The selected drivers are no longer offered (already installed?).' }}
Write-Output ("Downloading {{0}} driver(s)…" -f $c.Count)
$d = $s.CreateUpdateDownloader(); $d.Updates = $c; [void]$d.Download()
Write-Output 'Installing…'
$i = $s.CreateUpdateInstaller(); $i.Updates = $c
$res = $i.Install()
$ok = 0; $bad = @()
for ($n = 0; $n -lt $c.Count; $n++) {{
  $code = $res.GetUpdateResult($n).ResultCode
  if ($code -eq 2 -or $code -eq 3) {{ $ok++; Write-Output ('Installed: ' + $c.Item($n).Title) }} else {{ $bad += $c.Item($n).Title; Write-Output ('Failed: ' + $c.Item($n).Title) }}
}}
Write-Output ("RESULT|{{0}}|{{1}}|{{2}}" -f $ok, $bad.Count, $res.RebootRequired)
"#
    );
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let mut result = None;
    let args = vec!["-NoProfile".to_string(), "-NonInteractive".into(), "-Command".into(), format!("$ProgressPreference='SilentlyContinue'; {script}")];
    let mut errors = Vec::new();
    let r = sys::run_streaming("powershell.exe", &args, |l| {
        if let Some(rest) = l.strip_prefix("RESULT|") {
            result = Some(rest.to_string());
        } else if !l.is_empty() {
            if l.contains("Exception") || l.starts_with("At line") || l.contains("throw") || l.contains("0x8") {
                errors.push(l.to_string());
            }
            let _ = tx.send(Progress::Line(l.to_string()));
            ctx.request_repaint();
        }
    });
    let done = match (r, result) {
        (_, Some(res)) => {
            let p: Vec<&str> = res.split('|').collect();
            let ok: u32 = p.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            let bad: u32 = p.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let reboot = p.get(2).is_some_and(|s| s.eq_ignore_ascii_case("True"));
            let msg = format!("{ok} driver(s) installed{}.", if reboot { ". Restart Windows to finish" } else { "" });
            if bad == 0 { Ok(msg) } else { Err(format!("{msg} {bad} failed.")) }
        }
        (Ok(_), None) | (Err(_), None) => Err(errors.into_iter().map(|e| sys::first_error(&e)).next().unwrap_or_else(|| "Windows Update couldn't install the drivers.".into())),
    };
    send(Progress::Done(done));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to Windows Update; run with --ignored.
    #[test]
    #[ignore]
    fn live_search() {
        let v = search().unwrap();
        for u in &v {
            println!("{} | {} | {} | {}", u.title, u.manufacturer, u.class, u.date);
        }
    }

    #[test]
    fn reads_policy() {
        let _ = drivers_blocked_by_policy();
    }
}

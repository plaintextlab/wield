//! Every device and its driver, and devices that have a problem or no driver at all.

use serde::Deserialize;

use crate::tools::backup::sys;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Device {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Class")]
    pub class: String,
    #[serde(rename = "Manufacturer")]
    pub manufacturer: String,
    #[serde(rename = "Provider")]
    pub provider: String,
    #[serde(rename = "Version")]
    pub version: String,
    /// "yyyy-MM-dd"
    #[serde(rename = "Date")]
    pub date: String,
    #[serde(rename = "Inf")]
    pub inf: String,
    #[serde(rename = "HardwareId")]
    pub hardware_id: String,
    #[serde(rename = "DeviceId")]
    pub device_id: String,
    /// Device Manager problem code; 0 means working.
    #[serde(rename = "Problem")]
    pub problem: u32,
    #[serde(rename = "Signed")]
    pub signed: bool,
}

impl Device {
    pub fn has_driver(&self) -> bool {
        !self.version.is_empty()
    }
    pub fn year(&self) -> Option<u32> {
        self.date.get(..4).and_then(|y| y.parse().ok())
    }
    /// Search text for the web: the most specific hardware ID (VEN/DEV or VID/PID).
    pub fn search_id(&self) -> String {
        let h = if self.hardware_id.is_empty() { &self.device_id } else { &self.hardware_id };
        let parts: Vec<&str> = h.split('&').filter(|p| p.contains("VEN_") || p.contains("DEV_") || p.contains("VID_") || p.contains("PID_")).collect();
        if parts.is_empty() { h.split('\\').nth(1).unwrap_or(h).to_string() } else { parts.join("&").split('\\').last().unwrap_or("").to_string() }
    }
}

/// What a Device Manager problem code means, in plain words.
pub fn problem_text(code: u32) -> &'static str {
    match code {
        0 => "Working",
        1 => "Not configured correctly",
        3 => "The driver may be corrupted, or Windows is low on memory",
        10 => "Can't start",
        12 => "Not enough free resources (conflict with another device)",
        14 => "Needs a restart",
        18 => "Drivers need to be reinstalled",
        19 => "Configuration in the registry is damaged",
        21 => "Being removed",
        22 => "Disabled",
        24 => "Not present or not working",
        28 => "No driver installed",
        29 => "Disabled in the firmware (BIOS)",
        31 => "The driver couldn't be loaded",
        32 => "The driver's service is disabled",
        37 => "The driver failed to start",
        39 => "The driver is missing or corrupted",
        41 => "The driver loaded but the device wasn't found",
        43 => "Windows stopped it because it reported problems",
        45 => "Not connected",
        48 => "The driver is blocked because it's known to cause problems",
        52 => "The driver isn't digitally signed",
        _ => "Has a problem",
    }
}

const SCAN: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$drv = @{}
foreach ($d in Get-CimInstance Win32_PnPSignedDriver) { if ($d.DeviceID) { $drv[$d.DeviceID] = $d } }
$out = foreach ($e in Get-CimInstance Win32_PnPEntity) {
  $d = $drv[$e.DeviceID]
  $date = ''
  if ($d -and $d.DriverDate) { $date = ([datetime]$d.DriverDate).ToString('yyyy-MM-dd') }
  $hw = ''
  if ($e.HardwareID) { $hw = @($e.HardwareID)[0] }
  [pscustomobject]@{
    Name = [string]$(if ($e.Name) { $e.Name } elseif ($d) { $d.DeviceName } else { 'Unknown device' })
    Class = [string]$(if ($e.PNPClass) { $e.PNPClass } elseif ($d) { $d.DeviceClass } else { 'Other' })
    Manufacturer = [string]$e.Manufacturer
    Provider = [string]$d.DriverProviderName
    Version = [string]$d.DriverVersion
    Date = $date
    Inf = [string]$d.InfName
    HardwareId = [string]$hw
    DeviceId = [string]$e.DeviceID
    Problem = [int]$e.ConfigManagerErrorCode
    Signed = [bool]$d.IsSigned
  }
}
@($out) | ConvertTo-Json -Compress
"#;

pub fn scan() -> Result<Vec<Device>, String> {
    let json = sys::powershell(SCAN)?;
    let mut v: Vec<Device> = serde_json::from_str(json.trim()).map_err(|e| format!("Couldn't read the device list: {e}"))?;
    // Software-only and hidden plumbing devices make the list noisy.
    v.retain(|d| !d.device_id.starts_with("SWD\\PRINTENUM") && !d.device_id.starts_with("ROOT\\LEGACY_"));
    // Placeholder nodes (Bluetooth LE services and the like) have no type and need no driver; they only confuse.
    v.retain(|d| d.problem != 0 || d.has_driver() || (!d.class.trim().is_empty() && d.class != "Other"));
    v.sort_by(|a, b| (a.problem == 0, a.class.to_lowercase(), a.name.to_lowercase()).cmp(&(b.problem == 0, b.class.to_lowercase(), b.name.to_lowercase())));
    Ok(v)
}

/// Asks Windows to look for new hardware and install drivers it already has.
pub fn rescan() -> Result<String, String> {
    let (code, out) = sys::run("pnputil.exe", &["/scan-devices"])?;
    if code == 0 { Ok("Windows rescanned your hardware.".into()) } else { Err(sys::first_error(&out)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_devices() {
        let v = scan().unwrap();
        assert!(v.len() > 20, "{} devices", v.len());
        assert!(v.iter().any(|d| d.class.eq_ignore_ascii_case("Display") && d.has_driver()));
        assert!(v.iter().filter(|d| d.has_driver()).all(|d| !d.version.is_empty()));
    }

    #[test]
    fn hardware_ids() {
        let d = Device { hardware_id: r"PCI\VEN_10DE&DEV_24A0&SUBSYS_8A44103C&REV_A1".into(), ..Default::default() };
        assert_eq!(d.search_id(), "VEN_10DE&DEV_24A0");
        let u = Device { hardware_id: r"USB\VID_046D&PID_C52B&REV_2410".into(), ..Default::default() };
        assert_eq!(u.search_id(), "VID_046D&PID_C52B");
    }
}

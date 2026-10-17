//! Graphics drivers: what's installed, and for NVIDIA the newest driver from NVIDIA's own
//! driver service (the one the NVIDIA App uses), with a verified download.

use std::path::PathBuf;

use super::devices::Device;
use crate::tools::backup::sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    pub fn name(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other => "Other",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    /// PCI device ID, e.g. "24A0".
    pub dev_id: String,
    /// Windows' driver version, e.g. "32.0.16.1692".
    pub windows_version: String,
    pub date: String,
}

impl Gpu {
    /// The version vendors publish. NVIDIA: 32.0.16.1692 -> 616.92.
    pub fn vendor_version(&self) -> Option<String> {
        if self.vendor != Vendor::Nvidia {
            return None;
        }
        nvidia_version(&self.windows_version)
    }
}

pub fn nvidia_version(win: &str) -> Option<String> {
    let parts: Vec<&str> = win.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let digits = format!("{}{:0>4}", parts[2], parts[3]);
    if digits.len() < 5 {
        return None;
    }
    let tail = &digits[digits.len() - 5..];
    Some(format!("{}.{}", &tail[..3], &tail[3..]))
}

pub fn gpus(devices: &[Device]) -> Vec<Gpu> {
    devices
        .iter()
        .filter(|d| d.class.eq_ignore_ascii_case("Display") && d.device_id.starts_with("PCI\\"))
        .map(|d| {
            let id = d.device_id.to_uppercase();
            let field = |k: &str| id.split(['\\', '&']).find_map(|p| p.strip_prefix(k)).unwrap_or("").to_string();
            let vendor = match field("VEN_").as_str() {
                "10DE" => Vendor::Nvidia,
                "1002" | "1022" => Vendor::Amd,
                "8086" => Vendor::Intel,
                _ => Vendor::Other,
            };
            Gpu { name: d.name.clone(), vendor, dev_id: field("DEV_"), windows_version: d.version.clone(), date: d.date.clone() }
        })
        .collect()
}

/// Laptops get NVIDIA's notebook drivers.
pub fn is_laptop() -> bool {
    sys::powershell("(Get-CimInstance Win32_SystemEnclosure).ChassisTypes -join ','")
        .map(|s| s.split(',').filter_map(|t| t.trim().parse::<u32>().ok()).any(|t| [8, 9, 10, 11, 12, 14, 18, 21, 30, 31, 32].contains(&t)))
        .unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq)]
pub struct NvidiaDriver {
    pub name: String,
    pub version: String,
    pub date: String,
    pub download: String,
    pub details: String,
}

/// Newest NVIDIA driver for a GPU. `studio` asks for the Studio driver instead of Game Ready.
pub fn nvidia_latest(dev_id: &str, laptop: bool, studio: bool) -> Result<NvidiaDriver, String> {
    let build = std::env::var("WIELD_OS_BUILD").ok().unwrap_or_else(|| crate::win::sysver::os_info().build.split('.').next().unwrap_or("19045").to_string());
    let q = format!(
        r#"{{"dIDa":["{dev}_10DE"],"osC":"10.0","osB":"{build}","is6":"1","lg":"1033","iLp":"{lp}","prvMd":"0","gcV":"3.25.1.27","gIsB":"0","dch":"1","upCRD":"0","isCRD":"{crd}"}}"#,
        dev = dev_id.to_uppercase(),
        lp = if laptop { 1 } else { 0 },
        crd = if studio { 1 } else { 0 },
    );
    let path = format!(
        "/nvidia_web_services/controller.gfeclientcontent.NG.php/com.nvidia.services.GFEClientContent_NG.getDispDrvrByDevid/{}",
        crate::win::shell::url_encode(&q).replace('+', "%20")
    );
    let (status, body) = crate::win::http::get_url(&format!("https://gfwsl.geforce.com{path}"))?;
    if status != 200 {
        return Err(format!("NVIDIA's driver service answered HTTP {status}."));
    }
    parse_nvidia(&body)
}

pub fn parse_nvidia(body: &[u8]) -> Result<NvidiaDriver, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| "Unexpected answer from NVIDIA.".to_string())?;
    if v["criteria"]["IsSupported"]["state"].as_str() == Some("false") {
        return Err("NVIDIA doesn't list a driver for this GPU on this Windows version.".into());
    }
    let d = &v["DriverAttributes"];
    let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
    let version = s("Version");
    let download = s("DownloadURL");
    if version.is_empty() || !download.starts_with("https://") {
        return Err("NVIDIA didn't return a driver for this GPU.".into());
    }
    Ok(NvidiaDriver { name: s("Name"), version, date: s("ReleaseDateTime").get(..10).unwrap_or("").to_string(), download, details: s("DetailsURL") })
}

/// 617.42 > 616.92
pub fn newer(latest: &str, installed: &str) -> bool {
    let p = |s: &str| s.split('.').map(|x| x.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    p(latest) > p(installed)
}

/// Downloads NVIDIA's installer and checks it's really signed by NVIDIA before it can run.
pub fn download_nvidia(d: &NvidiaDriver) -> Result<PathBuf, String> {
    let host_ok = d.download.starts_with("https://") && d.download.split('/').nth(2).is_some_and(|h| h.ends_with(".nvidia.com"));
    if !host_ok {
        return Err("The download link doesn't point to nvidia.com, so Wield won't use it.".into());
    }
    let dir = std::env::temp_dir().join("wield-drivers");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(d.download.rsplit('/').next().unwrap_or("nvidia-driver.exe"));
    crate::win::http::download_to(&d.download, &file)?;
    let sig = crate::tools::malware::inspect::signature(&file);
    if !sig.is_trusted() || !sig.signer().is_some_and(|s| s.to_lowercase().contains("nvidia")) {
        let _ = std::fs::remove_file(&file);
        return Err("The downloaded file isn't signed by NVIDIA and was deleted.".into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_versions() {
        assert_eq!(nvidia_version("32.0.16.1692").as_deref(), Some("616.92"));
        assert_eq!(nvidia_version("31.0.15.4633").as_deref(), Some("546.33"));
        assert_eq!(nvidia_version("31.0.15.5222").as_deref(), Some("552.22"));
        assert!(newer("617.42", "616.92"));
        assert!(!newer("616.92", "616.92"));
        assert!(newer("620.01", "619.99"));
    }

    #[test]
    fn reads_nvidia_answer() {
        let body = br#"{"criteria":{"IsSupported":{"state":"true"}},"DriverAttributes":{"Name":"GeForce Game Ready Driver","Version":"617.42","ReleaseDateTime":"2026-10-06T00:00:00+0000","DownloadURL":"https://international-gfe.download.nvidia.com/Windows/617.42/x.exe","DetailsURL":"https://www.nvidia.com/en-us/drivers/details/280104/"}}"#;
        let d = parse_nvidia(body).unwrap();
        assert_eq!(d.version, "617.42");
        assert_eq!(d.date, "2026-10-06");
        assert!(parse_nvidia(br#"{"criteria":{"IsSupported":{"state":"false"}}}"#).is_err());
    }

    #[test]
    fn finds_gpus() {
        let devs = vec![Device { class: "Display".into(), name: "NVIDIA GeForce RTX 3070 Ti Laptop GPU".into(), device_id: r"PCI\VEN_10DE&DEV_24A0&SUBSYS_8A44103C&REV_A1\4&36".into(), version: "32.0.16.1692".into(), ..Default::default() }];
        let g = gpus(&devs);
        assert_eq!(g[0].vendor, Vendor::Nvidia);
        assert_eq!(g[0].dev_id, "24A0");
        assert_eq!(g[0].vendor_version().as_deref(), Some("616.92"));
    }

    /// Asks NVIDIA's service; run with --ignored.
    #[test]
    #[ignore]
    fn live_nvidia() {
        let d = nvidia_latest("24A0", true, false).unwrap();
        println!("{d:?}");
        assert!(d.download.contains("nvidia.com"));
    }
}

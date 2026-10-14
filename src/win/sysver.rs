use winreg::RegKey;
use winreg::enums::HKEY_LOCAL_MACHINE;

/// Human-friendly description of the running Windows installation.
#[derive(Clone, Debug, Default)]
pub struct OsInfo {
    /// e.g. "Windows 11 Pro"
    pub product: String,
    /// e.g. "23H2"
    pub release: String,
    /// e.g. "22631.4317"
    pub build: String,
}

impl OsInfo {
    pub fn summary(&self) -> String {
        let mut s = self.product.clone();
        if !self.release.is_empty() {
            s.push_str(&format!(" · {}", self.release));
        }
        if !self.build.is_empty() {
            s.push_str(&format!(" · build {}", self.build));
        }
        s
    }
}

pub fn os_info() -> OsInfo {
    let Ok(key) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") else {
        return OsInfo { product: "Windows".into(), ..Default::default() };
    };
    let get = |name: &str| key.get_value::<String, _>(name).unwrap_or_default();
    let build = get("CurrentBuildNumber");
    let ubr: u32 = key.get_value("UBR").unwrap_or(0);
    let build_num: u32 = build.parse().unwrap_or(0);

    // ProductName still says "Windows 10" on Windows 11, so fix it up from the build number.
    let mut product = get("ProductName");
    if build_num >= 22000 {
        product = product.replace("Windows 10", "Windows 11");
    }
    let mut release = get("DisplayVersion");
    if release.is_empty() {
        release = get("ReleaseId");
    }
    OsInfo {
        product,
        release,
        build: if ubr > 0 { format!("{build}.{ubr}") } else { build },
    }
}

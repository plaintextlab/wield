//! Self-updater backed by GitHub Releases.
//!
//! * Installed copies (set up with the installer) update by running the new installer silently.
//! * Portable copies replace their own .exe: a running program can be renamed on Windows, so the
//!   old file becomes `wield.exe.old` (deleted on next start) and the new one takes its name.
//!
//! Every download is checked against the SHA-256 published next to it before it's used.

use std::path::{Path, PathBuf};

use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

use crate::tools::malware::inspect::hash_file;
use crate::win::http;

pub const REPO: &str = "plaintextlab/wield";
/// Inno Setup AppId (see installer/wield.iss); the uninstall key is "{AppId}_is1".
pub const APP_ID: &str = "{8E5B9C2A-6F4D-4A8B-9E2C-57D1A3B0C4F6}";

pub fn current_version() -> (u32, u32, u32) {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>());
    Some((it.next()?.ok()?, it.next().unwrap_or(Ok(0)).ok()?, it.next().unwrap_or(Ok(0)).ok()?))
}

pub fn version_string(v: (u32, u32, u32)) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

#[derive(Clone, Debug, Default)]
pub struct Release {
    pub version: (u32, u32, u32),
    pub notes: String,
    pub published: String,
    pub page: String,
    pub setup_url: Option<String>,
    pub portable_url: Option<String>,
    pub checksums: Vec<(String, String)>,
}

impl Release {
    pub fn is_newer(&self) -> bool {
        self.version > current_version()
    }
}

fn parse_release(v: &serde_json::Value) -> Option<Release> {
    let tag = v["tag_name"].as_str()?.to_string();
    let mut r = Release {
        version: parse_version(&tag)?,
        notes: v["body"].as_str().unwrap_or("").trim().to_string(),
        published: v["published_at"].as_str().unwrap_or("").chars().take(10).collect(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        ..Default::default()
    };
    for a in v["assets"].as_array()? {
        let (Some(name), Some(url)) = (a["name"].as_str(), a["browser_download_url"].as_str()) else { continue };
        let lower = name.to_lowercase();
        if lower.ends_with(".sha256") {
            r.checksums.push((name.trim_end_matches(".sha256").to_string(), url.to_string()));
        } else if lower.ends_with("-setup.exe") {
            r.setup_url = Some(url.to_string());
        } else if lower.ends_with("-windows-x64.exe") {
            r.portable_url = Some(url.to_string());
        }
    }
    Some(r)
}

/// Asks GitHub for the newest published release.
pub fn fetch_latest() -> Result<Release, String> {
    let (status, body) = http::request("GET", "api.github.com", &format!("/repos/{REPO}/releases/latest"), None)?;
    match status {
        200 => {}
        403 | 429 => return Err("GitHub's rate limit was reached. Try again in an hour.".into()),
        404 => return Err("No releases published yet.".into()),
        s => return Err(format!("GitHub returned HTTP {s}.")),
    }
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    parse_release(&v).ok_or_else(|| "Unexpected response from GitHub.".into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Installed with the setup program into `dir`.
    Installed { dir: PathBuf },
    Portable { exe: PathBuf },
}

/// Installed when the installer's uninstall entry points at the folder we're running from.
pub fn mode() -> Mode {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let key = format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP_ID}_is1");
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Ok(k) = RegKey::predef(hive).open_subkey_with_flags(&key, KEY_READ) {
            let loc: String = k.get_value("InstallLocation").unwrap_or_default();
            let loc = PathBuf::from(loc.trim_end_matches('\\'));
            if !loc.as_os_str().is_empty() && same_path(&loc, &dir) {
                return Mode::Installed { dir };
            }
        }
    }
    Mode::Portable { exe }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

/// Downloads `url` to `dest` and checks it against the published checksum.
fn download_verified(r: &Release, url: &str, dest: &Path) -> Result<(), String> {
    let file_name = url.rsplit('/').next().unwrap_or_default().to_string();
    let sum_url = r.checksums.iter().find(|(n, _)| n.eq_ignore_ascii_case(&file_name)).map(|(_, u)| u.clone()).ok_or("The release has no checksum for this file, so it can't be verified.")?;
    let sum_file = dest.with_extension("sha256");
    http::download_to(&sum_url, &sum_file)?;
    let sum = std::fs::read_to_string(&sum_file).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&sum_file);
    let expected = sum.split_whitespace().next().unwrap_or("").to_lowercase();
    if expected.len() != 64 {
        return Err("The published checksum is malformed.".into());
    }
    http::download_to(url, dest)?;
    let actual = hash_file(dest).map_err(|e| e.to_string())?.sha256;
    if actual != expected {
        let _ = std::fs::remove_file(dest);
        return Err("The download doesn't match its published checksum and was deleted.".into());
    }
    Ok(())
}

/// A downloaded, verified update ready to be applied.
#[derive(Clone, Debug)]
pub struct Staged {
    pub version: (u32, u32, u32),
    pub file: PathBuf,
    pub mode: Mode,
}

fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("wield-update")
}

/// Downloads and verifies the right file for how Wield is installed.
pub fn stage(r: &Release) -> Result<Staged, String> {
    let mode = mode();
    let url = match &mode {
        Mode::Installed { .. } => r.setup_url.clone().or(r.portable_url.clone()),
        Mode::Portable { .. } => r.portable_url.clone(),
    }
    .ok_or("This release has no download for your copy of Wield.")?;
    let name = url.rsplit('/').next().unwrap_or("wield-update.exe");
    let file = staging_dir().join(name);
    download_verified(r, &url, &file)?;
    // An installed copy that only got a portable download falls back to replacing the exe.
    let mode = if matches!(mode, Mode::Installed { .. }) && !name.to_lowercase().ends_with("-setup.exe") {
        Mode::Portable { exe: std::env::current_exe().map_err(|e| e.to_string())? }
    } else {
        mode
    };
    Ok(Staged { version: r.version, file, mode })
}

/// Applies a staged update. With `relaunch`, the new version starts afterwards.
/// The caller must exit right after this returns Ok.
pub fn apply(s: &Staged, relaunch: bool) -> Result<(), String> {
    match &s.mode {
        Mode::Installed { .. } => {
            let mut cmd = std::process::Command::new(&s.file);
            cmd.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"]);
            if !relaunch {
                cmd.arg("/NOLAUNCH");
            }
            cmd.spawn().map(|_| ()).map_err(|e| format!("Could not start the installer: {e}"))
        }
        Mode::Portable { exe } => {
            let old = exe.with_extension("exe.old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(exe, &old).map_err(|e| format!("Can't replace {} ({e}). Download the new version from the releases page instead.", exe.display()))?;
            if let Err(e) = std::fs::copy(&s.file, exe) {
                // Put the original back so Wield still starts.
                let _ = std::fs::rename(&old, exe);
                return Err(format!("Could not write the new version: {e}"));
            }
            if relaunch {
                std::process::Command::new(exe).spawn().map_err(|e| e.to_string())?;
            }
            Ok(())
        }
    }
}

/// Removes leftovers from a previous portable update.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
    let _ = std::fs::remove_dir_all(staging_dir());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.5.0"), Some((0, 5, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("v2.0.1-beta"), Some((2, 0, 1)));
        assert!(parse_version("latest").is_none());
        assert!((0, 10, 0) > (0, 9, 9));
    }

    #[test]
    fn parses_release_assets() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"tag_name":"v0.6.0","body":"notes","published_at":"2026-10-20T10:00:00Z","html_url":"https://x",
                "assets":[{"name":"wield-v0.6.0-windows-x64.exe","browser_download_url":"https://a/p.exe"},
                          {"name":"wield-v0.6.0-windows-x64.exe.sha256","browser_download_url":"https://a/p.sha"},
                          {"name":"wield-v0.6.0-setup.exe","browser_download_url":"https://a/s.exe"}]}"#,
        )
        .unwrap();
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, (0, 6, 0));
        assert_eq!(r.portable_url.as_deref(), Some("https://a/p.exe"));
        assert_eq!(r.setup_url.as_deref(), Some("https://a/s.exe"));
        assert_eq!(r.checksums.len(), 1);
        assert_eq!(r.published, "2026-10-20");
    }

    #[test]
    fn portable_swap_replaces_the_exe() {
        let dir = std::env::temp_dir().join("wield-swap-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("app.exe");
        let new = dir.join("new.exe");
        std::fs::write(&exe, b"old version").unwrap();
        std::fs::write(&new, b"new version").unwrap();
        let staged = Staged { version: (9, 9, 9), file: new, mode: Mode::Portable { exe: exe.clone() } };
        apply(&staged, false).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new version");
        assert_eq!(std::fs::read(exe.with_extension("exe.old")).unwrap(), b"old version");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Online: `cargo test online_latest -- --ignored`.
    #[test]
    #[ignore]
    fn online_latest_release() {
        let r = fetch_latest().unwrap();
        assert!(r.version >= (0, 5, 0));
        assert!(r.portable_url.is_some());
        assert!(!r.checksums.is_empty());
    }

    /// Online: downloads the published portable exe and verifies its checksum (into temp only).
    #[test]
    #[ignore]
    fn online_download_verifies() {
        let r = fetch_latest().unwrap();
        let url = r.portable_url.clone().unwrap();
        let dest = std::env::temp_dir().join("wield-verify-test.exe");
        download_verified(&r, &url, &dest).unwrap();
        assert!(dest.metadata().unwrap().len() > 1_000_000);
        let _ = std::fs::remove_file(dest);
    }
}

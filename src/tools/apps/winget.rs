//! Installing apps with the Windows Package Manager (winget), and installing winget itself on
//! PCs that don't have it (LTSC and older Windows 10 builds).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use super::catalog::{App, Source};
use crate::tools::backup::sys;
use crate::win::fileinfo::expand_env;

/// Where winget is, if installed.
pub fn winget_path() -> Option<PathBuf> {
    let alias = PathBuf::from(expand_env(r"%LOCALAPPDATA%\Microsoft\WindowsApps\winget.exe"));
    if alias.exists() {
        return Some(alias);
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join("winget.exe")).find(|p| p.is_file()))
}

/// Installed winget package IDs (lowercase), from `winget export`, plus program names from the registry.
pub struct Installed {
    pub ids: HashSet<String>,
    pub names: Vec<String>,
}

impl Installed {
    pub fn has(&self, app: &App) -> bool {
        let by_id = matches!(app.source, Source::Winget(ids) if self.ids.contains(&ids[0].to_lowercase()));
        by_id || app.detect.iter().any(|d| self.names.iter().any(|n| n.to_lowercase().starts_with(&d.to_lowercase())))
    }
}

pub fn installed() -> Installed {
    let names = crate::tools::backup::kit::installed_programs().into_iter().map(|p| p.0).collect();
    let mut ids = HashSet::new();
    if let Some(wg) = winget_path() {
        let out = std::env::temp_dir().join(format!("wield-winget-export-{}.json", std::process::id()));
        let args = ["export", "-o", &out.display().to_string(), "--source", "winget", "--accept-source-agreements", "--disable-interactivity"];
        let _ = sys::run(&wg.display().to_string(), &args);
        if let Some(v) = std::fs::read_to_string(&out).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            for src in v["Sources"].as_array().into_iter().flatten() {
                for p in src["Packages"].as_array().into_iter().flatten() {
                    if let Some(id) = p["PackageIdentifier"].as_str() {
                        ids.insert(id.to_lowercase());
                    }
                }
            }
        }
        let _ = std::fs::remove_file(out);
    }
    Installed { ids, names }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Installed,
    AlreadyInstalled,
    RestartNeeded,
    OpenedWebsite,
    Failed(String),
    Skipped,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        !matches!(self, Outcome::Failed(_))
    }
}

pub enum Progress {
    /// Starting app number i.
    Start(usize),
    Line(String),
    Percent(f32),
    Finished(usize, Outcome),
    Done(Option<PathBuf>),
}

/// Reads a winget result from its exit code and output.
pub fn outcome(code: i32, out: &str) -> Outcome {
    let l = out.to_lowercase();
    let code = code as u32;
    if l.contains("restart") && (l.contains("successfully installed") || code == 0x8A15_0109 || code == 3010) {
        return Outcome::RestartNeeded;
    }
    if code == 0 || l.contains("successfully installed") {
        return Outcome::Installed;
    }
    if code == 0x8A15_002B || code == 0x8A15_0061 || l.contains("no available upgrade found") || l.contains("no newer package versions") || l.contains("already installed") {
        return Outcome::AlreadyInstalled;
    }
    let reason = if code == 0x8A15_0101 || l.contains("in use") {
        "The app is running. Close it and try again.".to_string()
    } else if l.contains("cancelled") || l.contains("canceled") || code == 0x8007_04C7 {
        "The installation was cancelled (for example the administrator prompt was declined).".to_string()
    } else if code == 0x8A15_0014 {
        "There's no installer for this PC's Windows version or processor.".into()
    } else {
        out.lines()
            .map(str::trim)
            .rfind(|s| !s.is_empty() && !s.chars().all(|c| "-\\|/ █▒░".contains(c)) && !s.ends_with('%'))
            .map(str::to_string)
            .unwrap_or_else(|| format!("winget exited with code 0x{code:08X}."))
    };
    Outcome::Failed(reason)
}

/// "  ██████▒▒▒▒  45.2 MB / 100 MB" or "  ████  60%" -> fraction
pub fn parse_percent(line: &str) -> Option<f32> {
    let t = line.trim();
    if let Some(p) = t.strip_suffix('%') {
        return p.rsplit(' ').next()?.trim().parse::<f32>().ok().map(|v| v / 100.0);
    }
    let (a, b) = t.rsplit_once(" / ")?;
    let num = |s: &str| -> Option<f64> {
        let mut it = s.split_whitespace();
        let v: f64 = it.next()?.parse().ok()?;
        let m = match it.next()? {
            "KB" => 1e3,
            "MB" => 1e6,
            "GB" => 1e9,
            "B" => 1.0,
            _ => return None,
        };
        Some(v * m)
    };
    let done = num(a.rsplit("  ").next()?.trim())?;
    let total = num(b.trim())?;
    (total > 0.0).then(|| (done / total) as f32)
}

fn log_dir() -> Option<PathBuf> {
    Some(crate::settings::data_dir()?.join("app-installs"))
}

fn save_log(lines: &[String]) -> Option<PathBuf> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let p = dir.join(format!("{}-install.log", sys::now_unix()));
    std::fs::write(&p, lines.join("\r\n")).ok()?;
    Some(p)
}

/// Installs apps one after another. `stop` ends the queue after the current app.
pub fn install(apps: Vec<&'static App>, tx: Sender<Progress>, stop: Arc<AtomicBool>, ctx: eframe::egui::Context) {
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let wg = winget_path();
    let mut log = Vec::new();
    for (i, app) in apps.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            send(Progress::Finished(i, Outcome::Skipped));
            continue;
        }
        send(Progress::Start(i));
        log.push(format!("==== {} ====", app.name));
        let result = match app.source {
            Source::Website(url) => {
                crate::win::shell::open(url);
                Outcome::OpenedWebsite
            }
            Source::Winget(ids) => match &wg {
                None => Outcome::Failed("winget isn't installed.".into()),
                Some(wg) => {
                    let mut overall = Outcome::AlreadyInstalled;
                    for id in ids {
                        let args: Vec<String> = [
                            "install",
                            "--id",
                            id,
                            "--exact",
                            "--source",
                            "winget",
                            "--silent",
                            "--accept-package-agreements",
                            "--accept-source-agreements",
                            "--disable-interactivity",
                        ]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                        let mut out = String::new();
                        let code = sys::run_streaming(&wg.display().to_string(), &args, |l| {
                            if l.is_empty() {
                                return;
                            }
                            if let Some(p) = parse_percent(l) {
                                let _ = tx.send(Progress::Percent(p));
                            } else {
                                out.push_str(l);
                                out.push('\n');
                                log.push(l.to_string());
                                let _ = tx.send(Progress::Line(l.to_string()));
                            }
                            ctx.request_repaint();
                        });
                        let o = match code {
                            Ok(c) => outcome(c, &out),
                            Err(e) => Outcome::Failed(e),
                        };
                        // The worst result of a multi-package app wins.
                        overall = match (&overall, &o) {
                            (Outcome::Failed(_), _) => overall,
                            (_, Outcome::Failed(_)) => o,
                            (_, Outcome::RestartNeeded) => o,
                            (Outcome::AlreadyInstalled, Outcome::Installed) => o,
                            _ => overall,
                        };
                    }
                    overall
                }
            },
        };
        log.push(format!("Result: {result:?}"));
        send(Progress::Finished(i, result));
    }
    send(Progress::Done(save_log(&log)));
}

/// Updates every app winget can update.
pub fn upgrade_all(tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let Some(wg) = winget_path() else {
        send(Progress::Finished(0, Outcome::Failed("winget isn't installed.".into())));
        send(Progress::Done(None));
        return;
    };
    send(Progress::Start(0));
    let args: Vec<String> = ["upgrade", "--all", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"].iter().map(|s| s.to_string()).collect();
    let mut log = Vec::new();
    let res = sys::run_streaming(&wg.display().to_string(), &args, |l| {
        if l.is_empty() {
            return;
        }
        if let Some(p) = parse_percent(l) {
            let _ = tx.send(Progress::Percent(p));
        } else {
            log.push(l.to_string());
            let _ = tx.send(Progress::Line(l.to_string()));
        }
        ctx.request_repaint();
    });
    let text = log.join("\n");
    let o = match res {
        Ok(c) if c == 0 || text.contains("Successfully installed") => Outcome::Installed,
        Ok(c) => {
            if text.to_lowercase().contains("no installed package found matching") || text.to_lowercase().contains("no available upgrade") {
                Outcome::AlreadyInstalled
            } else {
                outcome(c, &text)
            }
        }
        Err(e) => Outcome::Failed(e),
    };
    send(Progress::Finished(0, o));
    send(Progress::Done(save_log(&log)));
}

// ------------------------------------------------------------------ installing winget

const RELEASE_PATH: &str = "/repos/microsoft/winget-cli/releases/latest";

/// Downloads `url` and checks it against the SHA-256 GitHub publishes for the asset.
fn download_checked(url: &str, digest: Option<&str>, dest: &Path) -> Result<(), String> {
    crate::win::http::download_to(url, dest)?;
    if let Some(d) = digest.and_then(|d| d.strip_prefix("sha256:")) {
        let actual = crate::tools::malware::inspect::hash_file(dest).map_err(|e| e.to_string())?.sha256;
        if !actual.eq_ignore_ascii_case(d) {
            let _ = std::fs::remove_file(dest);
            return Err(format!("{} doesn't match its published checksum and was deleted.", dest.file_name().unwrap_or_default().to_string_lossy()));
        }
    }
    Ok(())
}

/// Installs the Windows Package Manager from Microsoft's official GitHub release, for this user.
pub fn install_winget(tx: Sender<Progress>, ctx: eframe::egui::Context) {
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let line = |s: &str| send(Progress::Line(s.to_string()));
    send(Progress::Start(0));
    let r = (|| -> Result<String, String> {
        line("Finding the latest Windows Package Manager release on GitHub…");
        let (status, body) = crate::win::http::request("GET", "api.github.com", RELEASE_PATH, None)?;
        match status {
            200 => {}
            403 | 429 => return Err("GitHub's rate limit was reached. Try again in an hour.".into()),
            s => return Err(format!("GitHub returned HTTP {s}. Try again later.")),
        }
        let v: serde_json::Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let tag = v["tag_name"].as_str().unwrap_or("?").to_string();
        let asset = |pred: &dyn Fn(&str) -> bool| -> Option<(String, Option<String>)> {
            v["assets"].as_array()?.iter().find(|a| a["name"].as_str().is_some_and(pred)).map(|a| {
                (a["browser_download_url"].as_str().unwrap_or_default().to_string(), a["digest"].as_str().map(str::to_string))
            })
        };
        let bundle = asset(&|n| n.ends_with(".msixbundle")).ok_or("The release has no App Installer package.")?;
        let deps = asset(&|n| n == "DesktopAppInstaller_Dependencies.zip").ok_or("The release has no dependencies package.")?;
        let dir = std::env::temp_dir().join("wield-winget");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        line(&format!("Downloading winget {tag} dependencies (about 100 MB)…"));
        send(Progress::Percent(0.1));
        let deps_zip = dir.join("deps.zip");
        download_checked(&deps.0, deps.1.as_deref(), &deps_zip)?;
        line("Downloading App Installer (about 200 MB)…");
        send(Progress::Percent(0.4));
        let bundle_file = dir.join("Microsoft.DesktopAppInstaller.msixbundle");
        download_checked(&bundle.0, bundle.1.as_deref(), &bundle_file)?;
        line("Installing…");
        send(Progress::Percent(0.8));
        let deps_dir = dir.join("deps");
        let script = format!(
            "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force; \
             $d = @(Get-ChildItem -LiteralPath '{}' -Recurse -Include *.appx,*.msix | Where-Object {{ $_.FullName -match '\\\\x64\\\\' }} | ForEach-Object FullName); \
             Add-AppxPackage -Path '{}' -DependencyPath $d -ForceApplicationShutdown -ErrorAction Stop",
            deps_zip.display(),
            deps_dir.display(),
            deps_dir.display(),
            bundle_file.display()
        );
        sys::powershell(&script)?;
        let _ = std::fs::remove_dir_all(&dir);
        // The command alias can take a moment to appear.
        for _ in 0..20 {
            if winget_path().is_some() {
                return Ok(format!("Windows Package Manager {tag} installed."));
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Err("App Installer was installed, but the winget command didn't appear. Sign out and back in, then try again.".into())
    })();
    match r {
        Ok(m) => {
            line(&m);
            send(Progress::Finished(0, Outcome::Installed));
        }
        Err(e) => send(Progress::Finished(0, Outcome::Failed(e))),
    }
    send(Progress::Done(None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_outcomes() {
        assert_eq!(outcome(0, "Found VLC [VideoLAN.VLC]\nSuccessfully installed"), Outcome::Installed);
        assert_eq!(outcome(0x8A15_002Bu32 as i32, "Found an existing package already installed.\nNo available upgrade found."), Outcome::AlreadyInstalled);
        assert_eq!(outcome(0x8A15_0109u32 as i32, "Successfully installed. Restart the application to complete the upgrade."), Outcome::RestartNeeded);
        assert!(matches!(outcome(1, "Installer failed with exit code: 1603\n"), Outcome::Failed(m) if m.contains("1603")));
        assert!(matches!(outcome(0x8A15_0101u32 as i32, ""), Outcome::Failed(m) if m.contains("running")));
    }

    #[test]
    fn reads_progress() {
        assert_eq!(parse_percent("  ██████████▒▒▒▒▒▒▒▒▒▒  50%"), Some(0.5));
        let p = parse_percent("  ████████▒▒▒▒  25.0 MB / 100 MB").unwrap();
        assert!((p - 0.25).abs() < 0.001);
        assert_eq!(parse_percent("Downloading https://example.com/x.exe"), None);
    }

    #[test]
    fn detects_installed_programs_without_winget() {
        // Every Windows PC has some Microsoft Visual C++ runtime, at least through the registry.
        let inst = installed();
        assert!(!inst.names.is_empty());
    }
}

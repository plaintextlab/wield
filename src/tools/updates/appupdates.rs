//! App updates: every installed program, which ones to leave alone, and newer versions from
//! the winget catalog, installed or downloaded on request.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};

use crate::tools::apps::winget::{Outcome, Progress, outcome, parse_percent, winget_path};
use crate::tools::backup::sys;
use crate::win::fileinfo::expand_env;

#[derive(Clone, Debug)]
pub struct InstalledApp {
    pub name: String,
    pub version: String,
    pub publisher: String,
}

/// Every installed program (Windows' uninstall list), alphabetically.
pub fn installed() -> Vec<InstalledApp> {
    crate::tools::backup::kit::installed_programs().into_iter().map(|(name, version, publisher)| InstalledApp { name, version, publisher }).collect()
}

// ------------------------------------------------------------------ exclusions

/// An app's name without version numbers, so "7-Zip 26.02 (x64)" still matches after it updates
/// to "7-Zip 26.05 (x64)".
pub fn norm(name: &str) -> String {
    name.split_whitespace()
        .filter(|t| {
            let t = t.trim_matches(|c: char| c == '(' || c == ')' || c == ',');
            let t = t.strip_prefix('v').or_else(|| t.strip_prefix('V')).unwrap_or(t);
            // Version-like: digits with dots (26.02, 10.0.12, v8.7). Bare numbers such as years stay.
            let versionish = t.contains('.') && t.chars().next().is_some_and(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-');
            !versionish
        })
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[derive(Default, Serialize, Deserialize)]
struct Prefs {
    /// Apps never checked for updates (names without versions, see `norm`).
    excluded: BTreeSet<String>,
}

fn prefs_path() -> Option<PathBuf> {
    Some(crate::settings::data_dir()?.join("updates.json"))
}

fn load() -> Prefs {
    prefs_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn excluded() -> BTreeSet<String> {
    load().excluded
}

pub fn set_excluded(names: &[String], exclude: bool) -> Result<(), String> {
    let mut p = load();
    for n in names {
        if exclude {
            p.excluded.insert(norm(n));
        } else {
            p.excluded.remove(&norm(n));
        }
    }
    let path = prefs_path().ok_or("No data folder")?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(path, serde_json::to_string_pretty(&p).unwrap_or_default()).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ scanning

#[derive(Clone, Debug, PartialEq)]
pub struct Upgrade {
    pub name: String,
    pub id: String,
    pub version: String,
    pub available: String,
}

/// Reads winget's upgrade table. Columns are found from the header line, so long names are fine.
pub fn parse_upgrade(text: &str) -> Vec<Upgrade> {
    parse_table(text, true)
}

/// Reads a winget table ("upgrade" or "list"). With `need_available`, rows without a newer version are dropped.
pub fn parse_table(text: &str, need_available: bool) -> Vec<Upgrade> {
    let lines: Vec<Vec<char>> = text.lines().map(|l| l.trim_end().chars().collect()).collect();
    let Some(hi) = lines.iter().position(|l| {
        let s: String = l.iter().collect();
        s.starts_with("Name") && s.contains(" Id ") && s.contains("Version")
    }) else {
        return Vec::new();
    };
    let header: String = lines[hi].iter().collect();
    let col = |name: &str| header.find(name).map(|byte| header[..byte].chars().count());
    let (Some(id), Some(ver)) = (col(" Id ").map(|c| c + 1), col("Version")) else { return Vec::new() };
    let avail_col = col("Available");
    let source = col("Source");
    let avail = avail_col.or(source).unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for l in &lines[hi + 1..] {
        if l.iter().all(|c| *c == '-') {
            continue;
        }
        // Summary lines like "12 upgrades available." are shorter than the table.
        if l.len() <= ver {
            break;
        }
        let cut = |a: usize, b: Option<usize>| -> String {
            let end = b.unwrap_or(l.len()).min(l.len());
            if a >= end { String::new() } else { l[a..end].iter().collect::<String>().trim().to_string() }
        };
        let available = match avail_col {
            Some(a) => cut(a, source),
            None => String::new(),
        };
        let u = Upgrade { name: cut(0, Some(id)), id: cut(id, Some(ver)), version: cut(ver, Some(avail.min(source.unwrap_or(usize::MAX)))), available };
        if !u.id.is_empty() && !u.id.contains(' ') && (!need_available || !u.available.is_empty()) {
            out.push(u);
        }
    }
    out
}

/// Asks winget which installed apps have newer versions. Takes 5–30 seconds.
pub fn scan() -> Result<Vec<Upgrade>, String> {
    let wg = winget_path().ok_or("winget isn't installed.")?;
    let (_, out) = sys::run(&wg.display().to_string(), &["upgrade", "--include-unknown", "--source", "winget", "--accept-source-agreements", "--disable-interactivity"])?;
    if out.contains("No installed package found matching") || out.contains("No available upgrade") {
        return Ok(Vec::new());
    }
    let v = parse_upgrade(&out);
    if v.is_empty() && !out.contains("upgrades available") && out.to_lowercase().contains("error") {
        return Err(sys::first_error(&out));
    }
    Ok(v)
}

/// Installed apps winget recognizes (normalized names), so the rest can be reported as "can't check".
pub fn known() -> Result<BTreeSet<String>, String> {
    let wg = winget_path().ok_or("winget isn't installed.")?;
    let (_, out) = sys::run(&wg.display().to_string(), &["list", "--source", "winget", "--accept-source-agreements", "--disable-interactivity"])?;
    Ok(parse_table(&out, false).into_iter().map(|u| norm(&u.name)).collect())
}

pub fn download_dir() -> PathBuf {
    PathBuf::from(expand_env(r"%USERPROFILE%\Downloads\Wield updates"))
}

/// Updates (or only downloads) apps one after another. `stop` ends the queue after the current one.
pub fn update(list: Vec<Upgrade>, download_only: bool, tx: Sender<Progress>, stop: Arc<AtomicBool>, ctx: eframe::egui::Context) {
    let send = |p: Progress| {
        let _ = tx.send(p);
        ctx.request_repaint();
    };
    let Some(wg) = winget_path() else {
        for i in 0..list.len() {
            send(Progress::Finished(i, Outcome::Failed("winget isn't installed.".into())));
        }
        send(Progress::Done(None));
        return;
    };
    let dir = download_dir();
    let mut log = Vec::new();
    for (i, u) in list.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            send(Progress::Finished(i, Outcome::Skipped));
            continue;
        }
        send(Progress::Start(i));
        log.push(format!("==== {} ({} → {}) ====", u.name, u.version, u.available));
        let mut args: Vec<String> = if download_only {
            vec!["download".into(), "--id".into(), u.id.clone(), "--exact".into(), "--source".into(), "winget".into(), "-d".into(), dir.display().to_string()]
        } else {
            vec!["upgrade".into(), "--id".into(), u.id.clone(), "--exact".into(), "--source".into(), "winget".into(), "--silent".into(), "--include-unknown".into()]
        };
        args.extend(["--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"].map(String::from));
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
            Ok(c) if download_only && (c == 0 || out.contains("Installer downloaded")) => Outcome::Installed,
            Ok(c) => outcome(c, &out),
            Err(e) => Outcome::Failed(e),
        };
        log.push(format!("Result: {o:?}"));
        send(Progress::Finished(i, o));
    }
    let log_file = crate::settings::data_dir().map(|d| d.join("app-installs")).and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        let p = d.join(format!("{}-{}.log", sys::now_unix(), if download_only { "download" } else { "update" }));
        std::fs::write(&p, log.join("\r\n")).ok()?;
        Some(p)
    });
    send(Progress::Done(log_file));
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "   - \r\n\
Name                                            Id                                 Version   Available Source\n\
-------------------------------------------------------------------------------------------------------------\n\
7-Zip 26.02 (x64)                               7zip.7zip                          26.02     26.05     winget\n\
Microsoft Windows Desktop Runtime 10.0.12 (x64) Microsoft.DotNet.DesktopRuntime.10 10.0.12   10.0.13   winget\n\
Syncthing (Current user)                        BillStewart.SyncthingWindowsSetup  Unknown   2.0.2     winget\n\
12 upgrades available.\n";

    #[test]
    fn parses_winget_table() {
        let v = parse_upgrade(SAMPLE);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0], Upgrade { name: "7-Zip 26.02 (x64)".into(), id: "7zip.7zip".into(), version: "26.02".into(), available: "26.05".into() });
        assert_eq!(v[1].id, "Microsoft.DotNet.DesktopRuntime.10");
        assert_eq!(v[2].version, "Unknown");
        assert!(parse_upgrade("No installed package found matching input criteria.").is_empty());
    }

    #[test]
    fn names_without_versions() {
        assert_eq!(norm("7-Zip 26.02 (x64)"), norm("7-Zip 26.05 (x64)"));
        assert_eq!(norm("Microsoft Windows Desktop Runtime 10.0.12 (x64)"), "microsoft windows desktop runtime (x64)");
        assert_eq!(norm("Ollama version 0.40.2"), "ollama version");
        assert_eq!(norm("Notepad++ v8.7"), "notepad++");
        assert_eq!(norm("7-Zip"), "7-zip");
        assert_ne!(norm("Microsoft Visual C++ 2005 Redistributable"), norm("Microsoft Visual C++ 2008 Redistributable"));
    }

    #[test]
    fn parses_list_table_without_available_column() {
        let t = "Name      Id        Version Source
----------------------------------
Foo App   Foo.App   1.2.3   winget
";
        let v = parse_table(t, false);
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].name.as_str(), v[0].id.as_str(), v[0].version.as_str()), ("Foo App", "Foo.App", "1.2.3"));
        assert!(parse_table(t, true).is_empty());
    }

    #[test]
    fn exclusions_roundtrip() {
        let name = "Wield test app 123".to_string();
        set_excluded(std::slice::from_ref(&name), true).unwrap();
        assert!(excluded().contains(&norm(&name)));
        set_excluded(std::slice::from_ref(&name), false).unwrap();
        assert!(!excluded().contains(&norm(&name)));
    }

    #[test]
    fn lists_installed_apps() {
        assert!(!installed().is_empty());
    }

    /// Asks winget for real; run with --ignored --nocapture.
    #[test]
    #[ignore]
    fn live_scan() {
        for u in scan().unwrap() {
            println!("{} | {} | {} -> {}", u.name, u.id, u.version, u.available);
        }
    }
}

//! Shortcuts that aren't part of Windows: shortcut keys on Start menu and desktop shortcuts,
//! PowerToys, well-known apps' global hotkeys, and any other hotkey a running program holds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ, STGM_READWRITE};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{Interface, PCWSTR};

use super::keys::Combo;
use crate::win::fileinfo::expand_env;
use crate::win::wide;

// ------------------------------------------------------------------ .lnk shortcut keys

#[derive(Clone, Debug)]
pub struct LinkKey {
    pub path: PathBuf,
    pub name: String,
    pub target: String,
    /// None when Wield turned it off (the original is remembered).
    pub combo: Option<Combo>,
    pub disabled: Option<Combo>,
    pub all_users: bool,
}

/// Folders where Windows honors shortcut keys (Start menu and desktop), plus pinned taskbar items.
pub fn link_dirs() -> Vec<(PathBuf, bool)> {
    [
        (r"%APPDATA%\Microsoft\Windows\Start Menu", false),
        (r"%ProgramData%\Microsoft\Windows\Start Menu", true),
        (r"%USERPROFILE%\Desktop", false),
        (r"%PUBLIC%\Desktop", true),
        (r"%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned", false),
    ]
    .into_iter()
    .map(|(p, all)| (PathBuf::from(expand_env(p)), all))
    .filter(|(p, _)| p.is_dir())
    .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 6 {
            walk(&p, out, depth + 1);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            out.push(p);
        }
    }
}

fn open_link(path: &Path, write: bool) -> Option<(IShellLinkW, IPersistFile)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let w = wide(&path.to_string_lossy());
        file.Load(PCWSTR(w.as_ptr()), if write { STGM_READWRITE } else { STGM_READ }).ok()?;
        Some((link, file))
    }
}

fn read_link(path: &Path) -> Option<(u16, String)> {
    let (link, _) = open_link(path, false)?;
    unsafe {
        let hk = link.GetHotkey().ok()?;
        let mut buf = [0u16; 1024];
        let _ = link.GetPath(&mut buf, std::ptr::null_mut(), 0);
        let target = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
        Some((hk, target))
    }
}

/// Every shortcut with a shortcut key, plus the ones Wield turned off. Needs COM (STA).
pub fn link_keys() -> Vec<LinkKey> {
    let disabled = load_state().links;
    let mut out = Vec::new();
    for (dir, all_users) in link_dirs() {
        let mut files = Vec::new();
        walk(&dir, &mut files, 0);
        for f in files {
            let Some((hk, target)) = read_link(&f) else { continue };
            let combo = Combo::from_lnk(hk);
            let off = disabled.get(&f.to_string_lossy().to_lowercase()).and_then(|h| Combo::from_lnk(*h));
            if combo.is_none() && off.is_none() {
                continue;
            }
            out.push(LinkKey { name: f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), path: f, target, combo, disabled: off, all_users });
        }
    }
    out.sort_by_key(|l| l.name.to_lowercase());
    out
}

/// Sets (or clears, with None) a shortcut's key. Windows applies it right away.
pub fn set_link_key(path: &Path, combo: Option<Combo>) -> Result<(), String> {
    let (link, file) = open_link(path, true).ok_or("Couldn't open the shortcut. It may need administrator rights (all-users shortcut).")?;
    unsafe {
        link.SetHotkey(combo.map(|c| c.to_lnk()).unwrap_or(0)).map_err(|e| e.message())?;
        file.Save(PCWSTR::null(), true).map_err(|e| if e.code().0 as u32 == 0x8007_0005 { "Needs administrator rights (this shortcut is for all users).".to_string() } else { e.message() })?;
    }
    Ok(())
}

/// Turns a shortcut key off but remembers it, so it can be turned back on.
pub fn disable_link(l: &LinkKey) -> Result<(), String> {
    let Some(c) = l.combo else { return Ok(()) };
    set_link_key(&l.path, None)?;
    let mut st = load_state();
    st.links.insert(l.path.to_string_lossy().to_lowercase(), c.to_lnk());
    save_state(&st)
}

pub fn enable_link(l: &LinkKey) -> Result<(), String> {
    let Some(c) = l.disabled else { return Ok(()) };
    set_link_key(&l.path, Some(c))?;
    let mut st = load_state();
    st.links.remove(&l.path.to_string_lossy().to_lowercase());
    save_state(&st)
}

/// Gives a program a shortcut key: a Start menu shortcut in "Wield shortcuts" with that key.
pub fn new_program_key(target: &Path, combo: Combo) -> Result<PathBuf, String> {
    let dir = PathBuf::from(expand_env(r"%APPDATA%\Microsoft\Windows\Start Menu\Programs\Wield shortcuts"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Program".into());
    let path = dir.join(format!("{name}.lnk"));
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        let t = wide(&target.to_string_lossy());
        link.SetPath(PCWSTR(t.as_ptr())).map_err(|e| e.message())?;
        if let Some(parent) = target.parent() {
            let w = wide(&parent.to_string_lossy());
            let _ = link.SetWorkingDirectory(PCWSTR(w.as_ptr()));
        }
        link.SetHotkey(combo.to_lnk()).map_err(|e| e.message())?;
        let file: IPersistFile = link.cast().map_err(|e| e.message())?;
        let p = wide(&path.to_string_lossy());
        file.Save(PCWSTR(p.as_ptr()), true).map_err(|e| e.message())?;
    }
    Ok(path)
}

/// Shortcut keys must use Ctrl+Alt, Ctrl+Shift or Ctrl+Alt+Shift (or a bare F-key), and Windows
/// only honors them on Start menu and desktop shortcuts.
pub fn valid_link_combo(c: &Combo) -> Result<(), &'static str> {
    if c.win {
        return Err("Shortcut keys can't use the Windows key.");
    }
    let fkey = (0x70..=0x87).contains(&c.vk);
    if !fkey && !(c.ctrl && (c.alt || c.shift)) {
        return Err("Use Ctrl+Alt, Ctrl+Shift or Ctrl+Alt+Shift with a key (or an F-key), so it doesn't clash with typing.");
    }
    Ok(())
}

// ------------------------------------------------------------------ state Wield keeps

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    /// Shortcut path (lowercase) -> its original shortcut key.
    links: HashMap<String, u16>,
}

fn state_path() -> Option<PathBuf> {
    Some(crate::settings::data_dir()?.join("hotkeys.json"))
}

fn load_state() -> State {
    state_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_state(s: &State) -> Result<(), String> {
    let p = state_path().ok_or("No data folder")?;
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(p, serde_json::to_string_pretty(s).unwrap_or_default()).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ PowerToys

#[derive(Clone, Debug)]
pub struct PtShortcut {
    pub module: String,
    pub name: String,
    pub combo: Combo,
    pub enabled: bool,
}

pub fn powertoys_dir() -> PathBuf {
    PathBuf::from(expand_env(r"%LOCALAPPDATA%\Microsoft\PowerToys"))
}

/// "open_powerlauncher" / "ActivationShortcut" -> "Open powerlauncher" / "Activation shortcut"
fn pretty(name: &str) -> String {
    let mut s = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch == '_' {
            s.push(' ');
        } else if ch.is_uppercase() && i > 0 && !s.ends_with(' ') {
            s.push(' ');
            s.push(ch.to_ascii_lowercase());
        } else {
            s.push(if i == 0 { ch.to_ascii_uppercase() } else { ch });
        }
    }
    s.trim().to_string()
}

fn find_hotkeys(v: &serde_json::Value, name: &str, out: &mut Vec<(String, Combo)>) {
    match v {
        serde_json::Value::Object(m) => {
            let has = |k: &str| m.get(k).is_some_and(|x| x.is_boolean());
            if has("win") && has("ctrl") && has("alt") && has("shift") {
                let code = m.get("code").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                if code != 0 {
                    let b = |k: &str| m.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
                    out.push((pretty(name), Combo { win: b("win"), ctrl: b("ctrl"), alt: b("alt"), shift: b("shift"), vk: code }));
                }
                return;
            }
            for (k, x) in m {
                let n = if k == "value" || k == "properties" { name.to_string() } else { k.clone() };
                find_hotkeys(x, &n, out);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| find_hotkeys(x, name, out)),
        _ => {}
    }
}

/// PowerToys' own shortcuts, from its settings files.
pub fn powertoys() -> Vec<PtShortcut> {
    let dir = powertoys_dir();
    let general: serde_json::Value = std::fs::read_to_string(dir.join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    for e in rd.flatten().filter(|e| e.path().is_dir()) {
        let module = e.file_name().to_string_lossy().to_string();
        let Some(v) = std::fs::read_to_string(e.path().join("settings.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else { continue };
        let mut found = Vec::new();
        find_hotkeys(&v, "Shortcut", &mut found);
        let enabled = general["enabled"][&module].as_bool().unwrap_or(true);
        for (name, combo) in found {
            out.push(PtShortcut { module: module.clone(), name, combo, enabled });
        }
    }
    out.sort_by(|a, b| (a.module.to_lowercase(), a.name.clone()).cmp(&(b.module.to_lowercase(), b.name.clone())));
    out
}

/// Key remaps made in PowerToys Keyboard Manager: (from, to).
pub fn powertoys_remaps() -> Vec<(String, String)> {
    let Some(v) = std::fs::read_to_string(powertoys_dir().join("Keyboard Manager").join("default.json")).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else {
        return Vec::new();
    };
    let keys = |s: &str| s.split(';').filter_map(|k| k.trim().parse::<u32>().ok()).map(|vk| match vk {
        0x5B | 0x5C => "Win".to_string(),
        0x11 | 0xA2 | 0xA3 => "Ctrl".into(),
        0x12 | 0xA4 | 0xA5 => "Alt".into(),
        0x10 | 0xA0 | 0xA1 => "Shift".into(),
        _ => super::keys::key_name(vk),
    }).collect::<Vec<_>>().join("+");
    let mut out = Vec::new();
    for section in ["remapKeys", "remapShortcuts"] {
        for scope in ["inProcess", "global"] {
            for r in v[section][scope].as_array().into_iter().flatten() {
                let from = r["originalKeys"].as_str().map(keys).unwrap_or_default();
                let to = r["newRemapKeys"].as_str().map(keys).or_else(|| r["newRemapString"].as_str().map(|s| format!("text \"{s}\""))).unwrap_or_default();
                let app = r["targetApp"].as_str().filter(|s| !s.is_empty()).map(|a| format!(" (in {a})")).unwrap_or_default();
                if !from.is_empty() {
                    out.push((format!("{from}{app}"), to));
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ well-known apps

/// Default global hotkeys of popular apps: (process name, app, keys, action).
pub const KNOWN_APPS: &[(&str, &str, &str, &str)] = &[
    ("nvidia overlay.exe", "NVIDIA overlay", "Alt+Z", "Open the NVIDIA overlay"),
    ("nvidia overlay.exe", "NVIDIA overlay", "Alt+F9", "Start or stop recording"),
    ("nvidia overlay.exe", "NVIDIA overlay", "Alt+F10", "Save instant replay"),
    ("nvidia overlay.exe", "NVIDIA overlay", "Alt+F1", "Screenshot"),
    ("nvidia overlay.exe", "NVIDIA overlay", "Alt+R", "Performance overlay"),
    ("radeonsoftware.exe", "AMD Software", "Alt+R", "Open AMD Software overlay"),
    ("radeonsoftware.exe", "AMD Software", "Ctrl+Shift+E", "Start or stop recording"),
    ("radeonsoftware.exe", "AMD Software", "Ctrl+Shift+S", "Save instant replay"),
    ("radeonsoftware.exe", "AMD Software", "Ctrl+Shift+O", "Metrics overlay"),
    ("sharex.exe", "ShareX", "PrtScn", "Capture full screen"),
    ("sharex.exe", "ShareX", "Ctrl+PrtScn", "Capture a region"),
    ("sharex.exe", "ShareX", "Alt+PrtScn", "Capture the active window"),
    ("sharex.exe", "ShareX", "Shift+PrtScn", "Record the screen"),
    ("greenshot.exe", "Greenshot", "PrtScn", "Capture a region"),
    ("greenshot.exe", "Greenshot", "Alt+PrtScn", "Capture a window"),
    ("greenshot.exe", "Greenshot", "Ctrl+PrtScn", "Capture full screen"),
    ("lightshot.exe", "Lightshot", "PrtScn", "Capture a region"),
    ("snagit32.exe", "Snagit", "PrtScn", "Capture"),
    ("flow.launcher.exe", "Flow Launcher", "Alt+Space", "Open Flow Launcher"),
    ("windowsterminal.exe", "Windows Terminal", "Win+`", "Quake-mode terminal"),
    ("ditto.exe", "Ditto", "Ctrl+`", "Open clipboard history"),
    ("flux.exe", "f.lux", "Alt+End", "Turn f.lux off for an hour"),
    ("everything.exe", "Everything", "Ctrl+Alt+Shift+F", "Open search (if set)"),
];

/// Lowercase names of running processes.
pub fn running_processes() -> std::collections::HashSet<String> {
    let mut s = sysinfo::System::new();
    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    s.processes().values().map(|p| p.name().to_string_lossy().to_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prettifies_names() {
        assert_eq!(pretty("open_powerlauncher"), "Open powerlauncher");
        assert_eq!(pretty("ActivationShortcut"), "Activation shortcut");
    }

    #[test]
    fn finds_powertoys_style_hotkeys() {
        let v: serde_json::Value = serde_json::from_str(r#"{"properties":{"open_powerlauncher":{"win":false,"ctrl":false,"alt":true,"shift":false,"code":32,"key":""},"unused":{"win":false,"ctrl":false,"alt":false,"shift":false,"code":0,"key":""},"ActivationShortcut":{"value":{"win":true,"ctrl":true,"alt":false,"shift":false,"code":84}}}}"#).unwrap();
        let mut out = Vec::new();
        find_hotkeys(&v, "x", &mut out);
        out.sort_by_key(|o| o.0.clone());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "Activation shortcut");
        assert_eq!(out[0].1.label(), "Win+Ctrl+T");
        assert_eq!(out[1].1.label(), "Alt+Space");
    }

    #[test]
    fn validates_link_keys() {
        assert!(valid_link_combo(&Combo::parse("Ctrl+Alt+N").unwrap()).is_ok());
        assert!(valid_link_combo(&Combo::parse("F9").unwrap()).is_ok());
        assert!(valid_link_combo(&Combo::parse("Alt+N").unwrap()).is_err());
        assert!(valid_link_combo(&Combo::parse("Win+N").unwrap()).is_err());
    }

    #[test]
    fn known_app_keys_parse() {
        for (_, _, k, _) in KNOWN_APPS {
            assert!(Combo::parse(k).is_some(), "{k}");
        }
    }

    /// Creates a shortcut with a key, reads it back, turns it off and on, then deletes it.
    #[test]
    fn link_key_roundtrip() {
        crate::win::with_com(|| {
            let target = PathBuf::from(expand_env(r"%SystemRoot%\System32\notepad.exe"));
            let combo = Combo::parse("Ctrl+Alt+Shift+F12").unwrap();
            let path = new_program_key(&target, combo).unwrap();
            let l = link_keys().into_iter().find(|l| l.path == path).expect("listed");
            assert_eq!(l.combo, Some(combo));
            disable_link(&l).unwrap();
            let off = link_keys().into_iter().find(|l| l.path == path).expect("still listed while off");
            assert_eq!((off.combo, off.disabled), (None, Some(combo)));
            enable_link(&off).unwrap();
            assert_eq!(read_link(&path).map(|r| r.0), Some(combo.to_lnk()));
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(path.parent().unwrap());
        });
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Prints everything found on this PC; run with --ignored --nocapture.
    #[test]
    #[ignore]
    fn live_sources() {
        crate::win::with_com(|| {
            for l in link_keys() {
                println!("LNK {} = {:?} ({})", l.name, l.combo.map(|c| c.label()), l.target);
            }
        });
        for p in powertoys() {
            println!("PT {} / {} = {} (enabled {})", p.module, p.name, p.combo.label(), p.enabled);
        }
        for r in powertoys_remaps() {
            println!("REMAP {} -> {}", r.0, r.1);
        }
        let t = super::super::keys::taken(super::super::keys::probe_candidates());
        println!("TAKEN {}: {}", t.len(), t.iter().map(|c| c.label()).collect::<Vec<_>>().join(", "));
    }
}

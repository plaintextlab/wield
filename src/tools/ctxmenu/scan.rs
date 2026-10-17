//! Finds every right-click menu entry: commands (shell verbs), shell extensions, Windows 11
//! app entries, Send To items and the New menu, for every place you can right-click.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use winreg::RegKey;
use winreg::enums::*;

use super::reg::{Hive, classes, get_str, has_value, hkcr, indirect};
use crate::win::fileinfo::{expand_env, resolve_target, version_strings};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Loc {
    Files,
    Folders,
    Background,
    Desktop,
    Drives,
    ThisPc,
    RecycleBin,
    SendTo,
    New,
    FileType(String),
}

impl Loc {
    pub fn label(&self) -> String {
        match self {
            Loc::Files => "All files".into(),
            Loc::Folders => "Folders".into(),
            Loc::Background => "Folder background".into(),
            Loc::Desktop => "Desktop".into(),
            Loc::Drives => "Drives".into(),
            Loc::ThisPc => "This PC".into(),
            Loc::RecycleBin => "Recycle Bin".into(),
            Loc::SendTo => "Send to".into(),
            Loc::New => "New".into(),
            Loc::FileType(e) => format!("{e} files"),
        }
    }
    /// Fixed places listed in the sidebar (file types come after).
    pub const PLACES: [Loc; 9] = [Loc::Files, Loc::Folders, Loc::Background, Loc::Desktop, Loc::Drives, Loc::ThisPc, Loc::RecycleBin, Loc::SendTo, Loc::New];
}

pub const THIS_PC: &str = "CLSID\\{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
pub const RECYCLE_BIN: &str = "CLSID\\{645FF040-5081-101B-9F08-00AA002F954E}";

/// Registry scopes (under Classes) for each fixed place.
pub const SCOPES: &[(&str, Loc)] = &[
    ("*", Loc::Files),
    ("AllFilesystemObjects", Loc::Files),
    ("Directory", Loc::Folders),
    ("Folder", Loc::Folders),
    ("Directory\\Background", Loc::Background),
    ("LibraryFolder\\background", Loc::Background),
    ("DesktopBackground", Loc::Desktop),
    ("Drive", Loc::Drives),
    (THIS_PC, Loc::ThisPc),
    (RECYCLE_BIN, Loc::RecycleBin),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// A shell verb: shell\<name>\command.
    Command,
    /// A COM shell extension: shellex\ContextMenuHandlers.
    Extension,
    /// A Windows 11 packaged-app entry (from the app's manifest).
    App,
    SendTo,
    New,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Command => "Command",
            Kind::Extension => "Shell extension",
            Kind::App => "App (Windows 11 menu)",
            Kind::SendTo => "Send to",
            Kind::New => "New item",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    /// Registry scope under Classes ("*", "Directory\\Background", ".txt"…), empty for Send To and apps.
    pub scope: String,
    /// Verb or handler key name, ShellNew key path, Send To file name, or app verb id.
    pub key: String,
    pub locs: Vec<Loc>,
    pub name: String,
    pub command: String,
    pub clsid: Option<String>,
    pub file: Option<PathBuf>,
    pub publisher: String,
    pub microsoft: bool,
    pub in_machine: bool,
    pub in_user: bool,
    pub hidden: bool,
    pub shift_only: bool,
    /// Registered but never shown by design (ProgrammaticAccessOnly).
    pub never_shown: bool,
    pub custom: bool,
    pub package: Option<String>,
}

impl Entry {
    /// Full registry key of the entry (for display and Regedit).
    pub fn reg_path(&self) -> Option<String> {
        match self.kind {
            Kind::Command => Some(classes(&format!(r"{}\shell\{}", self.scope, self.key))),
            Kind::Extension => Some(classes(&format!(r"{}\shellex\ContextMenuHandlers\{}", self.scope, self.key))),
            Kind::New => Some(classes(&self.key)),
            _ => None,
        }
    }
    pub fn can_delete(&self) -> bool {
        !self.microsoft && matches!(self.kind, Kind::Command | Kind::Extension | Kind::New | Kind::SendTo)
    }
    pub fn can_hide(&self) -> bool {
        self.kind != Kind::New && !self.never_shown
    }
    pub fn place(&self) -> String {
        let mut v: Vec<String> = self.locs.iter().take(4).map(Loc::label).collect();
        if self.locs.len() > 4 {
            v.push(format!("+{} more", self.locs.len() - 4));
        }
        v.join(", ")
    }
}

/// Readable names for Windows' built-in verbs that have no label of their own.
fn known_verb(v: &str) -> Option<&'static str> {
    Some(match v.to_lowercase().as_str() {
        "open" => "Open",
        "opennewwindow" => "Open in new window",
        "opennewprocess" => "Open in new process",
        "opennewtab" => "Open in new tab",
        "edit" => "Edit",
        "print" => "Print",
        "printto" => "Print to",
        "runas" => "Run as administrator",
        "runasuser" => "Run as different user",
        "openas" => "Open with",
        "explore" => "Explore",
        "find" => "Search",
        "properties" => "Properties",
        "pintohome" => "Pin to Quick access",
        "cmd" => "Open command window here",
        "powershell" => "Open PowerShell window here",
        "play" => "Play",
        "preview" => "Preview",
        "install" => "Install",
        "setdesktopwallpaper" => "Set as desktop background",
        "rotateleft" => "Rotate left",
        "rotateright" => "Rotate right",
        "mount" => "Mount",
        "burn" => "Burn to disc",
        "format" => "Format",
        "eject" => "Eject",
        "empty" => "Empty Recycle Bin",
        "manage" => "Manage",
        "uninstall" => "Uninstall",
        "share" => "Share",
        "taskbarpin" => "Pin to taskbar",
        _ => return None,
    })
}

fn clean(s: &str) -> String {
    s.replace('&', "").trim().to_string()
}

struct Ctx {
    blocked: HashSet<String>,
    pubs: HashMap<PathBuf, (String, bool)>,
    windir: String,
}

impl Ctx {
    fn new() -> Self {
        let mut blocked = HashSet::new();
        for h in Hive::BOTH {
            if let Some(k) = h.open(BLOCKED) {
                blocked.extend(k.enum_values().flatten().map(|(n, _)| n.to_uppercase()));
            }
        }
        Ctx { blocked, pubs: HashMap::new(), windir: std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase() }
    }
    /// (publisher, is Microsoft) for a file, cached.
    fn publisher(&mut self, file: &Path) -> (String, bool) {
        if let Some(p) = self.pubs.get(file) {
            return p.clone();
        }
        let company = version_strings(file).company.unwrap_or_default().trim().to_string();
        let ms = company.to_lowercase().contains("microsoft") || file.to_string_lossy().to_lowercase().starts_with(&self.windir);
        let r = (company, ms);
        self.pubs.insert(file.to_path_buf(), r.clone());
        r
    }
}

pub const BLOCKED: &str = r"Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Blocked";

/// (name, dll) of a COM class.
pub fn clsid_info(clsid: &str) -> (String, Option<PathBuf>) {
    let k = hkcr(&format!(r"CLSID\{clsid}"));
    let name = k.as_ref().and_then(|k| get_str(k, "")).map(|s| indirect(&s)).unwrap_or_default();
    let dll = k
        .as_ref()
        .and_then(|k| k.open_subkey_with_flags("InprocServer32", KEY_READ).ok())
        .and_then(|s| get_str(&s, ""))
        .filter(|s| !s.trim().is_empty())
        .map(|s| PathBuf::from(expand_env(s.trim().trim_matches('"'))))
        .map(|p| if p.is_absolute() { p } else { PathBuf::from(expand_env(r"%SystemRoot%\System32")).join(p) });
    (name, dll)
}

fn hives_with(path: &str) -> (bool, bool) {
    (Hive::Machine.exists(path), Hive::User.exists(path))
}

fn scan_verbs(ctx: &mut Ctx, scope: &str, locs: &[Loc], out: &mut Vec<Entry>) {
    let Some(shell) = hkcr(&format!(r"{scope}\shell")) else { return };
    for verb in shell.enum_keys().flatten() {
        let Ok(k) = shell.open_subkey_with_flags(&verb, KEY_READ) else { continue };
        let label = get_str(&k, "MUIVerb").filter(|s| !s.is_empty()).or_else(|| get_str(&k, "").filter(|s| !s.is_empty())).map(|s| clean(&indirect(&s)));
        let name = label.filter(|s| !s.is_empty()).or_else(|| known_verb(&verb).map(String::from)).unwrap_or_else(|| verb.clone());
        let command = k.open_subkey_with_flags("command", KEY_READ).ok().and_then(|c| get_str(&c, "")).unwrap_or_default();
        let clsid = ["ExplorerCommandHandler", "DelegateExecute"]
            .iter()
            .find_map(|v| get_str(&k, v))
            .or_else(|| k.open_subkey_with_flags("command", KEY_READ).ok().and_then(|c| get_str(&c, "DelegateExecute")))
            .filter(|s| s.starts_with('{'));
        let submenu = has_value(&k, "SubCommands") || has_value(&k, "ExtendedSubCommandsKey");
        let (file, cmd_text) = if !command.is_empty() {
            (resolve_target(&command), command.clone())
        } else if let Some(c) = &clsid {
            let (n, dll) = clsid_info(c);
            (dll, if n.is_empty() { format!("COM {c}") } else { format!("{n} ({c})") })
        } else if submenu {
            (None, "Submenu".into())
        } else {
            (None, String::new())
        };
        let (publisher, microsoft) = match &file {
            Some(f) => ctx.publisher(f),
            None => (String::new(), known_verb(&verb).is_some()),
        };
        let path = classes(&format!(r"{scope}\shell\{verb}"));
        let (in_machine, in_user) = hives_with(&path);
        out.push(Entry {
            id: format!("cmd:{scope}\\{verb}").to_lowercase(),
            kind: Kind::Command,
            scope: scope.into(),
            key: verb.clone(),
            locs: locs.to_vec(),
            name,
            command: cmd_text,
            clsid,
            file,
            publisher,
            microsoft,
            in_machine,
            in_user,
            hidden: has_value(&k, "LegacyDisable"),
            shift_only: has_value(&k, "Extended"),
            never_shown: has_value(&k, "ProgrammaticAccessOnly"),
            custom: has_value(&k, "WieldCustom"),
            package: None,
        });
    }
}

fn scan_handlers(ctx: &mut Ctx, scope: &str, locs: &[Loc], out: &mut Vec<Entry>) {
    let Some(hs) = hkcr(&format!(r"{scope}\shellex\ContextMenuHandlers")) else { return };
    for key in hs.enum_keys().flatten() {
        let Ok(k) = hs.open_subkey_with_flags(&key, KEY_READ) else { continue };
        let value = get_str(&k, "").unwrap_or_default();
        let legacy_off = value.trim().starts_with("-{");
        let clsid = [value.trim().trim_start_matches('-'), key.as_str()].into_iter().find(|s| s.starts_with('{') && s.ends_with('}')).map(str::to_string);
        let (cname, dll) = clsid.as_deref().map(clsid_info).unwrap_or_default();
        let name = if !key.starts_with('{') { key.clone() } else if !cname.is_empty() { cname.clone() } else { key.clone() };
        let (publisher, microsoft) = match &dll {
            Some(f) => ctx.publisher(f),
            None => (String::new(), false),
        };
        let path = classes(&format!(r"{scope}\shellex\ContextMenuHandlers\{key}"));
        let (in_machine, in_user) = hives_with(&path);
        let hidden = legacy_off || clsid.as_ref().is_some_and(|c| ctx.blocked.contains(&c.to_uppercase()));
        out.push(Entry {
            id: format!("ext:{scope}\\{key}").to_lowercase(),
            kind: Kind::Extension,
            scope: scope.into(),
            key,
            locs: locs.to_vec(),
            command: match (&dll, cname.is_empty()) {
                (Some(d), _) => d.display().to_string(),
                (None, false) => cname,
                _ => clsid.clone().unwrap_or_default(),
            },
            name,
            clsid,
            file: dll,
            publisher,
            microsoft,
            in_machine,
            in_user,
            hidden,
            shift_only: false,
            never_shown: false,
            custom: false,
            package: None,
        });
    }
}

/// Every extension with the scopes it inherits menu entries from.
fn file_type_scopes() -> (BTreeMap<String, Vec<String>>, Vec<(String, String)>) {
    let mut scopes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut shellnew = Vec::new();
    let root = RegKey::predef(HKEY_CLASSES_ROOT);
    let user_choice = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts", KEY_READ).ok();
    for ext in root.enum_keys().flatten().filter(|k| k.starts_with('.')) {
        let Ok(k) = root.open_subkey_with_flags(&ext, KEY_READ) else { continue };
        let progid = get_str(&k, "").filter(|s| !s.trim().is_empty());
        let mut add = |scope: String| scopes.entry(scope).or_default().push(ext.to_lowercase());
        add(ext.clone());
        if let Some(p) = &progid {
            add(p.clone());
        }
        if let Some(p) = user_choice.as_ref().and_then(|u| u.open_subkey_with_flags(format!(r"{ext}\UserChoice"), KEY_READ).ok()).and_then(|u| get_str(&u, "ProgId")) {
            if progid.as_deref() != Some(p.as_str()) {
                add(p);
            }
        }
        add(format!(r"SystemFileAssociations\{ext}"));
        if let Some(pt) = get_str(&k, "PerceivedType").filter(|s| !s.is_empty()) {
            add(format!(r"SystemFileAssociations\{pt}"));
        }
        // The New menu: .ext\ShellNew or .ext\<ProgID>\ShellNew.
        if k.open_subkey_with_flags("ShellNew", KEY_READ).is_ok() {
            shellnew.push((ext.clone(), format!(r"{ext}\ShellNew")));
        } else if let Some(p) = &progid {
            if k.open_subkey_with_flags(format!(r"{p}\ShellNew"), KEY_READ).is_ok() {
                shellnew.push((ext.clone(), format!(r"{ext}\{p}\ShellNew")));
            }
        }
    }
    (scopes, shellnew)
}

fn scan_shellnew(items: Vec<(String, String)>, out: &mut Vec<Entry>) {
    for (ext, path) in items {
        let progid = hkcr(&ext).and_then(|k| get_str(&k, "")).unwrap_or_default();
        let pk = hkcr(&progid);
        let name = pk
            .as_ref()
            .and_then(|k| get_str(k, "FriendlyTypeName").map(|s| indirect(&s)).filter(|s| !s.is_empty()).or_else(|| get_str(k, "")))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{} file", ext.trim_start_matches('.').to_uppercase()));
        let (in_machine, in_user) = hives_with(&classes(&path));
        out.push(Entry {
            id: format!("new:{path}").to_lowercase(),
            kind: Kind::New,
            scope: ext.clone(),
            key: path,
            locs: vec![Loc::New],
            name,
            command: format!("Creates a new {ext} file"),
            clsid: None,
            file: None,
            publisher: String::new(),
            microsoft: [".lnk", ".library-ms", ".zip", ".txt", ".bmp", ".rtf", ".contact", ".jnt"].contains(&ext.to_lowercase().as_str()) || ext.is_empty(),
            in_machine,
            in_user,
            hidden: false,
            shift_only: false,
            never_shown: false,
            custom: false,
            package: None,
        });
    }
}

pub fn sendto_dir() -> PathBuf {
    PathBuf::from(expand_env(r"%APPDATA%\Microsoft\Windows\SendTo"))
}

/// Where Wield keeps Send To items you've hidden.
pub fn sendto_hidden_dir() -> PathBuf {
    crate::settings::data_dir().unwrap_or_else(std::env::temp_dir).join("ctxmenu").join("SendTo-hidden")
}

fn sendto_name(file: &Path) -> String {
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    match ext.as_str() {
        "desklink" => "Desktop (create shortcut)".into(),
        "zfsendtotarget" => "Compressed (zipped) folder".into(),
        "mapimail" => "Mail recipient".into(),
        "mydocs" => "Documents".into(),
        _ => stem,
    }
}

fn scan_sendto(ctx: &mut Ctx, out: &mut Vec<Entry>) {
    for (dir, hidden) in [(sendto_dir(), false), (sendto_hidden_dir(), true)] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            let fname = f.file_name().to_string_lossy().to_string();
            if fname.eq_ignore_ascii_case("desktop.ini") || p.is_dir() {
                continue;
            }
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let target = if ext == "lnk" { crate::win::fileinfo::resolve_shortcut(&p).map(|(t, _)| PathBuf::from(t)) } else { None };
            let builtin = ["desklink", "zfsendtotarget", "mapimail", "mydocs"].contains(&ext.as_str()) || fname.to_lowercase().contains("fax recipient") || fname.to_lowercase().contains("bluetooth");
            let (publisher, microsoft) = match &target {
                Some(t) => ctx.publisher(t),
                None => (String::new(), builtin),
            };
            out.push(Entry {
                id: format!("sendto:{fname}").to_lowercase(),
                kind: Kind::SendTo,
                scope: String::new(),
                key: fname.clone(),
                locs: vec![Loc::SendTo],
                name: sendto_name(&p),
                command: target.as_ref().map(|t| t.display().to_string()).unwrap_or_else(|| p.display().to_string()),
                clsid: None,
                file: target,
                publisher,
                microsoft: microsoft || builtin,
                in_machine: false,
                in_user: true,
                hidden,
                shift_only: false,
                never_shown: false,
                custom: false,
                package: None,
            });
        }
    }
}

/// Context menu verbs declared by installed packaged apps (the Windows 11 menu).
fn scan_packages(ctx: &mut Ctx, out: &mut Vec<Entry>) {
    let Some(repo) = Hive::User.open(r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages") else { return };
    for full in repo.enum_keys().flatten() {
        let Ok(k) = repo.open_subkey_with_flags(&full, KEY_READ) else { continue };
        let Some(root) = get_str(&k, "PackageRootFolder") else { continue };
        let Ok(xml) = std::fs::read_to_string(Path::new(&root).join("AppxManifest.xml")) else { continue };
        if !xml.contains("FileExplorerContextMenus") {
            continue;
        }
        let display = get_str(&k, "DisplayName").map(|s| indirect(&s)).filter(|s| !s.is_empty()).unwrap_or_else(|| full.split('_').next().unwrap_or(&full).to_string());
        for (item_type, id, clsid) in parse_manifest_menus(&xml) {
            let loc = match item_type.as_str() {
                "*" => Loc::Files,
                "Directory" => Loc::Folders,
                "Directory\\Background" => Loc::Background,
                "Drive" => Loc::Drives,
                t if t.starts_with('.') => Loc::FileType(t.to_lowercase()),
                _ => Loc::Files,
            };
            let entry_id = format!("app:{full}:{item_type}:{id}").to_lowercase();
            if out.iter().any(|e| e.id == entry_id) {
                continue;
            }
            let (publisher, microsoft) = ctx.publisher(&Path::new(&root).join("AppxManifest.xml"));
            let publisher = if publisher.is_empty() { full.split('_').next_back().unwrap_or("").to_string() } else { publisher };
            out.push(Entry {
                id: entry_id,
                kind: Kind::App,
                scope: item_type.clone(),
                key: id.clone(),
                locs: vec![loc],
                name: format!("{display}: {id}"),
                command: format!("{root} ({clsid})"),
                hidden: ctx.blocked.contains(&clsid.to_uppercase()),
                clsid: Some(clsid),
                file: None,
                publisher,
                microsoft: microsoft || full.starts_with("Microsoft."),
                in_machine: true,
                in_user: false,
                shift_only: false,
                never_shown: false,
                custom: false,
                package: Some(display.clone()),
            });
        }
    }
}

/// (item type, verb id, clsid) for each `<…:ItemType Type="…"><…:Verb Id="…" Clsid="…"/>` in a manifest.
pub fn parse_manifest_menus(xml: &str) -> Vec<(String, String, String)> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let i = tag.find(&format!("{name}=\""))? + name.len() + 2;
        tag[i..].split('"').next().map(str::to_string)
    };
    let mut out = Vec::new();
    let mut current_type = String::from("*");
    for part in xml.split('<').skip(1) {
        let tag = part.split('>').next().unwrap_or("");
        let tname = tag.split_whitespace().next().unwrap_or("");
        if tname.ends_with(":ItemType") {
            current_type = attr(tag, "Type").unwrap_or_else(|| "*".into());
        } else if tname.ends_with(":Verb") {
            if let (Some(id), Some(clsid)) = (attr(tag, "Id"), attr(tag, "Clsid")) {
                let c = if clsid.starts_with('{') { clsid } else { format!("{{{clsid}}}") };
                out.push((current_type.clone(), id, c));
            }
        }
    }
    out
}

/// Scans everything. Takes a second or two (it reads every file type).
pub fn scan_all() -> Vec<Entry> {
    let mut ctx = Ctx::new();
    let mut out = Vec::new();
    for (scope, loc) in SCOPES {
        scan_verbs(&mut ctx, scope, std::slice::from_ref(loc), &mut out);
        scan_handlers(&mut ctx, scope, std::slice::from_ref(loc), &mut out);
    }
    let (types, shellnew) = file_type_scopes();
    for (scope, exts) in types {
        let mut exts = exts;
        exts.sort();
        exts.dedup();
        let locs: Vec<Loc> = exts.into_iter().map(Loc::FileType).collect();
        scan_verbs(&mut ctx, &scope, &locs, &mut out);
        scan_handlers(&mut ctx, &scope, &locs, &mut out);
    }
    scan_shellnew(shellnew, &mut out);
    scan_sendto(&mut ctx, &mut out);
    scan_packages(&mut ctx, &mut out);
    // A scope reachable from several extensions is scanned once; drop exact duplicates just in case.
    let mut seen = HashSet::new();
    out.retain(|e| seen.insert(e.id.clone()));
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_the_real_menus() {
        let all = scan_all();
        assert!(all.len() > 50, "found {}", all.len());
        // Every PC has these.
        assert!(all.iter().any(|e| e.kind == Kind::Command && e.scope.eq_ignore_ascii_case("exefile") && e.key.eq_ignore_ascii_case("runas")));
        assert!(all.iter().any(|e| e.kind == Kind::SendTo));
        assert!(all.iter().any(|e| e.kind == Kind::New));
        assert!(all.iter().any(|e| e.locs.contains(&Loc::FileType(".txt".into()))));
        let ids: HashSet<_> = all.iter().map(|e| &e.id).collect();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn parses_packaged_menus() {
        let xml = r#"<Extensions><desktop4:Extension Category="windows.fileExplorerContextMenus"><desktop4:FileExplorerContextMenus>
            <desktop5:ItemType Type="Directory"><desktop5:Verb Id="Open" Clsid="11111111-2222-3333-4444-555555555555" /></desktop5:ItemType>
            <desktop4:ItemType Type="*"><desktop4:Verb Id="Edit" Clsid="{AAAAAAAA-2222-3333-4444-555555555555}"/></desktop4:ItemType>
            </desktop4:FileExplorerContextMenus></desktop4:Extension></Extensions>"#;
        let v = parse_manifest_menus(xml);
        assert_eq!(v[0], ("Directory".into(), "Open".into(), "{11111111-2222-3333-4444-555555555555}".into()));
        assert_eq!(v[1].0, "*");
    }
}

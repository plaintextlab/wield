//! Your own right-click entries: any program, batch file, PowerShell script or command line,
//! as single items or a whole submenu, on files, chosen file types, folders, folder
//! backgrounds, the desktop or drives.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW};
use windows::core::PWSTR;

use super::edit::data_dir;
use super::reg::{self, Hive, classes, write_err};
use crate::tools::backup::image::{Check, Level};
use crate::win::fileinfo::expand_env;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Window {
    /// Shows its window (a console for scripts) and closes when done.
    Normal,
    /// Console stays open afterwards, so you can read the output or errors.
    KeepOpen,
    /// No window at all.
    Hidden,
}

impl Window {
    pub fn label(self) -> &'static str {
        match self {
            Window::Normal => "Show window",
            Window::KeepOpen => "Keep window open after it finishes",
            Window::Hidden => "Run hidden (no window)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Position {
    Default,
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub label: String,
    /// Program, .bat/.cmd, .ps1, .vbs… Empty means `args` is a complete command line.
    pub program: String,
    /// `{path}` is replaced by the clicked file or folder.
    pub args: String,
    pub window: Window,
    pub icon: String,
}

impl Default for Command {
    fn default() -> Self {
        Command { label: String::new(), program: String::new(), args: "\"{path}\"".into(), window: Window::Normal, icon: String::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Targets {
    pub files: bool,
    /// Space- or comma-separated extensions, e.g. ".webp .png".
    pub types: String,
    pub folders: bool,
    pub background: bool,
    pub desktop: bool,
    pub drives: bool,
}

impl Targets {
    pub fn extensions(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .types
            .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
            .map(|s| s.trim().trim_start_matches('*').to_lowercase())
            .filter(|s| !s.is_empty())
            .map(|s| if s.starts_with('.') { s } else { format!(".{s}") })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// (registry scope, placeholder Explorer fills in) for every place picked.
    pub fn scopes(&self) -> Vec<(String, &'static str)> {
        let mut v = Vec::new();
        if self.files {
            v.push(("*".to_string(), "%1"));
        }
        for e in self.extensions() {
            v.push((format!(r"SystemFileAssociations\{e}"), "%1"));
        }
        if self.folders {
            v.push(("Directory".into(), "%1"));
        }
        if self.background {
            v.push((r"Directory\Background".into(), "%V"));
        }
        if self.desktop {
            v.push(("DesktopBackground".into(), "%V"));
        }
        if self.drives {
            v.push(("Drive".into(), "%1"));
        }
        v
    }

    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        if self.files {
            v.push("all files".to_string());
        }
        let ext = self.extensions();
        if !ext.is_empty() {
            v.push(format!("{} files", ext.join(" ")));
        }
        for (on, t) in [(self.folders, "folders"), (self.background, "inside folders"), (self.desktop, "desktop"), (self.drives, "drives")] {
            if on {
                v.push(t.into());
            }
        }
        v.join(", ")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Custom {
    pub id: String,
    /// Name of the submenu; empty puts the commands straight in the menu.
    pub submenu: String,
    pub icon: String,
    pub commands: Vec<Command>,
    pub targets: Targets,
    pub shift_only: bool,
    pub position: Position,
    pub all_users: bool,
    /// Keys written last time (to remove or replace them exactly).
    #[serde(default)]
    pub written: Vec<String>,
}

impl Default for Custom {
    fn default() -> Self {
        Custom {
            id: String::new(),
            submenu: String::new(),
            icon: String::new(),
            commands: vec![Command::default()],
            targets: Targets::default(),
            shift_only: false,
            position: Position::Default,
            all_users: false,
            written: Vec::new(),
        }
    }
}

impl Custom {
    pub fn title(&self) -> String {
        if !self.submenu.trim().is_empty() {
            self.submenu.trim().to_string()
        } else {
            self.commands.iter().map(|c| c.label.trim()).collect::<Vec<_>>().join(", ")
        }
    }
    fn hive(&self) -> Hive {
        if self.all_users { Hive::Machine } else { Hive::User }
    }
}

fn ext_of(p: &str) -> String {
    Path::new(p.trim().trim_matches('"')).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Where a program name like "notepad.exe" or "%SystemRoot%\x.exe" really is.
pub fn find_program(p: &str) -> Option<PathBuf> {
    let p = PathBuf::from(expand_env(p.trim().trim_matches('"')));
    if p.is_absolute() {
        return p.is_file().then_some(p);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    dirs.insert(0, PathBuf::from(expand_env(r"%SystemRoot%\System32")));
    for d in dirs {
        for cand in [d.join(&p), d.join(format!("{}.exe", p.display()))] {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// The command line Explorer will run. `ph` is %1 (clicked item) or %V (current folder).
pub fn command_line(c: &Command, ph: &str, wield: &Path) -> String {
    let args = c.args.replace("{path}", ph);
    let prog = expand_env(c.program.trim().trim_matches('"'));
    let keep = c.window == Window::KeepOpen;
    let line = if prog.is_empty() {
        args
    } else {
        match ext_of(&prog).as_str() {
            "bat" | "cmd" => format!("cmd.exe /{} \"\"{prog}\" {args}\"", if keep { "k" } else { "c" }),
            "ps1" => format!(
                "powershell.exe -NoProfile -ExecutionPolicy Bypass {}{}-File \"{prog}\" {args}",
                if keep { "-NoExit " } else { "" },
                if c.window == Window::Hidden { "-WindowStyle Hidden " } else { "" }
            ),
            "vbs" | "vbe" | "js" | "jse" | "wsf" => format!("wscript.exe \"{prog}\" {args}"),
            "exe" | "com" => {
                if keep {
                    format!("cmd.exe /k \"\"{prog}\" {args}\"")
                } else {
                    format!("\"{prog}\" {args}")
                }
            }
            // Anything else opens with its associated program.
            _ => format!("cmd.exe /{} \"\"{prog}\" {args}\"", if keep { "k" } else { "c" }),
        }
    };
    let line = line.trim().to_string();
    if c.window == Window::Hidden { format!("\"{}\" --run-hidden {line}", wield.display()) } else { line }
}

/// Starts a command line exactly the way Explorer does (CreateProcess).
pub fn spawn(cmdline: &str, hidden: bool) -> Result<(), String> {
    let mut w: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();
    let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut pi = PROCESS_INFORMATION::default();
    let flags = if hidden { CREATE_NO_WINDOW } else { PROCESS_CREATION_FLAGS(0) };
    unsafe {
        CreateProcessW(None, Some(PWSTR(w.as_mut_ptr())), None, None, false, flags, None, None, &si, &mut pi).map_err(|e| format!("Couldn't start it: {}", e.message()))?;
        let _ = windows::Win32::Foundation::CloseHandle(pi.hProcess);
        let _ = windows::Win32::Foundation::CloseHandle(pi.hThread);
    }
    Ok(())
}

/// `wield.exe --run-hidden <command line>`: runs the command without a window and exits.
/// Returns false when Wield wasn't started that way.
pub fn run_hidden_main() -> bool {
    let raw = unsafe { windows::Win32::System::Environment::GetCommandLineW().to_string() }.unwrap_or_default();
    let Some(i) = raw.find(" --run-hidden ") else { return false };
    let _ = spawn(raw[i + " --run-hidden ".len()..].trim(), true);
    true
}

/// Runs a command for a sample file/folder, to test it before (or after) adding it.
pub fn test(c: &Command, ph: &str, sample: &Path) -> Result<String, String> {
    let wield = std::env::current_exe().unwrap_or_default();
    let line = command_line(c, ph, &wield).replace(ph, &sample.display().to_string());
    spawn(&line, false)?;
    Ok(line)
}

/// Checks a custom entry before it's saved.
pub fn validate(c: &Custom, elevated: bool, existing: &[Custom]) -> Vec<Check> {
    let mut out = Vec::new();
    let mut add = |level, text: String| out.push(Check { level, text });
    let t = &c.targets;
    if t.scopes().is_empty() {
        add(Level::Block, "Choose where the entry should appear.".into());
    }
    for e in t.extensions() {
        if e.len() < 2 || !e[1..].chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            add(Level::Block, format!("\"{e}\" isn't a valid file type. Use something like .webp"));
        }
    }
    if c.commands.is_empty() {
        add(Level::Block, "Add at least one command.".into());
    }
    if c.commands.len() > 1 && c.submenu.trim().is_empty() {
        add(Level::Ok, "No submenu name: the commands go straight into the menu.".into());
    }
    for (i, cmd) in c.commands.iter().enumerate() {
        let n = if cmd.label.trim().is_empty() { format!("Command {}", i + 1) } else { format!("\"{}\"", cmd.label.trim()) };
        if cmd.label.trim().is_empty() {
            add(Level::Block, format!("{n} needs a name."));
        }
        if cmd.program.trim().is_empty() {
            if cmd.args.trim().is_empty() {
                add(Level::Block, format!("{n}: choose a program or script, or type a command line."));
            }
        } else if find_program(&cmd.program).is_none() {
            add(Level::Block, format!("{n}: {} wasn't found.", cmd.program.trim()));
        }
        if !cmd.args.contains("{path}") && !cmd.program.trim().is_empty() {
            add(Level::Warn, format!("{n} doesn't use {{path}}, so it won't know which file or folder was clicked."));
        }
        if cmd.args.contains("{path}") && !cmd.args.contains("\"{path}\"") && !cmd.args.contains("'{path}'") {
            add(Level::Warn, format!("{n}: put quotes around {{path}} (\"{{path}}\") or paths with spaces will break."));
        }
        if cmd.window == Window::Hidden {
            let exe = std::env::current_exe().unwrap_or_default();
            if matches!(crate::update::mode(), crate::update::Mode::Portable { .. }) {
                add(Level::Warn, format!("{n} runs hidden through Wield at {}. If you move that file the command stops working.", exe.display()));
            }
        }
        if ext_of(&cmd.program) == "ps1" {
            add(Level::Ok, format!("{n}: PowerShell runs the script even if scripts are normally blocked (-ExecutionPolicy Bypass)."));
        }
    }
    let mut labels: Vec<String> = c.commands.iter().map(|x| x.label.trim().to_lowercase()).collect();
    labels.sort();
    if labels.windows(2).any(|w| w[0] == w[1] && !w[0].is_empty()) {
        add(Level::Warn, "Two commands have the same name.".into());
    }
    if existing.iter().any(|x| x.id != c.id && x.title().eq_ignore_ascii_case(&c.title()) && x.targets.scopes().iter().any(|s| c.targets.scopes().contains(s))) {
        add(Level::Warn, "You already have an entry with this name in the same place.".into());
    }
    if c.all_users && !elevated {
        add(Level::Block, "Adding an entry for all users needs administrator rights.".into());
    }
    if !t.extensions().is_empty() {
        add(Level::Ok, "File-type entries are added to Windows' per-type list, so they keep working when you change the default app.".into());
    }
    out
}

fn defs_path() -> PathBuf {
    data_dir().join("custom.json")
}

pub fn load() -> Vec<Custom> {
    std::fs::read_to_string(defs_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_all(v: &[Custom]) -> Result<(), String> {
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(defs_path(), serde_json::to_string_pretty(v).unwrap_or_default()).map_err(|e| e.to_string())
}

fn safe_id(s: &str) -> String {
    let id: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect();
    format!("{}{}", if id.is_empty() { "Entry".into() } else { id }, crate::tools::backup::sys::now_unix() % 100_000)
}

fn remove_keys(c: &Custom) -> Result<(), String> {
    let h = c.hive();
    for p in &c.written {
        reg::delete_path(h, p).map_err(|e| write_err(h, e))?;
    }
    Ok(())
}

fn write_command(key: &winreg::RegKey, c: &Command, ph: &str, wield: &Path) -> std::io::Result<()> {
    key.set_value("MUIVerb", &c.label.trim().to_string())?;
    let icon = if !c.icon.trim().is_empty() {
        expand_env(c.icon.trim())
    } else if ext_of(&c.program) == "exe" {
        find_program(&c.program).map(|p| format!("{},0", p.display())).unwrap_or_default()
    } else {
        String::new()
    };
    if !icon.is_empty() {
        key.set_value("Icon", &icon)?;
    }
    // Run once per selected item, with no 15-item limit.
    key.set_value("MultiSelectModel", &"Player")?;
    key.create_subkey("command")?.0.set_value("", &command_line(c, ph, wield))
}

/// Adds or updates a custom entry in the registry and remembers it.
pub fn save(mut c: Custom, elevated: bool) -> Result<Custom, String> {
    if validate(&c, elevated, &[]).iter().any(|x| x.level == Level::Block) {
        return Err("Fix the problems shown first.".into());
    }
    let mut all = load();
    if c.id.is_empty() {
        c.id = safe_id(&c.title());
    }
    // Replace whatever this entry wrote before (it may have moved places or hives).
    if let Some(old) = all.iter().find(|x| x.id == c.id) {
        remove_keys(old)?;
    }
    let h = c.hive();
    let wield = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut written = Vec::new();
    let r: std::io::Result<()> = (|| {
        for (scope, ph) in c.targets.scopes() {
            let flags = |k: &winreg::RegKey| -> std::io::Result<()> {
                k.set_value("WieldCustom", &"1")?;
                if c.shift_only {
                    k.set_value("Extended", &"")?;
                }
                match c.position {
                    Position::Top => k.set_value("Position", &"Top")?,
                    Position::Bottom => k.set_value("Position", &"Bottom")?,
                    Position::Default => {}
                }
                Ok(())
            };
            if !c.submenu.trim().is_empty() {
                let path = classes(&format!(r"{scope}\shell\Wield.{}", c.id));
                written.push(path.clone());
                let k = h.create(&path)?;
                k.set_value("MUIVerb", &c.submenu.trim().to_string())?;
                k.set_value("SubCommands", &"")?;
                if !c.icon.trim().is_empty() {
                    k.set_value("Icon", &expand_env(c.icon.trim()))?;
                }
                flags(&k)?;
                for (i, cmd) in c.commands.iter().enumerate() {
                    let sub = k.create_subkey(format!(r"shell\{:02}", i + 1))?.0;
                    write_command(&sub, cmd, ph, &wield)?;
                }
            } else {
                for (i, cmd) in c.commands.iter().enumerate() {
                    let path = classes(&format!(r"{scope}\shell\Wield.{}.{}", c.id, i + 1));
                    written.push(path.clone());
                    let k = h.create(&path)?;
                    write_command(&k, cmd, ph, &wield)?;
                    flags(&k)?;
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = r {
        for p in &written {
            let _ = reg::delete_path(h, p);
        }
        return Err(write_err(h, e));
    }
    c.written = written;
    all.retain(|x| x.id != c.id);
    all.push(c.clone());
    save_all(&all)?;
    Ok(c)
}

pub fn remove(id: &str) -> Result<(), String> {
    let mut all = load();
    if let Some(c) = all.iter().find(|x| x.id == id) {
        remove_keys(c)?;
    }
    all.retain(|x| x.id != id);
    save_all(&all)
}

/// Ready-made starting points.
pub fn templates() -> Vec<(&'static str, Custom)> {
    let one = |label: &str, program: &str, args: &str, window: Window, targets: Targets| Custom {
        commands: vec![Command { label: label.into(), program: program.into(), args: args.into(), window, icon: String::new() }],
        targets,
        ..Default::default()
    };
    vec![
        (
            "Run my script on a file type (e.g. convert .webp to JPEG)",
            one("Convert to JPEG", "", "\"{path}\"", Window::Normal, Targets { types: ".webp".into(), ..Default::default() }),
        ),
        (
            "Open PowerShell here",
            one(
                "Open PowerShell here",
                "powershell.exe",
                "-NoExit -Command \"Set-Location -LiteralPath '{path}'\"",
                Window::Normal,
                Targets { folders: true, background: true, desktop: true, ..Default::default() },
            ),
        ),
        (
            "Open Command Prompt here",
            one("Open Command Prompt here", "cmd.exe", "/k cd /d \"{path}\"", Window::Normal, Targets { folders: true, background: true, desktop: true, ..Default::default() }),
        ),
        ("Open with Notepad", one("Open with Notepad", "notepad.exe", "\"{path}\"", Window::Normal, Targets { files: true, ..Default::default() })),
        (
            "Copy full path",
            one("Copy full path", "powershell.exe", "-NoProfile -Command \"Set-Clipboard -Value '{path}'\"", Window::Hidden, Targets { files: true, folders: true, ..Default::default() }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(program: &str, args: &str, window: Window) -> Command {
        Command { label: "X".into(), program: program.into(), args: args.into(), window, icon: String::new() }
    }

    #[test]
    fn builds_command_lines() {
        let w = Path::new(r"C:\Apps\wield.exe");
        assert_eq!(command_line(&cmd(r"C:\s\to jpg.bat", "\"{path}\"", Window::Normal), "%1", w), r#"cmd.exe /c ""C:\s\to jpg.bat" "%1"""#);
        assert_eq!(command_line(&cmd(r"C:\s\a.bat", "\"{path}\"", Window::KeepOpen), "%1", w), r#"cmd.exe /k ""C:\s\a.bat" "%1"""#);
        assert_eq!(
            command_line(&cmd(r"C:\s\a.ps1", "\"{path}\"", Window::Hidden), "%V", w),
            r#""C:\Apps\wield.exe" --run-hidden powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "C:\s\a.ps1" "%V""#
        );
        assert_eq!(command_line(&cmd(r"C:\p\x.exe", "-a \"{path}\"", Window::Normal), "%1", w), r#""C:\p\x.exe" -a "%1""#);
        assert_eq!(command_line(&cmd("", "notepad.exe \"{path}\"", Window::Normal), "%1", w), r#"notepad.exe "%1""#);
    }

    #[test]
    fn parses_targets() {
        let t = Targets { types: "webp, .PNG;*.jpg  heic".into(), background: true, ..Default::default() };
        assert_eq!(t.extensions(), vec![".heic", ".jpg", ".png", ".webp"]);
        let s = t.scopes();
        assert!(s.contains(&(r"SystemFileAssociations\.webp".into(), "%1")));
        assert!(s.contains(&(r"Directory\Background".into(), "%V")));
    }

    #[test]
    fn validates() {
        let mut c = Custom::default();
        assert!(validate(&c, false, &[]).iter().any(|x| x.level == Level::Block));
        c.targets.types = ".webp".into();
        c.commands[0] = cmd("notepad.exe", "\"{path}\"", Window::Normal);
        assert!(!validate(&c, false, &[]).iter().any(|x| x.level == Level::Block), "{:?}", validate(&c, false, &[]));
        c.commands[0].program = r"C:\nope\missing.bat".into();
        assert!(validate(&c, false, &[]).iter().any(|x| x.level == Level::Block));
    }

    /// Adds a real submenu for a made-up file type, checks the scanner sees it, hides it, deletes
    /// and restores it, then removes it.
    #[test]
    fn adds_finds_hides_and_removes_an_entry() {
        let _lock = super::super::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use super::super::{edit, scan};
        let c = Custom {
            submenu: "Wield test menu".into(),
            commands: vec![
                Command { label: "Convert".into(), program: "notepad.exe".into(), ..Default::default() },
                Command { label: "Other".into(), program: "notepad.exe".into(), ..Default::default() },
            ],
            targets: Targets { types: ".wieldtest".into(), ..Default::default() },
            ..Default::default()
        };
        let c = save(c, false).unwrap();
        assert_eq!(c.written.len(), 1);
        let k = Hive::User.open(&c.written[0]).unwrap();
        let cmdline: String = k.open_subkey(r"shell\01\command").unwrap().get_value("").unwrap();
        assert!(cmdline.to_lowercase().contains("notepad.exe\" \"%1\""), "{cmdline}");

        // The scanner only lists extensions that exist; give the type a key like a real install would.
        Hive::User.create(r"Software\Classes\.wieldtest").unwrap();
        let find = || scan::scan_all().into_iter().find(|e| e.custom && e.key == format!("Wield.{}", c.id)).expect("custom entry found");
        let e = find();
        assert_eq!(e.name, "Wield test menu");
        assert!(e.locs.contains(&scan::Loc::FileType(".wieldtest".into())));

        edit::set_hidden(&e, true).unwrap();
        assert!(find().hidden);
        edit::set_hidden(&e, false).unwrap();
        assert!(!find().hidden);
        edit::set_shift_only(&e, true).unwrap();
        assert!(find().shift_only);
        edit::set_shift_only(&e, false).unwrap();

        edit::delete(&e, false).unwrap();
        assert!(!Hive::User.exists(&c.written[0]));
        let idx = edit::deleted().iter().rposition(|d| d.name == "Wield test menu").unwrap();
        edit::restore_deleted(idx, false).unwrap();
        assert_eq!(Hive::User.open(&c.written[0]).unwrap().open_subkey(r"shell\01\command").unwrap().get_value::<String, _>("").unwrap(), cmdline);

        remove(&c.id).unwrap();
        assert!(!Hive::User.exists(&c.written[0]));
        let _ = reg::delete_path(Hive::User, r"Software\Classes\.wieldtest");
        let _ = reg::delete_path(Hive::User, r"Software\Classes\SystemFileAssociations\.wieldtest");
    }
}

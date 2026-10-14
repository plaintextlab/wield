//! Command-line parsing, path resolution and PE version-resource reading.

use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink};
use windows::core::{Interface, PCWSTR};

use super::wide;

/// Expands `%VAR%` references using the current process environment.
pub fn expand_env(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let src = wide(s);
    unsafe {
        let needed = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), None);
        if needed == 0 {
            return s.to_string();
        }
        let mut buf = vec![0u16; needed as usize];
        let written = ExpandEnvironmentStringsW(PCWSTR(src.as_ptr()), Some(&mut buf));
        if written == 0 {
            return s.to_string();
        }
        String::from_utf16_lossy(&buf[..(written as usize).saturating_sub(1)])
    }
}

fn windows_dir() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
}

/// Normalizes the odd path prefixes that services and drivers use.
fn normalize(cmd: &str) -> String {
    let mut c = expand_env(cmd.trim()).trim().to_string();
    if let Some(rest) = c.strip_prefix(r"\??\") {
        c = rest.to_string();
    }
    let lower = c.to_ascii_lowercase();
    if lower.starts_with(r"\systemroot\") {
        c = format!("{}{}", windows_dir().display(), &c[r"\systemroot".len()..]);
    } else if lower.starts_with(r"system32\") || lower.starts_with(r"syswow64\") {
        c = format!("{}\\{}", windows_dir().display(), c);
    }
    c
}

fn exists_file(p: &Path) -> bool {
    p.is_file()
}

/// Finds an executable by name in the usual Windows search locations.
fn locate(exe: &str) -> Option<PathBuf> {
    // Winlogon's Userinit is stored with a trailing comma.
    let exe = exe.trim().trim_matches('"').trim_end_matches(',');
    if exe.is_empty() {
        return None;
    }
    let p = PathBuf::from(exe);
    let with_ext = |p: &Path| -> Option<PathBuf> {
        if exists_file(p) {
            return Some(p.to_path_buf());
        }
        if p.extension().is_none() {
            let e = p.with_extension("exe");
            if exists_file(&e) {
                return Some(e);
            }
        }
        None
    };
    if p.is_absolute() || exe.contains('\\') || exe.contains('/') {
        // Return the path even if the file is gone: a missing target is a useful finding.
        return with_ext(&p).or(Some(p));
    }
    let win = windows_dir();
    let mut dirs = vec![win.join("System32"), win.clone(), win.join("System32").join("wbem")];
    if let Ok(path) = std::env::var("PATH") {
        dirs.extend(path.split(';').filter(|s| !s.is_empty()).map(PathBuf::from));
    }
    dirs.iter().find_map(|d| with_ext(&d.join(exe)))
}

/// Splits a command line into (executable, arguments), coping with unquoted paths containing spaces.
pub fn split_command(cmd: &str) -> (String, String) {
    let cmd = normalize(cmd);
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
        return (rest.to_string(), String::new());
    }
    let tokens: Vec<&str> = cmd.split(' ').collect();
    for i in 1..=tokens.len() {
        let candidate = tokens[..i].join(" ");
        let p = Path::new(&candidate);
        if exists_file(p) || (p.extension().is_none() && exists_file(&p.with_extension("exe"))) {
            return (candidate, tokens[i..].join(" ").trim().to_string());
        }
    }
    let first = tokens.first().copied().unwrap_or_default().to_string();
    let rest = tokens.get(1..).map(|t| t.join(" ")).unwrap_or_default();
    (first, rest.trim().to_string())
}

/// Resolves the file that actually runs for a command line.
/// For `rundll32 foo.dll,Entry` this is the DLL, since that's what does the work.
pub fn resolve_target(cmd: &str) -> Option<PathBuf> {
    let (exe, args) = split_command(cmd);
    let path = locate(&exe)?;
    let fname = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if fname == "rundll32.exe" || fname == "rundll32" {
        // Skip switches such as `/d` before the DLL name.
        let args = args.split(' ').skip_while(|t| t.starts_with('/') || t.is_empty()).collect::<Vec<_>>().join(" ");
        let dll = args.split(',').next().unwrap_or("").trim().trim_matches('"');
        if !dll.is_empty() {
            if let Some(d) = locate(&normalize(dll)) {
                return Some(d);
            }
        }
    }
    Some(path)
}

/// Version-resource strings of a PE file.
#[derive(Clone, Debug, Default)]
pub struct VersionStrings {
    pub company: Option<String>,
    pub description: Option<String>,
    pub product: Option<String>,
}

pub fn version_strings(path: &Path) -> VersionStrings {
    let mut out = VersionStrings::default();
    let w = wide(&path.to_string_lossy());
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
        if size == 0 {
            return out;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, data.as_mut_ptr() as *mut _).is_err() {
            return out;
        }

        let query = |sub: &str| -> Option<(*const u8, u32)> {
            let sub_w = wide(sub);
            let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut len = 0u32;
            let ok = VerQueryValueW(data.as_ptr() as *const _, PCWSTR(sub_w.as_ptr()), &mut ptr, &mut len);
            (ok.as_bool() && !ptr.is_null() && len > 0).then_some((ptr as *const u8, len))
        };

        // Prefer the translations the file declares, then fall back to common English code pages.
        let mut langs: Vec<String> = Vec::new();
        if let Some((ptr, len)) = query(r"\VarFileInfo\Translation") {
            let pairs = std::slice::from_raw_parts(ptr as *const u16, (len / 2) as usize);
            for pair in pairs.chunks_exact(2) {
                langs.push(format!("{:04x}{:04x}", pair[0], pair[1]));
            }
        }
        langs.extend(["040904b0", "040904e4", "04090000"].map(String::from));

        let read = |field: &str| -> Option<String> {
            langs.iter().find_map(|lang| {
                let (ptr, len) = query(&format!(r"\StringFileInfo\{lang}\{field}"))?;
                let chars = std::slice::from_raw_parts(ptr as *const u16, len as usize);
                let s = String::from_utf16_lossy(chars);
                let s = s.trim_end_matches('\0').trim().to_string();
                (!s.is_empty()).then_some(s)
            })
        };
        out.company = read("CompanyName");
        out.description = read("FileDescription");
        out.product = read("ProductName");
    }
    out
}

/// Resolves a `.lnk` shortcut into (target path, arguments). Requires COM on this thread.
pub fn resolve_shortcut(path: &Path) -> Option<(String, String)> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let w = wide(&path.to_string_lossy());
        file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut target = [0u16; 1024];
        link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32).ok()?;
        let mut args = [0u16; 2048];
        let _ = link.GetArguments(&mut args);
        let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
        let target = expand_env(&s(&target));
        (!target.is_empty()).then(|| (target, s(&args)))
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_quoted_commands() {
        let (exe, args) = split_command(r#""C:\Program Files\App\app.exe" --minimized"#);
        assert_eq!(exe, r"C:\Program Files\App\app.exe");
        assert_eq!(args, "--minimized");
    }

    #[test]
    fn resolves_unquoted_system_paths() {
        let (exe, args) = split_command(r"%SystemRoot%\system32\svchost.exe -k netsvcs -p");
        assert!(exe.to_ascii_lowercase().ends_with(r"system32\svchost.exe"));
        assert_eq!(args, "-k netsvcs -p");
    }

    #[test]
    fn rundll32_targets_the_dll() {
        let t = resolve_target(r"rundll32.exe shell32.dll,Control_RunDLL").unwrap();
        assert!(t.to_string_lossy().to_ascii_lowercase().ends_with("shell32.dll"));
        let t = resolve_target(r"%windir%\system32\rundll32.exe /d shell32.dll,Foo").unwrap();
        assert!(t.to_string_lossy().to_ascii_lowercase().ends_with("shell32.dll"));
        let t = resolve_target(r"C:\Windows\system32\userinit.exe,").unwrap();
        assert!(t.is_file());
    }

    #[test]
    fn reads_version_info() {
        let v = version_strings(&windows_dir().join(r"System32\notepad.exe"));
        assert!(v.company.unwrap_or_default().contains("Microsoft"));
    }
}

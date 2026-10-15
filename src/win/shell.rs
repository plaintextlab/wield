use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use super::wide;

/// Opens Explorer with `path` selected (or opens the folder itself).
pub fn reveal_in_explorer(path: &Path) {
    let target = path.to_string_lossy();
    let _ = if path.is_dir() {
        Command::new("explorer.exe").raw_arg(format!("\"{target}\"")).spawn()
    } else {
        Command::new("explorer.exe").raw_arg(format!("/select,\"{target}\"")).spawn()
    };
}

/// Opens a document, URL or program with its default handler.
pub fn open(target: &str) {
    let t = wide(target);
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn search_web(query: &str) {
    open(&format!("https://duckduckgo.com/?q={}", url_encode(query)));
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Shows the Windows "Open" dialog filtered to programs. Blocks until closed; call from a worker thread.
pub fn pick_program() -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW};
    let filter: Vec<u16> = "Programs (*.exe)\0*.exe\0All files (*.*)\0*.*\0\0".encode_utf16().collect();
    let title = wide("Choose a program to block");
    let mut file = vec![0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    unsafe {
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return None;
        }
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    (len > 0).then(|| std::path::PathBuf::from(String::from_utf16_lossy(&file[..len])))
}

/// Multi-select "Open" dialog for any files. Blocks; call from a worker thread.
pub fn pick_files(title: &str) -> Vec<std::path::PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW,
    };
    let filter: Vec<u16> = "All files (*.*)\0*.*\0\0".encode_utf16().collect();
    let title = wide(title);
    let mut buf = vec![0u16; 64 * 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR | OFN_ALLOWMULTISELECT | OFN_EXPLORER,
        ..Default::default()
    };
    if unsafe { !GetOpenFileNameW(&mut ofn).as_bool() } {
        return Vec::new();
    }
    // Single file: one full path. Several: directory, then file names, null-separated.
    let parts: Vec<String> = buf.split(|&c| c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [one] => vec![one.into()],
        [dir, names @ ..] => names.iter().map(|n| std::path::Path::new(dir).join(n)).collect(),
    }
}

/// Folder picker (IFileOpenDialog). Blocks; call from a worker thread with COM initialized.
pub fn pick_folder(title: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let t = wide(title);
        let _ = dlg.SetTitle(PCWSTR(t.as_ptr()));
        dlg.Show(None).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(std::path::PathBuf::from)
    }
}

/// Moves a file or folder to the Recycle Bin.
pub fn recycle(path: &Path) -> Result<(), String> {
    use windows::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};
    // pFrom must be double-null terminated.
    let mut from: Vec<u16> = path.to_string_lossy().encode_utf16().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    match unsafe { SHFileOperationW(&mut op) } {
        0 if !op.fAnyOperationsAborted.as_bool() => Ok(()),
        0 => Err("The operation was cancelled.".into()),
        e => Err(format!("Could not move to the Recycle Bin (error {e}).")),
    }
}

const MENU_KEYS: [&str; 2] = [r"Software\Classes\*\shell\WieldScan", r"Software\Classes\Directory\shell\WieldScan"];

/// Adds or removes "Scan with Wield" in Explorer's right-click menu (current user only).
pub fn set_context_menu(enabled: bool) -> Result<(), String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for k in MENU_KEYS {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?.display().to_string();
            let (key, _) = hkcu.create_subkey(k).map_err(|e| e.to_string())?;
            key.set_value("", &"Scan with Wield".to_string()).map_err(|e| e.to_string())?;
            key.set_value("Icon", &format!("\"{exe}\",0")).map_err(|e| e.to_string())?;
            let (cmd, _) = key.create_subkey("command").map_err(|e| e.to_string())?;
            cmd.set_value("", &format!("\"{exe}\" --scan \"%1\"")).map_err(|e| e.to_string())?;
        } else {
            match hkcu.delete_subkey_all(k) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn context_menu_enabled() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!(r"{}\command", MENU_KEYS[0])).is_ok()
}

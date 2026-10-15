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

fn url_encode(s: &str) -> String {
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

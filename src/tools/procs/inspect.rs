//! Deep, on-demand details for one process: open files, loaded modules, memory and I/O
//! breakdown, integrity level, architecture, and who is locking a file.

use std::path::PathBuf;

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel};
use windows::Win32::Storage::FileSystem::{FILE_NAME_NORMALIZED, FILE_TYPE_DISK, GetFileType, GetFinalPathNameByHandleW};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::RestartManager::*;
use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessIoCounters, IO_COUNTERS, IsWow64Process2, OpenProcessToken, PROCESS_DUP_HANDLE,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::core::{PCWSTR, PWSTR};

use super::snapshot::{Owned, open};
use crate::win::wide;

#[derive(Clone, Debug, Default)]
pub struct Memory {
    pub working_set: u64,
    pub peak_working_set: u64,
    pub private: u64,
    pub peak_commit: u64,
    pub page_faults: u32,
    pub paged_pool: u64,
    pub nonpaged_pool: u64,
}

pub fn memory(pid: u32) -> Option<Memory> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut m = PROCESS_MEMORY_COUNTERS_EX { cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe { GetProcessMemoryInfo(h.0, &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb) }.ok()?;
    Some(Memory {
        working_set: m.WorkingSetSize as u64,
        peak_working_set: m.PeakWorkingSetSize as u64,
        private: m.PrivateUsage as u64,
        peak_commit: m.PeakPagefileUsage as u64,
        page_faults: m.PageFaultCount,
        paged_pool: m.QuotaPagedPoolUsage as u64,
        nonpaged_pool: m.QuotaNonPagedPoolUsage as u64,
    })
}

pub fn io(pid: u32) -> Option<IO_COUNTERS> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut c = IO_COUNTERS::default();
    unsafe { GetProcessIoCounters(h.0, &mut c) }.ok()?;
    Some(c)
}

pub fn integrity(pid: u32) -> Option<&'static str> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(h.0, TOKEN_QUERY, &mut token).ok()?;
        let t = Owned(token);
        let mut len = 0u32;
        let _ = GetTokenInformation(t.0, TokenIntegrityLevel, None, 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        GetTokenInformation(t.0, TokenIntegrityLevel, Some(buf.as_mut_ptr() as *mut _), len, &mut len).ok()?;
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let count = *GetSidSubAuthorityCount(label.Label.Sid);
        let rid = *GetSidSubAuthority(label.Label.Sid, u32::from(count) - 1);
        Some(match rid {
            0..=0x0FFF => "Untrusted",
            0x1000..=0x1FFF => "Low (sandboxed)",
            0x2000..=0x2FFF => "Medium",
            0x3000..=0x3FFF => "High (administrator)",
            _ => "System",
        })
    }
}

pub fn architecture(pid: u32) -> Option<&'static str> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let (mut proc_m, mut native) = (IMAGE_FILE_MACHINE::default(), IMAGE_FILE_MACHINE::default());
    unsafe { IsWow64Process2(h.0, &mut proc_m, Some(&mut native)) }.ok()?;
    let m = if proc_m.0 == 0 { native.0 } else { proc_m.0 };
    Some(match (proc_m.0, m) {
        (0, 0x8664) => "64-bit (x64)",
        (0, 0xAA64) => "64-bit (ARM64)",
        (_, 0x014C) => "32-bit (x86)",
        (_, 0x01C4) => "32-bit (ARM)",
        _ => "Unknown",
    })
}

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    pub path: PathBuf,
    pub size: u32,
}

pub fn modules(pid: u32) -> Result<Vec<Module>, String> {
    unsafe {
        // ERROR_BAD_LENGTH means the module list changed mid-snapshot (DLLs loading); retry as documented.
        let mut attempt = 0;
        let snap = loop {
            match CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) {
                Ok(h) => break h,
                Err(e) if e.code().0 as u32 == 0x80070018 && attempt < 10 => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) if e.code().0 as u32 == 0x80070005 => return Err("Access denied. Run as administrator to see this process's modules.".into()),
                Err(e) => return Err(e.message()),
            }
        };
        let snap = Owned(snap);
        let mut e = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut out = Vec::new();
        let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
        if Module32FirstW(snap.0, &mut e).is_ok() {
            loop {
                out.push(Module { name: s(&e.szModule), path: PathBuf::from(s(&e.szExePath)), size: e.modBaseSize });
                if Module32NextW(snap.0, &mut e).is_err() {
                    break;
                }
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(out)
    }
}

// ------------------------------------------------------------------ open files

#[repr(C)]
#[derive(Clone, Copy)]
struct HandleEntry {
    object: usize,
    pid: usize,
    handle: usize,
    access: u32,
    backtrace: u16,
    type_index: u16,
    attributes: u32,
    reserved: u32,
}

const SYSTEM_EXTENDED_HANDLE_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(64);

fn all_handles() -> Option<Vec<HandleEntry>> {
    let mut size = 4u32 << 20;
    for _ in 0..8 {
        let mut buf = vec![0u64; size as usize / 8];
        let mut needed = 0u32;
        let st = unsafe { NtQuerySystemInformation(SYSTEM_EXTENDED_HANDLE_INFORMATION, buf.as_mut_ptr() as *mut _, size, &mut needed) };
        if st.0 == 0xC0000004u32 as i32 {
            size = needed.max(size * 2);
            continue;
        }
        if st.is_err() {
            return None;
        }
        let count = buf[0] as usize;
        let entries = unsafe { std::slice::from_raw_parts(buf.as_ptr().add(2) as *const HandleEntry, count) };
        return Some(entries.to_vec());
    }
    None
}

/// Files and folders a process currently has open.
/// Uses the system handle table; only disk files are named, so pipes can't hang the query.
pub fn open_files(pid: u32) -> Result<Vec<String>, String> {
    // Learn the "File" object type index from a handle we own.
    let probe = std::fs::File::open(std::env::current_exe().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    use std::os::windows::io::AsRawHandle;
    let probe_handle = probe.as_raw_handle() as usize;
    let me = std::process::id() as usize;
    let handles = all_handles().ok_or("Could not read the system handle table.")?;
    let file_type = handles.iter().find(|h| h.pid == me && h.handle == probe_handle).map(|h| h.type_index).ok_or("Could not identify file handles.")?;
    drop(probe);

    let src = unsafe { windows::Win32::System::Threading::OpenProcess(PROCESS_DUP_HANDLE, false, pid) }
        .map(Owned)
        .map_err(|_| "Access denied. Run as administrator to see this process's open files.".to_string())?;
    let mut out = Vec::new();
    for h in handles.iter().filter(|h| h.pid == pid as usize && h.type_index == file_type).take(20_000) {
        unsafe {
            let mut dup = HANDLE::default();
            if DuplicateHandle(src.0, HANDLE(h.handle as *mut _), GetCurrentProcess(), &mut dup, 0, false, DUPLICATE_SAME_ACCESS).is_err() {
                continue;
            }
            let d = Owned(dup);
            if GetFileType(d.0) != FILE_TYPE_DISK {
                continue;
            }
            let mut buf = [0u16; 1024];
            let n = GetFinalPathNameByHandleW(d.0, &mut buf, FILE_NAME_NORMALIZED) as usize;
            if n > 0 && n < buf.len() {
                let p = String::from_utf16_lossy(&buf[..n]);
                out.push(p.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).or_else(|| p.strip_prefix(r"\\?\").map(String::from)).unwrap_or(p));
            }
        }
    }
    out.sort_by_key(|p| p.to_lowercase());
    out.dedup();
    Ok(out)
}

// ------------------------------------------------------------------ who locks a file

#[derive(Clone, Debug)]
pub struct Locker {
    pub pid: u32,
    pub app: String,
    pub service: String,
}

/// Asks the Restart Manager which processes are using a file (or files inside a folder).
pub fn who_locks(paths: &[PathBuf]) -> Result<Vec<Locker>, String> {
    let mut files: Vec<String> = Vec::new();
    for p in paths {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) {
                files.extend(rd.flatten().filter(|e| e.path().is_file()).take(2000).map(|e| e.path().display().to_string()));
            }
        } else {
            files.push(p.display().to_string());
        }
    }
    if files.is_empty() {
        return Ok(Vec::new());
    }
    unsafe {
        let mut session = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        let rc = RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr()));
        if rc.0 != 0 {
            return Err(format!("Restart Manager unavailable ({}).", rc.0));
        }
        let wides: Vec<Vec<u16>> = files.iter().map(|f| wide(f)).collect();
        let ptrs: Vec<PCWSTR> = wides.iter().map(|w| PCWSTR(w.as_ptr())).collect();
        let rc = RmRegisterResources(session, Some(&ptrs), None, None);
        let result = if rc.0 != 0 {
            Err(format!("Could not check these files ({}).", rc.0))
        } else {
            let (mut needed, mut count, mut reasons) = (0u32, 0u32, 0u32);
            let _ = RmGetList(session, &mut needed, &mut count, None, &mut reasons);
            let mut infos = vec![RM_PROCESS_INFO::default(); needed as usize + 4];
            count = infos.len() as u32;
            let rc = RmGetList(session, &mut needed, &mut count, Some(infos.as_mut_ptr()), &mut reasons);
            if rc.0 == 0 {
                let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
                Ok(infos[..count as usize]
                    .iter()
                    .map(|i| Locker { pid: i.Process.dwProcessId, app: s(&i.strAppName), service: s(&i.strServiceShortName) })
                    .collect())
            } else {
                Err(format!("Restart Manager query failed ({}).", rc.0))
            }
        };
        let _ = RmEndSession(session);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspects_own_process() {
        let me = std::process::id();
        assert!(memory(me).unwrap().working_set > 0);
        assert!(io(me).is_some());
        assert_eq!(architecture(me), Some("64-bit (x64)"));
        assert!(integrity(me).is_some());
        let m = modules(me).unwrap();
        assert!(m.iter().any(|m| m.name.eq_ignore_ascii_case("ntdll.dll")));
    }

    #[test]
    fn finds_our_open_file_and_its_locker() {
        let path = std::env::temp_dir().join("wield-open-file-test.txt");
        std::fs::write(&path, b"x").unwrap();
        let f = std::fs::File::open(&path).unwrap();
        let files = open_files(std::process::id()).unwrap();
        assert!(files.iter().any(|p| p.eq_ignore_ascii_case(&path.display().to_string())), "{files:?}");
        let lockers = who_locks(std::slice::from_ref(&path)).unwrap();
        assert!(lockers.iter().any(|l| l.pid == std::process::id()));
        drop(f);
        let _ = std::fs::remove_file(path);
    }
}

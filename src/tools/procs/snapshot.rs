//! Live process list built from one `NtQuerySystemInformation(SystemProcessInformation)` call.
//!
//! That single call returns every process with its threads, handles, memory, CPU times and I/O
//! counters, including protected system processes that can't be opened. Extra details that do
//! need a process handle (path, command line, user, elevation…) are cached per process.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, HWND, LPARAM, UNICODE_STRING};
use windows::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, SID_NAME_USE, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessInformation, GetSystemTimes, IsProcessCritical, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    PROCESS_QUERY_LIMITED_INFORMATION, ProcessPowerThrottling, QueryFullProcessImageNameW,
};
use windows::Win32::System::WindowsProgramming::{SYSTEM_PROCESS_INFORMATION, SYSTEM_THREAD_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindow, GetWindowTextW, GetWindowThreadProcessId, GW_OWNER, IsWindowVisible};
use windows::core::{BOOL, PWSTR};

use crate::tools::startup::services;

/// Closes a process/token handle on drop.
pub struct Owned(pub HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

pub fn open(pid: u32, access: windows::Win32::System::Threading::PROCESS_ACCESS_RIGHTS) -> Option<Owned> {
    unsafe { OpenProcess(access, false, pid).ok().map(Owned) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    App,
    Background,
    Windows,
}

/// Details that need a process handle; looked up once per process instance.
#[derive(Clone, Debug, Default)]
pub struct Static {
    pub exe: Option<PathBuf>,
    pub cmdline: Option<String>,
    pub user: Option<String>,
    pub elevated: bool,
    pub critical: bool,
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub threads: u32,
    pub handles: u32,
    pub session: u32,
    pub working_set: u64,
    pub private_ws: u64,
    /// Private committed memory ("Memory" in Task Manager's Details tab).
    pub private_bytes: u64,
    pub create_time: u64,
    /// User + kernel CPU time in 100 ns units.
    pub cpu_time: u64,
    pub io_read: u64,
    pub io_write: u64,
    pub suspended: bool,
    // Computed between samples.
    pub cpu_pct: f32,
    pub read_rate: f64,
    pub write_rate: f64,
    // Enrichment.
    pub info: Static,
    pub window_title: Option<String>,
    pub services: Vec<(String, String)>,
    pub priority_class: u32,
    pub efficiency: bool,
    pub kind: Kind,
}

impl Proc {
    pub fn key(&self) -> (u32, u64) {
        (self.pid, self.create_time)
    }
    pub fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.name,
            self.pid,
            self.info.exe.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            self.window_title.as_deref().unwrap_or(""),
            self.services.iter().map(|s| format!("{} {}", s.0, s.1)).collect::<Vec<_>>().join(" "),
            self.info.user.as_deref().unwrap_or("")
        )
        .to_lowercase()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Totals {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub commit_used: u64,
    pub commit_limit: u64,
    pub read_rate: f64,
    pub write_rate: f64,
    pub processes: usize,
    pub threads: u64,
    pub handles: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub procs: Vec<Proc>,
    pub totals: Totals,
}

fn ustr(u: &UNICODE_STRING) -> String {
    if u.Buffer.is_null() || u.Length == 0 {
        return String::new();
    }
    unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(u.Buffer.0, (u.Length / 2) as usize)) }
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Parses the full SystemProcessInformation list.
fn query_processes() -> Vec<Proc> {
    let mut size = 1u32 << 20;
    let mut buf: Vec<u64>;
    loop {
        buf = vec![0u64; size as usize / 8];
        let mut needed = 0u32;
        let st = unsafe { NtQuerySystemInformation(SystemProcessInformation, buf.as_mut_ptr() as *mut _, size, &mut needed) };
        if st.0 == 0xC0000004u32 as i32 {
            // STATUS_INFO_LENGTH_MISMATCH: grow and retry (the list changes constantly).
            size = needed.max(size) + (256 << 10);
            continue;
        }
        if st.is_err() {
            return Vec::new();
        }
        break;
    }
    let mut out = Vec::new();
    let base = buf.as_ptr() as *const u8;
    let mut offset = 0usize;
    loop {
        let p = unsafe { &*(base.add(offset) as *const SYSTEM_PROCESS_INFORMATION) };
        // Reserved1 holds: WorkingSetPrivateSize, HardFaultCount, NumberOfThreadsHighWatermark,
        // CycleTime, CreateTime, UserTime, KernelTime.
        let r1 = &p.Reserved1;
        let private_ws = read_u64(r1, 0);
        let create_time = read_u64(r1, 24);
        let user_time = read_u64(r1, 32);
        let kernel_time = read_u64(r1, 40);
        // Reserved7 holds the I/O counters: read/write/other operation counts, then transfer counts.
        let r7 = &p.Reserved7;
        let pid = p.UniqueProcessId.0 as usize as u32;
        let threads = unsafe {
            std::slice::from_raw_parts(
                (p as *const SYSTEM_PROCESS_INFORMATION).add(1) as *const SYSTEM_THREAD_INFORMATION,
                p.NumberOfThreads as usize,
            )
        };
        // Thread state 5 = Waiting, wait reason 5 = Suspended.
        let suspended = !threads.is_empty() && threads.iter().all(|t| t.ThreadState == 5 && t.WaitReason == 5);
        let name = match pid {
            0 => "System Idle Process".to_string(),
            _ => ustr(&p.ImageName),
        };
        out.push(Proc {
            pid,
            ppid: p.Reserved2 as usize as u32,
            name,
            threads: p.NumberOfThreads,
            handles: p.HandleCount,
            session: p.SessionId,
            working_set: p.WorkingSetSize as u64,
            private_ws,
            private_bytes: p.PagefileUsage as u64,
            create_time,
            cpu_time: user_time + kernel_time,
            io_read: r7[3] as u64,
            io_write: r7[4] as u64,
            suspended,
            cpu_pct: 0.0,
            read_rate: 0.0,
            write_rate: 0.0,
            info: Static::default(),
            window_title: None,
            services: Vec::new(),
            priority_class: 0,
            efficiency: false,
            kind: Kind::Background,
        });
        if p.NextEntryOffset == 0 {
            break;
        }
        offset += p.NextEntryOffset as usize;
    }
    out
}

fn token_user(token: HANDLE) -> Option<String> {
    unsafe {
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len).ok()?;
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let (mut nl, mut dl) = (name.len() as u32, domain.len() as u32);
        let mut use_ = SID_NAME_USE::default();
        LookupAccountSidW(None, tu.User.Sid, Some(PWSTR(name.as_mut_ptr())), &mut nl, Some(PWSTR(domain.as_mut_ptr())), &mut dl, &mut use_).ok()?;
        let n = String::from_utf16_lossy(&name[..nl as usize]);
        let d = String::from_utf16_lossy(&domain[..dl as usize]);
        Some(if d.is_empty() { n } else { format!("{d}\\{n}") })
    }
}

/// Opens the process once and reads everything that needs a handle.
pub fn read_static(pid: u32) -> Static {
    let mut s = Static::default();
    let Some(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return s };
    unsafe {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok() {
            s.exe = Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
        }
        let mut need = 0u32;
        let _ = NtQueryInformationProcess(h.0, ProcessCommandLineInformation, std::ptr::null_mut(), 0, &mut need);
        if need > 0 && need < 1 << 20 {
            let mut cbuf = vec![0u64; (need as usize).div_ceil(8)];
            if NtQueryInformationProcess(h.0, ProcessCommandLineInformation, cbuf.as_mut_ptr() as *mut _, need, &mut need).is_ok() {
                let us = &*(cbuf.as_ptr() as *const UNICODE_STRING);
                let c = ustr(us);
                if !c.is_empty() {
                    s.cmdline = Some(c);
                }
            }
        }
        let mut crit = BOOL(0);
        s.critical = IsProcessCritical(h.0, &mut crit).is_ok() && crit.as_bool();
        let mut token = HANDLE::default();
        if OpenProcessToken(h.0, TOKEN_QUERY, &mut token).is_ok() {
            let t = Owned(token);
            s.user = token_user(t.0);
            let mut el = TOKEN_ELEVATION::default();
            let mut l = 0u32;
            if GetTokenInformation(t.0, TokenElevation, Some(&mut el as *mut _ as *mut _), size_of::<TOKEN_ELEVATION>() as u32, &mut l).is_ok() {
                s.elevated = el.TokenIsElevated != 0;
            }
        }
    }
    s
}

/// (priority class, efficiency mode) — these can change at any time.
pub fn read_dynamic(pid: u32) -> (u32, bool) {
    let Some(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return (0, false) };
    unsafe {
        let prio = GetPriorityClass(h.0);
        let mut st = PROCESS_POWER_THROTTLING_STATE { Version: 1, ..Default::default() };
        let eff = GetProcessInformation(h.0, ProcessPowerThrottling, &mut st as *mut _ as *mut _, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32).is_ok()
            && st.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0
            && st.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0;
        (prio, eff)
    }
}

/// pid -> title of its main visible window.
fn window_titles() -> HashMap<u32, String> {
    unsafe extern "system" fn cb(hwnd: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
                return BOOL(1);
            }
            let mut buf = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let map = &mut *(lp.0 as *mut HashMap<u32, String>);
                map.entry(pid).or_insert_with(|| String::from_utf16_lossy(&buf[..n as usize]));
            }
            BOOL(1)
        }
    }
    let mut map: HashMap<u32, String> = HashMap::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut map as *mut _ as isize));
    }
    // Explorer's desktop/taskbar windows don't make it an "app" in the Task Manager sense.
    map.retain(|_, t| t != "Program Manager");
    map
}

/// Windows processes we may not be allowed to open (so their path is unknown).
fn is_system_name(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "registry", "memory compression", "secure system", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe",
        "lsass.exe", "lsaiso.exe", "dwm.exe", "fontdrvhost.exe", "svchost.exe", "msmpeng.exe", "nissrv.exe", "audiodg.exe",
        "securityhealthservice.exe", "sgrmbroker.exe", "spoolsv.exe", "wudfhost.exe", "dllhost.exe", "sihost.exe", "ctfmon.exe",
    ];
    NAMES.contains(&name.to_lowercase().as_str())
}

fn ft(f: FILETIME) -> u64 {
    (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)
}

fn windows_dir() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase()
}

/// Turns successive raw snapshots into rates and keeps per-process caches.
pub struct Sampler {
    statics: HashMap<(u32, u64), Static>,
    dynamics: HashMap<(u32, u64), (u32, bool)>,
    prev: HashMap<(u32, u64), (u64, u64, u64)>,
    prev_sys: Option<(u64, u64, u64, Instant)>,
    services: HashMap<u32, Vec<(String, String)>>,
    tick: u64,
    cpus: f32,
    windir: String,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            statics: HashMap::new(),
            dynamics: HashMap::new(),
            prev: HashMap::new(),
            prev_sys: None,
            services: HashMap::new(),
            tick: 0,
            cpus: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32,
            windir: windows_dir(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let mut procs = query_processes();

        // System-wide CPU from GetSystemTimes deltas.
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        let mut totals = Totals::default();
        unsafe {
            let _ = GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user));
        }
        let (idle, kernel, user) = (ft(idle), ft(kernel), ft(user));
        let wall = self.prev_sys.map(|(_, _, _, t)| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        if let Some((pi, pk, pu, _)) = self.prev_sys {
            let busy = (kernel - pk + user - pu).saturating_sub(idle - pi);
            let all = (kernel - pk + user - pu).max(1);
            totals.cpu_pct = (busy as f64 / all as f64 * 100.0) as f32;
        }
        self.prev_sys = Some((idle, kernel, user, now));

        if self.tick % 10 == 0 {
            self.services = services::running_by_pid();
        }
        let titles = window_titles();
        let refresh_dynamic = self.tick % 3 == 0;
        let mut live = HashSet::new();
        for p in &mut procs {
            let key = p.key();
            live.insert(key);
            if let Some((cpu, rd, wr)) = self.prev.insert(key, (p.cpu_time, p.io_read, p.io_write)) {
                if wall > 0.0 {
                    p.cpu_pct = ((p.cpu_time.saturating_sub(cpu)) as f64 / (wall * 1e7) / self.cpus as f64 * 100.0) as f32;
                    p.read_rate = p.io_read.saturating_sub(rd) as f64 / wall;
                    p.write_rate = p.io_write.saturating_sub(wr) as f64 / wall;
                }
            }
            if p.pid == 0 {
                // The idle process "uses" all free CPU; Task Manager hides that confusion too.
                p.cpu_pct = 0.0;
            }
            p.info = self.statics.entry(key).or_insert_with(|| if p.pid > 4 { read_static(p.pid) } else { Static::default() }).clone();
            if p.pid > 4 && (refresh_dynamic || !self.dynamics.contains_key(&key)) {
                self.dynamics.insert(key, read_dynamic(p.pid));
            }
            let (prio, eff) = self.dynamics.get(&key).copied().unwrap_or((0, false));
            p.priority_class = prio;
            p.efficiency = eff;
            p.window_title = titles.get(&p.pid).cloned();
            p.services = self.services.get(&p.pid).cloned().unwrap_or_default();
            let in_windows = p.info.exe.as_ref().is_some_and(|e| e.to_string_lossy().to_lowercase().starts_with(&self.windir));
            p.kind = if p.window_title.is_some() && !(in_windows && p.name.eq_ignore_ascii_case("explorer.exe") && p.session == 0) {
                Kind::App
            } else if p.pid <= 4 || in_windows || p.info.critical || p.info.exe.is_none() && (p.session == 0 || is_system_name(&p.name)) {
                Kind::Windows
            } else {
                Kind::Background
            };
            totals.read_rate += p.read_rate;
            totals.write_rate += p.write_rate;
            totals.threads += u64::from(p.threads);
            totals.handles += u64::from(p.handles);
        }
        self.prev.retain(|k, _| live.contains(k));
        self.statics.retain(|k, _| live.contains(k));
        self.dynamics.retain(|k, _| live.contains(k));
        totals.processes = procs.len();

        let mut ms = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() {
            totals.mem_total = ms.ullTotalPhys;
            totals.mem_used = ms.ullTotalPhys - ms.ullAvailPhys;
            totals.commit_limit = ms.ullTotalPageFile;
            totals.commit_used = ms.ullTotalPageFile - ms.ullAvailPageFile;
        }
        self.tick += 1;
        Snapshot { procs, totals }
    }
}

/// Converts a FILETIME-style timestamp (100 ns since 1601) to Unix seconds.
pub fn filetime_to_unix(t: u64) -> u64 {
    (t / 10_000_000).saturating_sub(11_644_473_600)
}

pub fn priority_label(class: u32) -> &'static str {
    match class {
        0x40 => "Low",
        0x4000 => "Below normal",
        0x20 => "Normal",
        0x8000 => "Above normal",
        0x80 => "High",
        0x100 => "Realtime",
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_contains_this_process() {
        let mut s = Sampler::new();
        let _ = s.sample();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let snap = s.sample();
        let me = std::process::id();
        let p = snap.procs.iter().find(|p| p.pid == me).expect("own process listed");
        assert!(p.threads >= 1);
        assert!(p.working_set > 0);
        assert!(p.info.exe.is_some());
        assert!(p.info.cmdline.is_some());
        assert!(p.info.user.is_some());
        assert!(snap.procs.iter().any(|p| p.pid == 4), "System process present");
        assert!(snap.totals.mem_total > 0);
        // Parent of the test process is cargo or a shell, which must exist in the list.
        assert!(p.ppid != 0);
    }
}

#[cfg(test)]
mod live {
    /// Prints the busiest processes. `cargo test live_procs -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_procs() {
        let mut s = super::Sampler::new();
        let _ = s.sample();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let mut snap = s.sample();
        snap.procs.sort_by(|a, b| b.private_bytes.cmp(&a.private_bytes));
        for p in snap.procs.iter().take(12) {
            println!(
                "{:>6} {:>6} {:<28.28} cpu {:>5.1}% priv {:>6} MB ws {:>6} MB thr {:>4} hnd {:>6} rd {:>9.0} B/s started {} {:?} {:?} {} {}",
                p.pid, p.ppid, p.name, p.cpu_pct, p.private_bytes >> 20, p.working_set >> 20, p.threads, p.handles, p.read_rate,
                crate::tools::startup::journal::format_time(super::filetime_to_unix(p.create_time)), p.kind, p.info.user, super::priority_label(p.priority_class), if p.suspended {"SUSPENDED"} else {""}
            );
        }
        println!("cpu {:.1}% mem {}/{} MB procs {} threads {} handles {}", snap.totals.cpu_pct, snap.totals.mem_used >> 20, snap.totals.mem_total >> 20, snap.totals.processes, snap.totals.threads, snap.totals.handles);
    }
}

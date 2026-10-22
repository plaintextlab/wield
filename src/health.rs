//! PC health for the Home dashboard: live CPU/memory, temperatures, disks, GPU, battery, and
//! recommendations for keeping the PC fast, each pointing at the tool or setting that fixes it.
//!
//! A background thread keeps a shared snapshot fresh: CPU and memory every 2 seconds, sensors
//! every 30 seconds, and the heavier checks (startup apps, devices, Defender, temp files) every
//! 10 minutes. Nothing here goes online.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::System::Performance::{
    PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

use crate::tools::backup::sys::{self, DriveKind, Volume};
use crate::win::fileinfo::{expand_env, format_bytes};

#[derive(Clone, Debug, Default)]
pub struct Gpu {
    pub name: String,
    pub temp: Option<f32>,
    pub load: Option<f32>,
    pub mem_used_mb: Option<u64>,
    pub mem_total_mb: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Critical,
    Warn,
    Tip,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open a Wield tool (by its --tool name), optionally on a tab.
    Tool(&'static str, Option<&'static str>),
    /// Open a Windows Settings page or other URI.
    Open(&'static str),
    CleanTemp,
}

#[derive(Clone, Debug)]
pub struct Rec {
    pub id: &'static str,
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub action: Option<(String, Action)>,
}

#[derive(Clone, Debug, Default)]
pub struct Health {
    pub cpu: f32,
    pub cpu_name: String,
    pub cores: usize,
    pub cpu_hist: VecDeque<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub mem_hist: VecDeque<f32>,
    pub uptime: u64,
    pub processes: usize,
    /// Highest plausible thermal-zone reading, °C.
    pub temp: Option<f32>,
    /// Recent temperature readings (every 30 s).
    pub temp_hist: VecDeque<f32>,
    pub gpus: Vec<Gpu>,
    pub disks: Vec<Volume>,
    /// (percent, charging) on PCs with a battery.
    pub battery: Option<(u8, bool)>,
    pub temp_files: Option<u64>,
    pub recs: Vec<Rec>,
    /// True once the heavier checks have run at least once.
    pub checked: bool,
}

impl Health {
    /// Many laptops only refresh their ACPI thermal zones occasionally; a value that never moves
    /// across several readings is stale, not live.
    pub fn temp_stale(&self) -> bool {
        self.temp_hist.len() >= 6 && self.temp_hist.iter().all(|t| (t - self.temp_hist[0]).abs() < 0.05)
    }
    pub fn mem_free(&self) -> u64 {
        self.mem_total.saturating_sub(self.mem_used)
    }
    fn avg(h: &VecDeque<f32>, n: usize) -> f32 {
        let v: Vec<f32> = h.iter().rev().take(n).copied().collect();
        if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
    }
}

// ------------------------------------------------------------------ sensors

/// Thermal zones via Windows' performance counters (no administrator rights needed). Readings are
/// in tenths of a kelvin; zones that report nonsense (below 5 °C or above 125 °C) are ignored.
struct Thermal {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl Thermal {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, w!(r"\Thermal Zone Information(*)\High Precision Temperature"), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            Some(Thermal { query, counter })
        }
    }

    fn read(&self) -> Option<f32> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None) != PDH_MORE_DATA {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return None;
            }
            (0..count as usize)
                .map(|i| &*items.add(i))
                .filter(|it| it.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA)
                .map(|it| (it.FmtValue.Anonymous.doubleValue / 10.0 - 273.15) as f32)
                .filter(|c| (5.0..=125.0).contains(c))
                .reduce(f32::max)
        }
    }
}

impl Drop for Thermal {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

/// NVIDIA GPUs through the driver's own nvidia-smi (installed with every NVIDIA driver).
fn nvidia() -> Vec<Gpu> {
    let exe = PathBuf::from(expand_env(r"%SystemRoot%\System32\nvidia-smi.exe"));
    if !exe.is_file() {
        return Vec::new();
    }
    let Ok((0, out)) = sys::run(&exe.display().to_string(), &["--query-gpu=name,temperature.gpu,utilization.gpu,memory.used,memory.total", "--format=csv,noheader,nounits"]) else {
        return Vec::new();
    };
    out.lines().filter_map(|l| parse_nvidia_line(l)).collect()
}

pub fn parse_nvidia_line(l: &str) -> Option<Gpu> {
    let p: Vec<&str> = l.split(',').map(str::trim).collect();
    if p.len() < 5 || p[0].is_empty() {
        return None;
    }
    let num = |s: &str| s.trim_end_matches(" %").trim_end_matches(" MiB").parse::<f32>().ok();
    Some(Gpu { name: p[0].to_string(), temp: num(p[1]), load: num(p[2]), mem_used_mb: num(p[3]).map(|v| v as u64), mem_total_mb: num(p[4]).map(|v| v as u64) })
}

// ------------------------------------------------------------------ temporary files

fn temp_dir() -> PathBuf {
    std::env::temp_dir()
}

fn dir_size(dir: &Path, depth: u32) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() && !t.is_symlink() && depth < 32 => dir_size(&e.path(), depth + 1),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Deletes files in your Temp folder that are more than a day old. Files in use are skipped.
/// Returns (bytes freed, files deleted, files skipped).
pub fn clean_temp() -> (u64, usize, usize) {
    clean_dir(&temp_dir())
}

/// Deletes files older than a day under `dir`, then any folders left empty.
fn clean_dir(root: &Path) -> (u64, usize, usize) {
    fn walk(dir: &Path, cutoff: SystemTime, acc: &mut (u64, usize, usize), depth: u32) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let p = e.path();
            if t.is_dir() && !t.is_symlink() {
                if depth < 32 {
                    walk(&p, cutoff, acc, depth + 1);
                }
                // Only removes folders that are now empty.
                let _ = std::fs::remove_dir(&p);
            } else if t.is_file() {
                let Ok(m) = e.metadata() else { continue };
                if m.modified().map(|t| t > cutoff).unwrap_or(true) {
                    continue;
                }
                match std::fs::remove_file(&p) {
                    Ok(()) => {
                        acc.0 += m.len();
                        acc.1 += 1;
                    }
                    Err(_) => acc.2 += 1,
                }
            }
        }
    }
    let mut acc = (0, 0, 0);
    let cutoff = SystemTime::now() - Duration::from_secs(24 * 3600);
    walk(root, cutoff, &mut acc, 0);
    acc
}

// ------------------------------------------------------------------ the heavier checks

#[derive(Clone, Debug, Default)]
struct Slow {
    startup_high: Option<usize>,
    startup_enabled: Option<usize>,
    problem_devices: Option<usize>,
    realtime_off: bool,
    reboot_pending: bool,
    power_saver: bool,
    temp_files: Option<u64>,
    no_backup: bool,
}

fn reboot_pending() -> bool {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let hk = RegKey::predef(HKEY_LOCAL_MACHINE);
    hk.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired").is_ok()
        || hk.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending").is_ok()
}

fn slow_checks(ctx: &eframe::egui::Context) -> Slow {
    let mut s = Slow { reboot_pending: reboot_pending(), temp_files: Some(dir_size(&temp_dir(), 0)), ..Default::default() };
    // Startup apps (the same scan Startup Manager uses).
    let rx = crate::tools::startup::scan::spawn(ctx.clone());
    while let Ok(m) = rx.recv() {
        if let crate::tools::startup::scan::ScanMsg::Done(items, _) = m {
            // Apps only (Run entries and Startup folders); services and tasks are Startup Manager's business.
            use crate::tools::startup::model::{Impact, SourceKind};
            let apps: Vec<_> = items.iter().filter(|i| i.enabled && matches!(i.source, SourceKind::RegistryRun | SourceKind::StartupFolder)).collect();
            s.startup_high = Some(apps.iter().filter(|i| i.impact == Impact::High).count());
            s.startup_enabled = Some(apps.len());
            break;
        }
    }
    s.problem_devices = crate::tools::drivers::devices::scan().ok().map(|d| d.iter().filter(|x| x.problem != 0 && x.problem != 22).count());
    s.realtime_off = crate::tools::malware::defender::status().ok().is_some_and(|st| st.realtime == Some(false) && st.running_mode.as_deref() == Some("Normal"));
    s.power_saver = sys::run("powercfg.exe", &["/getactivescheme"]).is_ok_and(|(_, o)| o.contains("a1841308-3541-4fab-bc81-f71556f20b4a"));
    s.no_backup = crate::tools::backup::image::history().iter().all(|h| !h.ok);
    s
}

/// Turns measurements into recommendations, most urgent first.
fn recommend(h: &Health, s: &Slow) -> Vec<Rec> {
    let mut v = Vec::new();
    let tool = |label: &str, t: &'static str, tab: Option<&'static str>| Some((label.to_string(), Action::Tool(t, tab)));
    let sysd = sys::system_drive();
    for d in &h.disks {
        let pct = if d.total > 0 { d.free as f64 / d.total as f64 * 100.0 } else { 100.0 };
        let (level, why) = if pct < 5.0 || d.free < 5 << 30 {
            (Level::Critical, "almost full")
        } else if pct < 12.0 || d.free < 15 << 30 {
            (Level::Warn, "running low")
        } else {
            continue;
        };
        let system = d.letter == sysd;
        v.push(Rec {
            id: "disk",
            level,
            title: format!("Drive {} is {why}: {} free", d.letter, format_bytes(d.free)),
            detail: if system {
                "Windows slows down and updates fail when the system drive fills up. Clear temporary files, uninstall games and apps you don't use, or see what's taking the space.".into()
            } else {
                "Move or delete large files you don't need.".into()
            },
            action: Some(("Open storage settings".into(), Action::Open("ms-settings:storagesense"))),
        });
    }
    if let Some(t) = s.temp_files.filter(|t| *t > 1 << 30) {
        v.push(Rec {
            id: "temp",
            level: if t > 10 << 30 { Level::Warn } else { Level::Tip },
            title: format!("{} of temporary files", format_bytes(t)),
            detail: "Leftovers from installers and apps in your Temp folder. Wield removes the ones older than a day; files in use are skipped.".into(),
            action: Some(("Clean temporary files".into(), Action::CleanTemp)),
        });
    }
    if h.mem_total > 0 && Health::avg(&h.mem_hist, 15) > 88.0 {
        v.push(Rec {
            id: "memory",
            level: Level::Warn,
            title: format!("Memory is nearly full ({} free)", format_bytes(h.mem_free())),
            detail: "When memory runs out Windows swaps to disk and everything slows down. See which apps use the most and close what you don't need.".into(),
            action: tool("See what uses memory", "processes", None),
        });
    }
    if h.cpu_hist.len() >= 20 && Health::avg(&h.cpu_hist, 30) > 85.0 {
        v.push(Rec {
            id: "cpu",
            level: Level::Warn,
            title: format!("The processor has been busy ({:.0}% for the last minute)", Health::avg(&h.cpu_hist, 30)),
            detail: "Something is working hard in the background. Find it in Process Explorer, sorted by CPU.".into(),
            action: tool("Find what's using the CPU", "processes", None),
        });
    }
    // Laptop processors briefly reach the mid-90s under load by design; only a sustained high reading matters.
    if !h.temp_stale() && h.temp_hist.len() >= 4 && h.temp_hist.iter().rev().take(4).all(|t| *t >= 95.0) && Health::avg(&h.cpu_hist, 60) >= 30.0 {
        let t = h.temp.unwrap_or(95.0);
        v.push(Rec {
            id: "temp-cpu",
            level: Level::Warn,
            title: format!("Running hot for several minutes: {t:.0} °C"),
            detail: "High temperatures slow the processor down to protect it. Make sure the vents aren't blocked, clean dust from the fans, and use laptops on a hard surface.".into(),
            action: tool("See what's working hard", "processes", None),
        });
    }
    for g in &h.gpus {
        if let Some(t) = g.temp.filter(|t| *t >= 90.0) {
            v.push(Rec {
                id: "temp-gpu",
                level: Level::Warn,
                title: format!("{} is hot: {t:.0} °C", g.name),
                detail: "Check the graphics card's fans and airflow; a frame-rate limit in games also lowers heat.".into(),
                action: None,
            });
        }
    }
    if s.reboot_pending {
        v.push(Rec {
            id: "reboot",
            level: Level::Warn,
            title: "Restart to finish installing updates".into(),
            detail: "Windows has installed updates that only take effect after a restart.".into(),
            action: Some(("Open Windows Update".into(), Action::Open("ms-settings:windowsupdate"))),
        });
    } else if h.uptime > 7 * 86_400 {
        v.push(Rec {
            id: "uptime",
            level: Level::Tip,
            title: format!("Not restarted for {} days", h.uptime / 86_400),
            detail: "A restart clears leaked memory and stuck background tasks. \"Shut down\" with Fast Startup on doesn't count; use Restart.".into(),
            action: None,
        });
    }
    if let (Some(high), Some(en)) = (s.startup_high, s.startup_enabled) {
        if high >= 3 || en >= 15 {
            v.push(Rec {
                id: "startup",
                level: Level::Tip,
                title: format!("{en} apps start with Windows ({high} with high impact)"),
                detail: "Every startup app delays sign-in and keeps using memory. Turn off the ones you don't need right away; Startup Manager shows which are safe.".into(),
                action: tool("Open Startup Manager", "startup", None),
            });
        }
    }
    if let Some(n) = s.problem_devices.filter(|n| *n > 0) {
        v.push(Rec {
            id: "devices",
            level: Level::Warn,
            title: format!("{n} device(s) have a driver problem"),
            detail: "Missing or failing drivers cause crashes, poor performance and hardware that doesn't work.".into(),
            action: tool("Open Driver Manager", "drivers", Some("problems")),
        });
    }
    if s.realtime_off {
        v.push(Rec {
            id: "defender",
            level: Level::Critical,
            title: "Real-time malware protection is off".into(),
            detail: "Microsoft Defender isn't checking files as they're opened. Turn it on in Windows Security unless another antivirus protects this PC.".into(),
            action: Some(("Open Windows Security".into(), Action::Open("windowsdefender://threat"))),
        });
    }
    if s.power_saver && h.battery.is_none_or(|(_, charging)| charging) {
        v.push(Rec {
            id: "power",
            level: Level::Tip,
            title: "The Power saver plan is on".into(),
            detail: "It slows the processor to save energy. On mains power, Balanced is faster and still efficient.".into(),
            action: Some(("Open power options".into(), Action::Open("ms-settings:powersleep"))),
        });
    }
    if s.no_backup {
        v.push(Rec {
            id: "backup",
            level: Level::Tip,
            title: "No backup made with Wield yet".into(),
            detail: "A system image or folder backup on another drive is the only protection against a dead disk or ransomware.".into(),
            action: tool("Set up a backup", "backup", None),
        });
    }
    v.sort_by_key(|r| r.level);
    v
}

// ------------------------------------------------------------------ the monitor

pub struct Monitor {
    state: Arc<Mutex<Health>>,
    started: bool,
    rescan: Arc<Mutex<bool>>,
}

impl Monitor {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(Health::default())), started: false, rescan: Arc::new(Mutex::new(false)) }
    }

    pub fn snapshot(&self) -> Health {
        self.state.lock().map(|h| h.clone()).unwrap_or_default()
    }

    /// Runs the heavier checks again soon (after cleaning temp files, for example).
    pub fn recheck(&self) {
        if let Ok(mut r) = self.rescan.lock() {
            *r = true;
        }
    }

    /// Starts the background thread the first time the dashboard is shown.
    pub fn start(&mut self, ctx: &eframe::egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        let (state, rescan, ctx) = (self.state.clone(), self.rescan.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut s = sysinfo::System::new();
            s.refresh_cpu_all();
            let thermal = Thermal::new();
            let cpu_name = {
                s.refresh_cpu_all();
                s.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default()
            };
            let cores = s.cpus().len();
            let mut last_sensors = Instant::now() - Duration::from_secs(3600);
            let mut last_slow: Option<Instant> = None;
            let mut slow = Slow::default();
            let started = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(2000));
                s.refresh_cpu_usage();
                s.refresh_memory();
                let cpu = s.global_cpu_usage();
                let (used, total) = (s.used_memory(), s.total_memory());
                let mut sensors = None;
                if last_sensors.elapsed() > Duration::from_secs(30) {
                    last_sensors = Instant::now();
                    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                    let st = sys_battery();
                    sensors = Some((
                        thermal.as_ref().and_then(Thermal::read),
                        nvidia(),
                        sys::volumes().into_iter().filter(|v| v.kind == DriveKind::Fixed && v.total > 0).collect::<Vec<_>>(),
                        st,
                        s.processes().len(),
                    ));
                }
                let force = rescan.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or(false);
                // First heavy check a few seconds in, so startup stays quick.
                let due = match last_slow {
                    None => started.elapsed() > Duration::from_secs(4),
                    Some(t) => t.elapsed() > Duration::from_secs(600),
                };
                if due || force {
                    slow = slow_checks(&ctx);
                    last_slow = Some(Instant::now());
                }
                if let Ok(mut h) = state.lock() {
                    h.cpu = cpu;
                    h.cpu_name = cpu_name.clone();
                    h.cores = cores;
                    h.mem_used = used;
                    h.mem_total = total;
                    h.uptime = sysinfo::System::uptime();
                    h.cpu_hist.push_back(cpu);
                    h.mem_hist.push_back(if total > 0 { used as f32 / total as f32 * 100.0 } else { 0.0 });
                    while h.cpu_hist.len() > 90 {
                        h.cpu_hist.pop_front();
                    }
                    while h.mem_hist.len() > 90 {
                        h.mem_hist.pop_front();
                    }
                    if let Some((t, g, d, b, n)) = sensors {
                        h.temp = t;
                        if let Some(t) = t {
                            h.temp_hist.push_back(t);
                            while h.temp_hist.len() > 20 {
                                h.temp_hist.pop_front();
                            }
                        }
                        h.gpus = g;
                        h.disks = d;
                        h.battery = b;
                        h.processes = n;
                    }
                    h.temp_files = slow.temp_files;
                    h.checked = last_slow.is_some();
                    let recs = recommend(&h, &slow);
                    h.recs = recs;
                }
            }
        });
    }
}

fn sys_battery() -> Option<(u8, bool)> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut st) }.ok()?;
    // 128 = no battery, 255 = unknown.
    if st.BatteryFlag & 128 != 0 || st.BatteryFlag == 255 || st.BatteryLifePercent > 100 {
        return None;
    }
    Some((st.BatteryLifePercent, st.ACLineStatus == 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nvidia_output() {
        let g = parse_nvidia_line("NVIDIA GeForce RTX 3070 Ti Laptop GPU, 52, 3, 512, 8192").unwrap();
        assert_eq!(g.temp, Some(52.0));
        assert_eq!(g.load, Some(3.0));
        assert_eq!(g.mem_total_mb, Some(8192));
        assert!(parse_nvidia_line("").is_none());
    }

    #[test]
    fn reads_thermal_zones_when_present() {
        // Most PCs expose at least one zone; VMs may not. Either way it must not crash or lie.
        if let Some(t) = Thermal::new().and_then(|t| t.read()) {
            assert!((5.0..=125.0).contains(&t), "{t}");
        }
    }

    #[test]
    fn recommends_for_a_full_disk_and_temp_files() {
        let h = Health {
            disks: vec![Volume { letter: sys::system_drive(), label: String::new(), fs: "NTFS".into(), kind: DriveKind::Fixed, total: 500 << 30, free: 3 << 30, disk: None }],
            ..Default::default()
        };
        let s = Slow { temp_files: Some(12 << 30), startup_high: Some(5), startup_enabled: Some(20), ..Default::default() };
        let r = recommend(&h, &s);
        assert_eq!(r[0].id, "disk");
        assert_eq!(r[0].level, Level::Critical);
        assert!(r.iter().any(|x| x.id == "temp" && x.action.as_ref().is_some_and(|a| a.1 == Action::CleanTemp)));
        assert!(r.iter().any(|x| x.id == "startup"));
        assert!(r.windows(2).all(|w| w[0].level <= w[1].level), "sorted by urgency");
    }

    #[test]
    fn ignores_a_stuck_sensor() {
        let mut h = Health { temp: Some(100.1), cpu_hist: std::iter::repeat_n(80.0, 60).collect(), ..Default::default() };
        h.temp_hist = std::iter::repeat_n(100.1, 8).collect();
        assert!(h.temp_stale());
        assert!(!recommend(&h, &Slow::default()).iter().any(|r| r.id == "temp-cpu"));
        h.temp_hist = (0..8).map(|i| 96.0 + i as f32 * 0.3).collect();
        assert!(!h.temp_stale());
        assert!(recommend(&h, &Slow::default()).iter().any(|r| r.id == "temp-cpu"));
    }

    #[test]
    fn healthy_pc_gets_no_warnings() {
        let h = Health { mem_total: 16 << 30, mem_used: 4 << 30, uptime: 3600, ..Default::default() };
        let r = recommend(&h, &Slow::default());
        assert!(r.is_empty(), "{r:?}");
    }

    /// Works on its own folder only; never on the real Temp folder.
    #[test]
    fn cleaning_only_touches_old_files() {
        let dir = std::env::temp_dir().join("wield-clean-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let fresh = dir.join("fresh.tmp");
        let old = dir.join("sub").join("old.tmp");
        std::fs::write(&fresh, b"new").unwrap();
        std::fs::write(&old, b"old file").unwrap();
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(3 * 86_400)).unwrap();
        drop(f);
        let (freed, deleted, _) = clean_dir(&dir);
        assert_eq!((freed, deleted), (8, 1));
        assert!(fresh.exists() && !old.exists(), "only the old file goes");
        assert!(!dir.join("sub").exists(), "emptied folders are removed");
        let _ = std::fs::remove_dir_all(dir);
    }
}

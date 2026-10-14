//! Collects every startup entry on a background thread, enriches and classifies it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::model::{Criticality, StartupItem};
use super::{classify, journal, registry, services, tasks};
use crate::win::ComGuard;
use crate::win::fileinfo::{resolve_target, version_strings};

pub enum ScanMsg {
    Progress(String),
    Done(Vec<StartupItem>, Vec<String>),
}

pub fn spawn(ctx: eframe::egui::Context) -> Receiver<ScanMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (items, warnings) = run(&tx, &ctx);
        let _ = tx.send(ScanMsg::Done(items, warnings));
        ctx.request_repaint();
    });
    rx
}

fn run(tx: &Sender<ScanMsg>, ctx: &eframe::egui::Context) -> (Vec<StartupItem>, Vec<String>) {
    let _com = ComGuard::new();
    let progress = |s: &str| {
        let _ = tx.send(ScanMsg::Progress(s.to_string()));
        ctx.request_repaint();
    };
    let mut items = Vec::new();
    let mut warnings = Vec::new();

    progress("Reading registry Run keys…");
    registry::collect_run_keys(&mut items);
    progress("Reading Startup folders…");
    registry::collect_startup_folders(&mut items);
    registry::collect_winlogon(&mut items);

    progress("Querying services…");
    let remembered = journal::Journal::load().services_changed_by_wield();
    let service_pids = match services::collect(&mut items, &remembered) {
        Ok(p) => p,
        Err(e) => {
            warnings.push(e);
            Vec::new()
        }
    };

    progress("Querying scheduled tasks…");
    match tasks::collect(&mut items) {
        Ok(0) => {}
        Ok(n) => warnings.push(format!(
            "{n} task folder(s) or task(s) could not be read. Run as administrator to see everything."
        )),
        Err(e) => warnings.push(e),
    }

    progress("Inspecting programs…");
    enrich(&mut items, &service_pids);

    progress("Rating criticality and impact…");
    for item in &mut items {
        classify::classify(item);
    }
    items.sort_by(|a, b| {
        b.enabled
            .cmp(&a.enabled)
            .then(b.impact.cmp(&a.impact))
            .then(a.criticality.cmp(&b.criticality))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    (items, warnings)
}

#[derive(Default, Clone)]
struct FileMeta {
    company: Option<String>,
    description: Option<String>,
    size: Option<u64>,
}

fn enrich(items: &mut [StartupItem], service_pids: &[(String, u32)]) {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_memory().with_exe(UpdateKind::OnlyIfNotSet),
    );
    // Sum memory per executable path and per process name (multi-process apps like Electron).
    let mut by_path: HashMap<String, u64> = HashMap::new();
    let mut by_name: HashMap<String, u64> = HashMap::new();
    for p in sys.processes().values() {
        if let Some(exe) = p.exe() {
            *by_path.entry(exe.to_string_lossy().to_lowercase()).or_default() += p.memory();
        }
        *by_name.entry(p.name().to_string_lossy().to_lowercase()).or_default() += p.memory();
    }
    let pid_mem: HashMap<&str, u64> = service_pids
        .iter()
        .filter_map(|(id, pid)| sys.process(Pid::from_u32(*pid)).map(|p| (id.as_str(), p.memory())))
        .collect();

    let mut cache: HashMap<PathBuf, FileMeta> = HashMap::new();
    for item in items.iter_mut() {
        if item.target.is_none() && !item.command.starts_with("COM handler") {
            item.target = resolve_target(&item.command);
        }
        let Some(target) = item.target.clone() else { continue };
        item.target_exists = target.is_file();
        let meta = cache
            .entry(target.clone())
            .or_insert_with(|| {
                if !target.is_file() {
                    return FileMeta::default();
                }
                let v = version_strings(&target);
                FileMeta { company: v.company, description: v.description, size: std::fs::metadata(&target).ok().map(|m| m.len()) }
            })
            .clone();
        item.publisher = meta.company;
        if item.description.is_none() {
            item.description = meta.description;
        }
        item.file_size = meta.size;

        let task_ms = item.location.to_lowercase().starts_with(r"\microsoft");
        item.is_microsoft = task_ms || item.publisher.as_deref().is_some_and(|p| p.to_lowercase().contains("microsoft"));

        item.memory = if let Some(m) = pid_mem.get(item.id.as_str()) {
            Some(*m)
        } else if item.service.as_ref().is_some_and(|s| s.shared_process) {
            None
        } else if let Some(name) = squirrel_process(&item.command) {
            by_name.get(&name).copied()
        } else {
            by_path.get(&target.to_string_lossy().to_lowercase()).copied()
        };
        if item.service.is_none() {
            if let Some(m) = item.memory.filter(|m| *m > 0) {
                item.facts.push(("Currently".into(), format!("Running ({} RAM)", crate::win::fileinfo::format_bytes(m))));
            }
        }
    }
}

/// Squirrel-installed apps (Discord, Teams classic, Slack…) start via `Update.exe --processStart App.exe`.
fn squirrel_process(command: &str) -> Option<String> {
    let lower = command.to_lowercase();
    let idx = lower.find("--processstart")?;
    let rest = lower[idx + "--processstart".len()..].trim_start();
    let name = rest.trim_start_matches('"').split(['"', ' ']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Counts used for the summary strip.
pub struct Summary {
    pub total: usize,
    pub enabled: usize,
    pub high_enabled: usize,
    pub safe_enabled: usize,
}

pub fn summarize(items: &[StartupItem]) -> Summary {
    Summary {
        total: items.len(),
        enabled: items.iter().filter(|i| i.enabled).count(),
        high_enabled: items.iter().filter(|i| i.enabled && i.impact == super::model::Impact::High).count(),
        safe_enabled: items.iter().filter(|i| i.enabled && i.can_toggle && i.criticality == Criticality::Optional).count(),
    }
}

#[cfg(test)]
mod tests {
    /// Scans the real machine. Run with `cargo test live_scan -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_scan() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let (items, warnings) = super::run(&tx, &eframe::egui::Context::default());
        for i in &items {
            println!(
                "{:<14} {:<7} {:<16} {:<5} {:<40.40} {:<28.28} {}",
                i.source.label(),
                i.impact.label(),
                i.criticality.label(),
                if i.enabled { "on" } else { "off" },
                i.name,
                i.publisher.as_deref().unwrap_or("-"),
                i.command.chars().take(70).collect::<String>()
            );
        }
        println!("{} items, warnings: {warnings:?}", items.len());
        assert!(!items.is_empty());
    }

    #[test]
    fn squirrel() {
        assert_eq!(
            super::squirrel_process(r#""C:\Users\u\AppData\Local\Discord\Update.exe" --processStart Discord.exe"#).as_deref(),
            Some("discord.exe")
        );
    }
}

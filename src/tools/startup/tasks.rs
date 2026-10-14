//! Scheduled tasks that fire at boot or at sign-in, via the Task Scheduler 2.0 COM API.
//! All functions here need COM initialized on the calling thread.

use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

use super::model::{Locator, SourceKind, StartupItem};
use crate::win::fileinfo::expand_env;

fn connect() -> windows::core::Result<ITaskService> {
    unsafe {
        let service: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        service.Connect(&empty, &empty, &empty, &empty)?;
        Ok(service)
    }
}

fn bstr(get: impl FnOnce(*mut BSTR) -> windows::core::Result<()>) -> String {
    let mut b = BSTR::new();
    match get(&mut b) {
        Ok(()) => b.to_string(),
        Err(_) => String::new(),
    }
}

/// Human description of the boot/logon triggers, e.g. "At log on (delay PT30S)".
struct TriggerInfo {
    labels: Vec<String>,
    delayed: bool,
}

fn startup_triggers(def: &ITaskDefinition) -> Option<TriggerInfo> {
    unsafe {
        let triggers = def.Triggers().ok()?;
        let mut count = 0;
        triggers.Count(&mut count).ok()?;
        let mut info = TriggerInfo { labels: Vec::new(), delayed: false };
        for i in 1..=count {
            let Ok(t) = triggers.get_Item(i) else { continue };
            let mut kind = TASK_TRIGGER_TYPE2::default();
            if t.Type(&mut kind).is_err() {
                continue;
            }
            let (label, delay) = match kind {
                TASK_TRIGGER_BOOT => ("At system startup", t.cast::<IBootTrigger>().ok().map(|b| bstr(|p| b.Delay(p)))),
                TASK_TRIGGER_LOGON => {
                    let logon = t.cast::<ILogonTrigger>().ok();
                    let user = logon.as_ref().map(|l| bstr(|p| l.UserId(p))).unwrap_or_default();
                    let label = if user.is_empty() { "At log on of any user" } else { "At log on" };
                    (label, logon.map(|l| bstr(|p| l.Delay(p))))
                }
                _ => continue,
            };
            let delay = delay.unwrap_or_default();
            if delay.is_empty() {
                info.labels.push(label.to_string());
            } else {
                info.delayed = true;
                info.labels.push(format!("{label} (delay {})", pretty_duration(&delay)));
            }
        }
        (!info.labels.is_empty()).then_some(info)
    }
}

/// "PT1M30S" -> "1m 30s"
fn pretty_duration(iso: &str) -> String {
    let s = iso.trim_start_matches('P').replace('T', "");
    let mut out = String::new();
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            out.push_str(&format!("{num}{} ", c.to_ascii_lowercase()));
            num.clear();
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() { iso.to_string() } else { out }
}

fn first_action(def: &ITaskDefinition) -> (String, Vec<String>) {
    let mut commands = Vec::new();
    unsafe {
        let Ok(actions) = def.Actions() else { return (String::new(), commands) };
        let mut count = 0;
        let _ = actions.Count(&mut count);
        for i in 1..=count {
            let Ok(a) = actions.get_Item(i) else { continue };
            let mut kind = TASK_ACTION_TYPE::default();
            let _ = a.Type(&mut kind);
            if kind == TASK_ACTION_EXEC {
                if let Ok(exec) = a.cast::<IExecAction>() {
                    let path = expand_env(&bstr(|p| exec.Path(p)));
                    let args = bstr(|p| exec.Arguments(p));
                    let quoted = if path.contains(' ') && !path.starts_with('"') { format!("\"{path}\"") } else { path };
                    commands.push(if args.is_empty() { quoted } else { format!("{quoted} {args}") });
                }
            } else if let Ok(com) = a.cast::<IComHandlerAction>() {
                commands.push(format!("COM handler {}", bstr(|p| com.ClassId(p))));
            }
        }
    }
    (commands.first().cloned().unwrap_or_default(), commands)
}

fn walk(folder: &ITaskFolder, out: &mut Vec<StartupItem>, errors: &mut usize) {
    unsafe {
        if let Ok(tasks) = folder.GetTasks(TASK_ENUM_HIDDEN.0) {
            let count = tasks.Count().unwrap_or(0);
            for i in 1..=count {
                let Ok(task) = tasks.get_Item(&VARIANT::from(i)) else {
                    *errors += 1;
                    continue;
                };
                if let Some(item) = to_item(&task) {
                    out.push(item);
                }
            }
        } else {
            *errors += 1;
        }
        if let Ok(subs) = folder.GetFolders(0) {
            let count = subs.Count().unwrap_or(0);
            for i in 1..=count {
                if let Ok(sub) = subs.get_Item(&VARIANT::from(i)) {
                    walk(&sub, out, errors);
                }
            }
        }
    }
}

fn to_item(task: &IRegisteredTask) -> Option<StartupItem> {
    unsafe {
        let def = task.Definition().ok()?;
        let triggers = startup_triggers(&def)?;
        let path = task.Path().ok()?.to_string();
        let name = task.Name().map(|n| n.to_string()).unwrap_or_else(|_| path.clone());
        let enabled = task.Enabled().map(|b| b.as_bool()).unwrap_or(true);
        let (command, all_commands) = first_action(&def);
        let folder = path.rsplit_once('\\').map(|(f, _)| if f.is_empty() { "\\" } else { f }).unwrap_or("\\").to_string();

        let mut item = StartupItem::new(
            format!("task|{}", path.to_lowercase()),
            name,
            command,
            SourceKind::ScheduledTask,
            folder,
            Locator::Task { path: path.clone() },
        );
        item.enabled = enabled;
        item.status_text = if enabled { "Enabled" } else { "Disabled" }.into();
        // Only tasks that run as the current user can be changed without elevation.
        let user_id = def.Principal().map(|p| bstr(|b| p.UserId(b))).unwrap_or_default().to_lowercase();
        let me = std::env::var("USERNAME").unwrap_or_default().to_lowercase();
        item.needs_admin = path.to_lowercase().starts_with(r"\microsoft\") || me.is_empty() || !user_id.ends_with(&me);
        if let Ok(reg) = def.RegistrationInfo() {
            let author = bstr(|p| reg.Author(p));
            let desc = bstr(|p| reg.Description(p));
            if !author.is_empty() && !author.starts_with("$(") {
                item.facts.push(("Author".into(), author));
            }
            if !desc.is_empty() && !desc.starts_with("$(") {
                item.description = Some(desc);
            }
        }
        item.facts.push(("Task path".into(), path));
        item.facts.push(("Triggers".into(), triggers.labels.join("; ")));
        if triggers.delayed {
            item.facts.push(("Delayed".into(), "yes".into()));
        }
        if all_commands.len() > 1 {
            item.facts.push(("All actions".into(), all_commands.join("\n")));
        }
        Some(item)
    }
}

/// Returns tasks with boot/logon triggers and the number of folders/tasks that could not be read.
pub fn collect(out: &mut Vec<StartupItem>) -> Result<usize, String> {
    let service = connect().map_err(|e| format!("Cannot connect to Task Scheduler: {}", e.message()))?;
    let root = unsafe { service.GetFolder(&BSTR::from("\\")) }.map_err(|e| e.message())?;
    let mut errors = 0;
    walk(&root, out, &mut errors);
    Ok(errors)
}

pub fn is_enabled(path: &str) -> Result<bool, String> {
    let service = connect().map_err(|e| e.message())?;
    unsafe {
        let root = service.GetFolder(&BSTR::from("\\")).map_err(|e| e.message())?;
        let task = root.GetTask(&BSTR::from(path)).map_err(|e| e.message())?;
        task.Enabled().map(|b| b.as_bool()).map_err(|e| e.message())
    }
}

pub fn set_enabled(path: &str, enabled: bool) -> Result<(), String> {
    let service = connect().map_err(|e| e.message())?;
    unsafe {
        let root = service.GetFolder(&BSTR::from("\\")).map_err(|e| e.message())?;
        let task = root.GetTask(&BSTR::from(path)).map_err(|e| e.message())?;
        task.SetEnabled(VARIANT_BOOL::from(enabled)).map_err(|e| e.message())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations() {
        assert_eq!(super::pretty_duration("PT30S"), "30s");
        assert_eq!(super::pretty_duration("PT1M30S"), "1m 30s");
    }
}

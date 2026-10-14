//! Automatic-start Windows services, via the Service Control Manager.

use std::collections::HashSet;

use windows::Win32::Foundation::ERROR_MORE_DATA;
use windows::Win32::System::Services::*;
use windows::core::{BOOL, PCWSTR};
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

use super::model::{Locator, ServiceDetails, SourceKind, StartupItem};
use crate::win::fileinfo::expand_env;
use crate::win::{from_pwstr, wide};

/// Per-user service template/instance flag (SERVICE_USER_SERVICE).
const SERVICE_USER_SERVICE: u32 = 0x40;
const SERVICE_USERSERVICE_INSTANCE: u32 = 0x80;
const SERVICE_WIN32_SHARE_PROCESS: u32 = 0x20;

/// Closes an SC_HANDLE on drop.
struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_manager(access: u32) -> windows::core::Result<Handle> {
    unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access).map(Handle) }
}

fn open_service(scm: &Handle, name: &str, access: u32) -> windows::core::Result<Handle> {
    let n = wide(name);
    unsafe { OpenServiceW(scm.0, PCWSTR(n.as_ptr()), access).map(Handle) }
}

pub fn start_type_label(start: u32, delayed: bool) -> &'static str {
    match SERVICE_START_TYPE(start) {
        SERVICE_BOOT_START => "Boot",
        SERVICE_SYSTEM_START => "System",
        SERVICE_AUTO_START if delayed => "Automatic (delayed start)",
        SERVICE_AUTO_START => "Automatic",
        SERVICE_DEMAND_START => "Manual",
        SERVICE_DISABLED => "Disabled",
        _ => "Unknown",
    }
}

struct Config {
    service_type: u32,
    start_type: u32,
    binary: String,
    display: String,
    account: String,
}

fn query_config(svc: &Handle) -> Option<Config> {
    unsafe {
        let mut needed = 0u32;
        let _ = QueryServiceConfigW(svc.0, None, 0, &mut needed);
        if needed == 0 {
            return None;
        }
        // u64 buffer keeps the struct properly aligned.
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
        QueryServiceConfigW(svc.0, Some(cfg), needed, &mut needed).ok()?;
        let cfg = &*cfg;
        Some(Config {
            service_type: cfg.dwServiceType.0,
            start_type: cfg.dwStartType.0,
            binary: from_pwstr(cfg.lpBinaryPathName),
            display: from_pwstr(cfg.lpDisplayName),
            account: from_pwstr(cfg.lpServiceStartName),
        })
    }
}

fn query_config2(svc: &Handle, level: SERVICE_CONFIG) -> Option<Vec<u64>> {
    unsafe {
        let mut needed = 0u32;
        let _ = QueryServiceConfig2W(svc.0, level, None, &mut needed);
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        let bytes = std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, needed as usize);
        QueryServiceConfig2W(svc.0, level, Some(bytes), &mut needed).ok()?;
        Some(buf)
    }
}

fn query_delayed(svc: &Handle) -> bool {
    query_config2(svc, SERVICE_CONFIG_DELAYED_AUTO_START_INFO)
        .map(|b| unsafe { (*(b.as_ptr() as *const SERVICE_DELAYED_AUTO_START_INFO)).fDelayedAutostart.as_bool() })
        .unwrap_or(false)
}

fn query_description(svc: &Handle) -> Option<String> {
    let b = query_config2(svc, SERVICE_CONFIG_DESCRIPTION)?;
    let d = unsafe { from_pwstr((*(b.as_ptr() as *const SERVICE_DESCRIPTIONW)).lpDescription) };
    // Some descriptions are unresolved resource references like "@%SystemRoot%\...,-101".
    (!d.trim().is_empty() && !d.starts_with('@')).then_some(d)
}

/// For svchost-hosted services the real code lives in the ServiceDll.
fn service_dll(name: &str) -> Option<String> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(format!(r"SYSTEM\CurrentControlSet\Services\{name}\Parameters"), KEY_READ)
        .ok()?;
    key.get_value::<String, _>("ServiceDll").ok().map(|s| expand_env(&s))
}

struct EnumEntry {
    name: String,
    display: String,
    running: bool,
    pid: u32,
}

fn enumerate(scm: &Handle) -> Vec<EnumEntry> {
    let mut out = Vec::new();
    let mut resume = 0u32;
    let mut buf: Vec<u64> = vec![0; 32 * 1024];
    loop {
        let mut needed = 0u32;
        let mut count = 0u32;
        let bytes = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 8) };
        let res = unsafe {
            EnumServicesStatusExW(
                scm.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(bytes),
                &mut needed,
                &mut count,
                Some(&mut resume),
                PCWSTR::null(),
            )
        };
        let more = matches!(&res, Err(e) if e.code() == ERROR_MORE_DATA.to_hresult());
        if res.is_err() && !more {
            break;
        }
        let entries = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize) };
        for e in entries {
            out.push(EnumEntry {
                name: from_pwstr(e.lpServiceName),
                display: from_pwstr(e.lpDisplayName),
                running: e.ServiceStatusProcess.dwCurrentState == SERVICE_RUNNING,
                pid: e.ServiceStatusProcess.dwProcessId,
            });
        }
        if !more {
            break;
        }
        if count == 0 {
            // Buffer too small for even one entry.
            buf.resize((needed as usize).div_ceil(8) + 1024, 0);
        }
    }
    out
}

/// Collects services that start automatically, plus any that Wield previously turned off
/// (`remembered`) so they stay visible and can be switched back on.
/// Returns the process ids of running own-process services keyed by item id, for memory lookup.
pub fn collect(out: &mut Vec<StartupItem>, remembered: &HashSet<String>) -> Result<Vec<(String, u32)>, String> {
    let scm = open_manager(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)
        .map_err(|e| format!("Cannot open the Service Control Manager: {}", e.message()))?;
    let mut pids = Vec::new();
    for entry in enumerate(&scm) {
        let Ok(svc) = open_service(&scm, &entry.name, SERVICE_QUERY_CONFIG) else { continue };
        let Some(cfg) = query_config(&svc) else { continue };
        let auto = cfg.start_type == SERVICE_AUTO_START.0;
        if !auto && !remembered.contains(&entry.name.to_ascii_lowercase()) {
            continue;
        }
        let delayed = auto && query_delayed(&svc);
        let shared = cfg.service_type & SERVICE_WIN32_SHARE_PROCESS != 0;
        let is_user_service = cfg.service_type & (SERVICE_USER_SERVICE | SERVICE_USERSERVICE_INSTANCE) != 0;

        let display = if cfg.display.is_empty() { entry.display.clone() } else { cfg.display.clone() };
        let id = format!("svc|{}", entry.name.to_ascii_lowercase());
        let mut item = StartupItem::new(
            id.clone(),
            display,
            cfg.binary.clone(),
            SourceKind::Service,
            entry.name.clone(),
            Locator::Service { name: entry.name.clone() },
        );
        item.enabled = auto;
        item.status_text = start_type_label(cfg.start_type, delayed).to_string();
        item.needs_admin = true;
        item.description = query_description(&svc);
        if let Some(dll) = service_dll(&entry.name) {
            item.facts.push(("Service DLL".into(), dll.clone()));
            item.target = Some(dll.into());
        }
        item.facts.push(("Service name".into(), entry.name.clone()));
        item.facts.push(("Runs as".into(), if cfg.account.is_empty() { "LocalSystem".into() } else { cfg.account.clone() }));
        item.facts.push(("Currently".into(), if entry.running { "Running" } else { "Stopped" }.into()));
        if is_user_service {
            item = item.read_only(
                "Per-user service instance. Its start type is inherited from the template service of the same name without the _suffix.",
            );
            item.locator = Locator::ReadOnly;
        }
        item.service = Some(ServiceDetails {
            name: entry.name.clone(),
            start_type: cfg.start_type,
            delayed,
            shared_process: shared,
        });
        if entry.running && entry.pid != 0 && !shared {
            pids.push((id, entry.pid));
        }
        out.push(item);
    }
    Ok(pids)
}

/// Reads the current (start type, delayed) of a service.
pub fn current_start(name: &str) -> Result<(u32, bool), String> {
    let scm = open_manager(SC_MANAGER_CONNECT).map_err(|e| e.message())?;
    let svc = open_service(&scm, name, SERVICE_QUERY_CONFIG).map_err(|e| e.message())?;
    let cfg = query_config(&svc).ok_or("Cannot read service configuration")?;
    Ok((cfg.start_type, query_delayed(&svc)))
}

/// Changes a service's start type (and delayed-start flag for automatic services).
/// Takes effect at the next boot; a running service keeps running.
pub fn set_start(name: &str, start_type: u32, delayed: bool) -> Result<(), String> {
    let scm = open_manager(SC_MANAGER_CONNECT).map_err(|e| e.message())?;
    let svc = open_service(&scm, name, SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG).map_err(|e| e.message())?;
    unsafe {
        ChangeServiceConfigW(
            svc.0,
            ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
            SERVICE_START_TYPE(start_type),
            SERVICE_ERROR(SERVICE_NO_CHANGE),
            PCWSTR::null(),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
        .map_err(|e| e.message())?;
        if start_type == SERVICE_AUTO_START.0 {
            let info = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: BOOL::from(delayed) };
            ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, Some(&info as *const _ as *const _))
                .map_err(|e| e.message())?;
        }
    }
    Ok(())
}

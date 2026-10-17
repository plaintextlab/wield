//! Changing processes: end, end tree, suspend/resume, priority, Efficiency mode, affinity, restart.

use windows::Win32::Foundation::NTSTATUS;
use windows::Win32::System::Threading::*;

use super::snapshot::{Owned, Proc, open};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: windows::Win32::Foundation::HANDLE) -> NTSTATUS;
    fn NtResumeProcess(process: windows::Win32::Foundation::HANDLE) -> NTSTATUS;
}

fn denied(e: windows::core::Error, what: &str) -> String {
    if e.code().0 as u32 == 0x80070005 {
        format!("Access denied. {what} needs administrator rights (or the process is protected by Windows).")
    } else {
        e.message()
    }
}

fn handle(pid: u32, access: PROCESS_ACCESS_RIGHTS, what: &str) -> Result<Owned, String> {
    unsafe { OpenProcess(access, false, pid) }.map(Owned).map_err(|e| denied(e, what))
}

pub fn end(pid: u32) -> Result<(), String> {
    let h = handle(pid, PROCESS_TERMINATE, "Ending this process")?;
    unsafe { TerminateProcess(h.0, 1) }.map_err(|e| denied(e, "Ending this process"))
}

/// Ends a process and all its descendants, children first.
pub fn end_tree(pid: u32, all: &[Proc]) -> (usize, Vec<String>) {
    let mut order = Vec::new();
    fn collect(pid: u32, all: &[Proc], out: &mut Vec<u32>, depth: usize) {
        if depth > 64 {
            return;
        }
        let me = all.iter().find(|p| p.pid == pid).map(|p| p.create_time).unwrap_or(0);
        // A child must have started after its parent; this skips stale PID reuse.
        for c in all.iter().filter(|c| c.ppid == pid && c.pid != pid && c.create_time >= me) {
            collect(c.pid, all, out, depth + 1);
        }
        out.push(pid);
    }
    collect(pid, all, &mut order, 0);
    let mut errors = Vec::new();
    let mut ok = 0;
    for p in order {
        match end(p) {
            Ok(()) => ok += 1,
            Err(e) => errors.push(format!("PID {p}: {e}")),
        }
    }
    (ok, errors)
}

pub fn suspend(pid: u32) -> Result<(), String> {
    let h = handle(pid, PROCESS_SUSPEND_RESUME, "Suspending this process")?;
    let st = unsafe { NtSuspendProcess(h.0) };
    if st.is_ok() { Ok(()) } else { Err(format!("Could not suspend (NTSTATUS 0x{:08X}).", st.0)) }
}

pub fn resume(pid: u32) -> Result<(), String> {
    let h = handle(pid, PROCESS_SUSPEND_RESUME, "Resuming this process")?;
    let st = unsafe { NtResumeProcess(h.0) };
    if st.is_ok() { Ok(()) } else { Err(format!("Could not resume (NTSTATUS 0x{:08X}).", st.0)) }
}

pub const PRIORITIES: &[(u32, &str)] = &[
    (IDLE_PRIORITY_CLASS.0, "Low"),
    (BELOW_NORMAL_PRIORITY_CLASS.0, "Below normal"),
    (NORMAL_PRIORITY_CLASS.0, "Normal"),
    (ABOVE_NORMAL_PRIORITY_CLASS.0, "Above normal"),
    (HIGH_PRIORITY_CLASS.0, "High"),
];

pub fn set_priority(pid: u32, class: u32) -> Result<(), String> {
    let h = handle(pid, PROCESS_SET_INFORMATION, "Changing priority")?;
    unsafe { SetPriorityClass(h.0, PROCESS_CREATION_FLAGS(class)) }.map_err(|e| denied(e, "Changing priority"))
}

/// Efficiency mode (EcoQoS), like Task Manager: throttles the process and lowers its priority.
pub fn set_efficiency(pid: u32, on: bool) -> Result<(), String> {
    let h = handle(pid, PROCESS_SET_INFORMATION, "Efficiency mode")?;
    let st = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 },
    };
    unsafe {
        SetProcessInformation(h.0, ProcessPowerThrottling, &st as *const _ as *const _, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32)
            .map_err(|e| denied(e, "Efficiency mode"))?;
        let class = if on { IDLE_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
        SetPriorityClass(h.0, class).map_err(|e| denied(e, "Efficiency mode"))
    }
}

/// (process mask, system mask)
pub fn affinity(pid: u32) -> Option<(usize, usize)> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let (mut p, mut s) = (0usize, 0usize);
    unsafe { GetProcessAffinityMask(h.0, &mut p, &mut s) }.ok()?;
    Some((p, s))
}

pub fn set_affinity(pid: u32, mask: usize) -> Result<(), String> {
    if mask == 0 {
        return Err("Pick at least one CPU.".into());
    }
    let h = handle(pid, PROCESS_SET_INFORMATION, "Changing CPU affinity")?;
    unsafe { SetProcessAffinityMask(h.0, mask) }.map_err(|e| denied(e, "Changing CPU affinity"))
}

/// Ends the process and starts it again with the same program and arguments.
pub fn restart(p: &Proc) -> Result<(), String> {
    let exe = p.info.exe.clone().ok_or("Unknown program path.")?;
    let args = p.info.cmdline.as_deref().map(|c| crate::win::fileinfo::split_command(c).1).unwrap_or_default();
    end(p.pid)?;
    // Give the old instance a moment to release files and single-instance locks.
    std::thread::sleep(std::time::Duration::from_millis(600));
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(&exe);
    if !args.is_empty() {
        cmd.raw_arg(args);
    }
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Ended, but could not start it again: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_resume_and_end_a_child_process() {
        // A harmless long-running child we own.
        let mut child = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = child.id();
        suspend(pid).unwrap();
        resume(pid).unwrap();
        set_priority(pid, BELOW_NORMAL_PRIORITY_CLASS.0).unwrap();
        set_efficiency(pid, true).unwrap();
        // Efficiency mode lowers the priority to Low; reading the throttling flag back needs a newer build.
        assert_eq!(super::super::snapshot::read_dynamic(pid).0, IDLE_PRIORITY_CLASS.0);
        let (mask, sys) = affinity(pid).unwrap();
        assert!(mask != 0 && sys != 0);
        end(pid).unwrap();
        let _ = child.wait();
    }
}

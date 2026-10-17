//! Shared plumbing for the backup tool: running Windows' own tools with live output,
//! listing drives, power and BitLocker state.

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    GetVolumeInformationW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::core::PCWSTR;

use crate::win::wide;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a program, calling `on_line` for every line it prints (stdout and stderr). Returns the exit code.
pub fn run_streaming(program: &str, args: &[String], mut on_line: impl FnMut(&str)) -> Result<i32, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Could not start {program}: {e}"))?;
    let stderr = child.stderr.take();
    let err_thread = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(e) = stderr {
            for l in BufReader::new(e).split(b'\n').map_while(Result::ok) {
                lines.push(String::from_utf8_lossy(&l).trim_end().to_string());
            }
        }
        lines
    });
    if let Some(out) = child.stdout.take() {
        // Tools like wbadmin rewrite the progress line with \r; treat both as line ends.
        let mut buf = Vec::new();
        let mut reader = BufReader::new(out);
        let mut byte = [0u8; 1];
        while std::io::Read::read(&mut reader, &mut byte).map(|n| n > 0).unwrap_or(false) {
            if byte[0] == b'\n' || byte[0] == b'\r' {
                if !buf.is_empty() {
                    on_line(String::from_utf8_lossy(&buf).trim());
                    buf.clear();
                }
            } else {
                buf.push(byte[0]);
            }
        }
        if !buf.is_empty() {
            on_line(String::from_utf8_lossy(&buf).trim());
        }
    }
    for l in err_thread.join().unwrap_or_default() {
        if !l.is_empty() {
            on_line(&l);
        }
    }
    child.wait().map(|s| s.code().unwrap_or(-1)).map_err(|e| e.to_string())
}

/// Runs a program and returns (exit code, combined output).
pub fn run(program: &str, args: &[&str]) -> Result<(i32, String), String> {
    let out = Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| format!("Could not start {program}: {e}"))?;
    Ok((out.status.code().unwrap_or(-1), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
}

pub fn powershell(script: &str) -> Result<String, String> {
    let (code, out) = run("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", &format!("$ProgressPreference='SilentlyContinue'; {script}")])?;
    if code == 0 { Ok(out.trim().to_string()) } else { Err(first_error(&out)) }
}

pub fn first_error(out: &str) -> String {
    let l = out.to_lowercase();
    if l.contains("access is denied") || l.contains("elevated") || l.contains("administrator") || l.contains("0x80070005") {
        return "This needs administrator rights. Use \"Restart as administrator\".".into();
    }
    out.lines()
        .map(str::trim)
        .find(|l| l.to_lowercase().starts_with("error") || l.contains("Exception") || l.contains("failed"))
        .or_else(|| out.lines().map(str::trim).rfind(|l| !l.is_empty()))
        .unwrap_or("The command failed.")
        .to_string()
}

pub fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).trim_end_matches('\\').to_uppercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    Other,
}

impl DriveKind {
    pub fn label(&self) -> &'static str {
        match self {
            DriveKind::Fixed => "Internal / external disk",
            DriveKind::Removable => "Removable (USB stick)",
            DriveKind::Network => "Network drive",
            DriveKind::Optical => "Optical",
            DriveKind::Other => "Other",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Volume {
    /// "E:"
    pub letter: String,
    pub label: String,
    pub fs: String,
    pub kind: DriveKind,
    pub total: u64,
    pub free: u64,
    /// Physical disk number, when it can be read.
    pub disk: Option<u32>,
}

impl Volume {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
    pub fn display(&self) -> String {
        if self.label.is_empty() { format!("{} ({})", self.letter, self.kind.label()) } else { format!("{} {} ({})", self.letter, self.label, self.kind.label()) }
    }
}

fn disk_number(letter: &str) -> Option<u32> {
    let path = wide(&format!(r"\\.\{letter}"));
    unsafe {
        let h = CreateFileW(PCWSTR(path.as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None).ok()?;
        let mut n = STORAGE_DEVICE_NUMBER::default();
        let mut ret = 0u32;
        let ok = DeviceIoControl(h, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, 0, Some(&mut n as *mut _ as *mut _), size_of::<STORAGE_DEVICE_NUMBER>() as u32, Some(&mut ret), None).is_ok();
        let _ = CloseHandle(h);
        let _ = GENERIC_READ;
        ok.then_some(n.DeviceNumber)
    }
}

/// All mounted drive letters with size, free space and file system.
pub fn volumes() -> Vec<Volume> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = format!("{}:", (b'A' + i as u8) as char);
        let root = wide(&format!("{letter}\\"));
        unsafe {
            let kind = match GetDriveTypeW(PCWSTR(root.as_ptr())) {
                2 => DriveKind::Removable,
                3 => DriveKind::Fixed,
                4 => DriveKind::Network,
                5 => DriveKind::Optical,
                _ => DriveKind::Other,
            };
            let (mut free, mut total) = (0u64, 0u64);
            if GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free), Some(&mut total), None).is_err() {
                continue; // empty card reader, disconnected network drive…
            }
            let mut name = [0u16; 261];
            let mut fsname = [0u16; 32];
            let _ = GetVolumeInformationW(PCWSTR(root.as_ptr()), Some(&mut name), None, None, None, Some(&mut fsname));
            let s = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
            out.push(Volume {
                disk: if kind == DriveKind::Fixed || kind == DriveKind::Removable { disk_number(&letter) } else { None },
                letter,
                label: s(&name),
                fs: s(&fsname),
                kind,
                total,
                free,
            });
        }
    }
    out
}

/// Lowercase volume GUID ("{1234…}") of a drive letter.
pub fn volume_guid(letter: &str) -> Option<String> {
    let mp = wide(&format!("{letter}\\"));
    let mut buf = [0u16; 64];
    unsafe { windows::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW(PCWSTR(mp.as_ptr()), &mut buf) }.ok()?;
    let s = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]).to_lowercase();
    let start = s.find('{')?;
    let end = s[start..].find('}')? + start;
    Some(s[start..=end].to_string())
}

/// True when running on battery (no AC power).
pub fn on_battery() -> bool {
    let mut st = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut st) }.is_ok() && st.ACLineStatus == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitLocker {
    Off,
    On,
    /// Encryption or decryption in progress, or suspended.
    Changing,
    Unknown,
}

/// BitLocker state of a drive, readable without administrator rights (Explorer's shell property).
pub fn bitlocker(letter: &str) -> BitLocker {
    let script = format!("(New-Object -ComObject Shell.Application).NameSpace('{letter}').Self.ExtendedProperty('System.Volume.BitLockerProtection')");
    match powershell(&script).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(1) | Some(5) => BitLocker::On,
        Some(2) => BitLocker::Off,
        Some(3) | Some(4) | Some(6) => BitLocker::Changing,
        _ => BitLocker::Unknown,
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_system_drive() {
        let v = volumes();
        let sys = v.iter().find(|v| v.letter == system_drive()).expect("system drive listed");
        assert_eq!(sys.kind, DriveKind::Fixed);
        assert!(sys.total > sys.free && sys.free > 0);
        assert!(sys.fs.eq_ignore_ascii_case("NTFS"));
        assert!(sys.disk.is_some(), "disk number readable without admin");
    }

    #[test]
    fn streams_output_lines() {
        let mut lines = Vec::new();
        let code = run_streaming("cmd.exe", &["/C".into(), "echo one& echo two".into()], |l| lines.push(l.to_string())).unwrap();
        assert_eq!(code, 0);
        assert_eq!(lines, vec!["one", "two"]);
    }

    #[test]
    fn reads_bitlocker_state() {
        assert_ne!(bitlocker(&system_drive()), BitLocker::Changing);
    }
}

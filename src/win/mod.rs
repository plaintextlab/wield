//! Thin, safe-ish wrappers around the Win32 APIs Wield needs.

pub mod elevation;
pub mod fileinfo;
pub mod http;
pub mod shell;
pub mod sysver;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::core::PWSTR;

/// Null-terminated UTF-16 copy of `s`.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a null-terminated UTF-16 string. Returns an empty string for null.
pub fn from_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { p.to_string().unwrap_or_default() }
}

/// Runs `f` on a short-lived worker thread with COM initialized, and waits for it.
pub fn with_com<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(move || {
        let _com = ComGuard::new();
        f()
    })
    .join()
    .expect("worker thread panicked")
}

/// Runs `f` on a worker thread in the multithreaded apartment (for blocking WinRT calls).
pub fn with_mta<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(move || {
        let hr = unsafe { CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
        let out = f();
        if hr.is_ok() {
            unsafe { CoUninitialize() };
        }
        out
    })
    .join()
    .expect("worker thread panicked")
}

/// Initializes COM for the current thread and uninitializes it on drop.
pub struct ComGuard {
    owned: bool,
}

impl ComGuard {
    pub fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // S_FALSE means it was already initialized on this thread; we still must balance it.
        // RPC_E_CHANGED_MODE means a different model is active; COM is usable but we must not uninit.
        Self { owned: hr.is_ok() }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owned {
            unsafe { CoUninitialize() };
        }
    }
}

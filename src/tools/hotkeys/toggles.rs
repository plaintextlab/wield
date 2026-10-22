//! Reading and changing Windows' shortcut settings. All are per-user (no administrator rights).

use windows::Win32::UI::Accessibility::{FILTERKEYS, HIGHCONTRASTW, MOUSEKEYS, STICKYKEYS, TOGGLEKEYS};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETFILTERKEYS, SPI_GETHIGHCONTRAST, SPI_GETMOUSEKEYS, SPI_GETSTICKYKEYS, SPI_GETTOGGLEKEYS, SPI_SETFILTERKEYS, SPI_SETHIGHCONTRAST, SPI_SETMOUSEKEYS, SPI_SETSTICKYKEYS,
    SPI_SETTOGGLEKEYS, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SYSTEM_PARAMETERS_INFO_ACTION, SystemParametersInfoW,
};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

use super::catalog::Toggle;

/// Every accessibility shortcut uses the same "hotkey active" flag bit.
const HOTKEY_ACTIVE: u32 = 0x4;

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}

fn get_dword(path: &str, name: &str) -> Option<u32> {
    hkcu().open_subkey_with_flags(path, KEY_READ).ok()?.get_value::<u32, _>(name).ok()
}

fn get_str(path: &str, name: &str) -> Option<String> {
    hkcu().open_subkey_with_flags(path, KEY_READ).ok()?.get_value::<String, _>(name).ok()
}

fn set_dword(path: &str, name: &str, v: u32) -> Result<(), String> {
    hkcu().create_subkey(path).and_then(|(k, _)| k.set_value(name, &v)).map_err(|e| e.to_string())
}

fn set_str(path: &str, name: &str, v: &str) -> Result<(), String> {
    hkcu().create_subkey(path).and_then(|(k, _)| k.set_value(name, &v.to_string())).map_err(|e| e.to_string())
}

fn delete_value(path: &str, name: &str) -> Result<(), String> {
    match hkcu().open_subkey_with_flags(path, winreg::enums::KEY_ALL_ACCESS) {
        Ok(k) => match k.delete_value(name) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
        Err(_) => Ok(()),
    }
}

const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const EXPLORER_POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
const SYSTEM_POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\System";
const LANG: &str = r"Keyboard Layout\Toggle";

/// Gets/sets an accessibility struct. All five start with cbSize then dwFlags (both u32), so the
/// flags are always the second u32, whatever wrapper type the bindings give the field.
macro_rules! access_flag {
    ($ty:ty, $get:expr, $set:expr, $on:expr) => {{
        let mut s: $ty = unsafe { std::mem::zeroed() };
        s.cbSize = size_of::<$ty>() as u32;
        let flags = unsafe { (&mut s as *mut $ty as *mut u32).add(1) };
        unsafe { SystemParametersInfoW($get, s.cbSize, Some(&mut s as *mut _ as *mut _), Default::default()) }.map_err(|e| e.message())?;
        match $on {
            None => Ok((unsafe { *flags } & HOTKEY_ACTIVE) != 0),
            Some(on) => {
                unsafe {
                    if on {
                        *flags |= HOTKEY_ACTIVE;
                    } else {
                        *flags &= !HOTKEY_ACTIVE;
                    }
                }
                unsafe { SystemParametersInfoW($set, s.cbSize, Some(&mut s as *mut _ as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE) }.map_err(|e| e.message())?;
                Ok(on)
            }
        }
    }};
}

fn access(t: Toggle, set: Option<bool>) -> Result<bool, String> {
    match t {
        Toggle::StickyKeys => access_flag!(STICKYKEYS, SPI_GETSTICKYKEYS, SPI_SETSTICKYKEYS, set),
        Toggle::FilterKeys => access_flag!(FILTERKEYS, SPI_GETFILTERKEYS, SPI_SETFILTERKEYS, set),
        Toggle::ToggleKeys => access_flag!(TOGGLEKEYS, SPI_GETTOGGLEKEYS, SPI_SETTOGGLEKEYS, set),
        Toggle::HighContrast => access_flag!(HIGHCONTRASTW, SPI_GETHIGHCONTRAST, SPI_SETHIGHCONTRAST, set),
        Toggle::MouseKeys => access_flag!(MOUSEKEYS, SPI_GETMOUSEKEYS, SPI_SETMOUSEKEYS, set),
        _ => unreachable!(),
    }
}

/// Whether the shortcut(s) behind a toggle are on.
pub fn is_on(t: Toggle, build: u32) -> Result<bool, String> {
    Ok(match t {
        Toggle::StickyKeys | Toggle::FilterKeys | Toggle::ToggleKeys | Toggle::HighContrast | Toggle::MouseKeys => access(t, None)?,
        Toggle::AllWinKeys => get_dword(EXPLORER_POLICY, "NoWinKeys") != Some(1),
        Toggle::Lock => get_dword(SYSTEM_POLICY, "DisableLockWorkstation") != Some(1),
        Toggle::Shake => get_dword(ADVANCED, "DisallowShaking") != Some(1),
        Toggle::Snap => get_str(r"Control Panel\Desktop", "WindowArrangementActive").map(|v| v.trim() != "0").unwrap_or(true),
        Toggle::Narrator => get_dword(r"Software\Microsoft\Narrator\NoRoam", "WinEnterLaunchEnabled").unwrap_or(1) != 0,
        Toggle::ColorFilters => get_dword(r"Software\Microsoft\ColorFiltering", "HotkeyEnabled").unwrap_or(0) != 0,
        // Windows 11 22H2 and later turn this on by default.
        Toggle::PrintScreenSnip => get_dword(r"Control Panel\Keyboard", "PrintScreenKeyForSnippingEnabled").map(|v| v != 0).unwrap_or(build >= 22621),
        Toggle::LanguageAltShift => get_str(LANG, "Language Hotkey").or_else(|| get_str(LANG, "Hotkey")).map(|v| v.trim() == "1").unwrap_or(true),
        Toggle::LayoutCtrlShift => get_str(LANG, "Layout Hotkey").map(|v| v.trim() == "2").unwrap_or(true),
    })
}

/// Turns the shortcut(s) behind a toggle on or off.
pub fn set(t: Toggle, on: bool) -> Result<(), String> {
    match t {
        Toggle::StickyKeys | Toggle::FilterKeys | Toggle::ToggleKeys | Toggle::HighContrast | Toggle::MouseKeys => access(t, Some(on)).map(|_| ()),
        Toggle::AllWinKeys => {
            if on {
                delete_value(EXPLORER_POLICY, "NoWinKeys")
            } else {
                set_dword(EXPLORER_POLICY, "NoWinKeys", 1)
            }
        }
        Toggle::Lock => {
            if on {
                delete_value(SYSTEM_POLICY, "DisableLockWorkstation")
            } else {
                set_dword(SYSTEM_POLICY, "DisableLockWorkstation", 1)
            }
        }
        Toggle::Shake => set_dword(ADVANCED, "DisallowShaking", if on { 0 } else { 1 }),
        Toggle::Snap => {
            set_str(r"Control Panel\Desktop", "WindowArrangementActive", if on { "1" } else { "0" })?;
            // SPI_SETWINARRANGING applies it immediately.
            unsafe { SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x0083), on as u32, None, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE) }.map_err(|e| e.message())
        }
        Toggle::Narrator => set_dword(r"Software\Microsoft\Narrator\NoRoam", "WinEnterLaunchEnabled", on as u32),
        Toggle::ColorFilters => set_dword(r"Software\Microsoft\ColorFiltering", "HotkeyEnabled", on as u32),
        Toggle::PrintScreenSnip => set_dword(r"Control Panel\Keyboard", "PrintScreenKeyForSnippingEnabled", on as u32),
        Toggle::LanguageAltShift => {
            let v = if on { "1" } else { "3" };
            set_str(LANG, "Language Hotkey", v)?;
            set_str(LANG, "Hotkey", v)
        }
        Toggle::LayoutCtrlShift => set_str(LANG, "Layout Hotkey", if on { "2" } else { "3" }),
    }
}

// ------------------------------------------------------------------ Win+letter shortcuts

/// Letters and digits whose Win+ shortcuts Explorer is told to ignore.
pub fn disabled_win_keys() -> String {
    get_str(ADVANCED, "DisabledHotkeys").unwrap_or_default().to_uppercase()
}

#[cfg(test)]
pub fn win_keys_on(keys: &str) -> bool {
    let d = disabled_win_keys();
    !keys.chars().all(|c| d.contains(c))
}

pub fn set_win_keys(keys: &str, on: bool) -> Result<(), String> {
    let mut d: Vec<char> = disabled_win_keys().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if on {
        d.retain(|c| !keys.contains(*c));
    } else {
        for c in keys.chars() {
            if !d.contains(&c) {
                d.push(c);
            }
        }
    }
    let s: String = d.into_iter().collect();
    if s.is_empty() { delete_value(ADVANCED, "DisabledHotkeys") } else { set_str(ADVANCED, "DisabledHotkeys", &s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_toggle() {
        for t in [
            Toggle::AllWinKeys,
            Toggle::StickyKeys,
            Toggle::FilterKeys,
            Toggle::ToggleKeys,
            Toggle::HighContrast,
            Toggle::MouseKeys,
            Toggle::ColorFilters,
            Toggle::Narrator,
            Toggle::PrintScreenSnip,
            Toggle::LanguageAltShift,
            Toggle::LayoutCtrlShift,
            Toggle::Snap,
            Toggle::Shake,
            Toggle::Lock,
        ] {
            assert!(is_on(t, 19044).is_ok(), "{t:?}");
        }
    }

    /// Flips the Sticky Keys shortcut and puts it back exactly.
    #[test]
    fn sticky_keys_roundtrip() {
        let before = is_on(Toggle::StickyKeys, 0).unwrap();
        set(Toggle::StickyKeys, !before).unwrap();
        assert_eq!(is_on(Toggle::StickyKeys, 0).unwrap(), !before);
        set(Toggle::StickyKeys, before).unwrap();
        assert_eq!(is_on(Toggle::StickyKeys, 0).unwrap(), before);
    }

    #[test]
    fn win_key_list_roundtrip() {
        let before = get_str(ADVANCED, "DisabledHotkeys");
        set_win_keys("Y", false).unwrap();
        assert!(!win_keys_on("Y"));
        set_win_keys("Y", true).unwrap();
        assert!(win_keys_on("Y"));
        // Leave the user's value exactly as it was.
        match before {
            Some(v) => set_str(ADVANCED, "DisabledHotkeys", &v).unwrap(),
            None => delete_value(ADVANCED, "DisabledHotkeys").unwrap(),
        }
    }
}

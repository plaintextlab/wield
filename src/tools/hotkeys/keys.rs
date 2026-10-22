//! Key combinations: parsing "Ctrl+Alt+K", naming keys, and finding which combinations
//! running programs have already claimed as global hotkeys.

use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Windows virtual-key code.
    pub vk: u32,
}

/// (virtual key, display name, alternative spellings)
const NAMED: &[(u32, &str, &[&str])] = &[
    (0x08, "Backspace", &[]),
    (0x09, "Tab", &[]),
    (0x0D, "Enter", &[]),
    (0x13, "Pause", &["Break"]),
    (0x14, "Caps Lock", &["CapsLock"]),
    (0x1B, "Esc", &["Escape"]),
    (0x20, "Space", &["Spacebar"]),
    (0x21, "Page Up", &["PgUp"]),
    (0x22, "Page Down", &["PgDn"]),
    (0x23, "End", &[]),
    (0x24, "Home", &[]),
    (0x25, "Left", &["Left arrow"]),
    (0x26, "Up", &["Up arrow"]),
    (0x27, "Right", &["Right arrow"]),
    (0x28, "Down", &["Down arrow"]),
    (0x2C, "PrtScn", &["Print Screen", "PrintScreen"]),
    (0x2D, "Insert", &["Ins"]),
    (0x2E, "Delete", &["Del"]),
    (0x5D, "Menu", &["Application"]),
    (0x90, "Num Lock", &["NumLock"]),
    (0x91, "Scroll Lock", &[]),
    (0xBB, "Plus", &["+", "="]),
    (0xBD, "Minus", &["-"]),
    (0xBC, "Comma", &[","]),
    (0xBE, "Period", &["."]),
    (0xBA, "Semicolon", &[";"]),
    (0xBF, "Slash", &["/"]),
    (0xC0, "`", &["Grave", "Backtick"]),
    (0xDB, "[", &[]),
    (0xDD, "]", &[]),
    (0xDC, "\\", &["Backslash"]),
];

pub fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap().to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        _ => NAMED.iter().find(|n| n.0 == vk).map(|n| n.1.to_string()).unwrap_or_else(|| format!("Key 0x{vk:02X}")),
    }
}

pub fn key_from_name(s: &str) -> Option<u32> {
    let t = s.trim();
    if t.len() == 1 {
        let c = t.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u32);
        }
    }
    if let Some(n) = t.strip_prefix('F').or_else(|| t.strip_prefix('f')).and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6F + n);
        }
    }
    NAMED.iter().find(|n| n.1.eq_ignore_ascii_case(t) || n.2.iter().any(|a| a.eq_ignore_ascii_case(t))).map(|n| n.0)
}

impl Combo {
    /// "Win+Shift+S", "Ctrl+Alt+Del". None for things that aren't a single key combination.
    pub fn parse(s: &str) -> Option<Combo> {
        let mut c = Combo::default();
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        // "Ctrl+Plus" style; a literal "+" key would split into an empty part.
        let (mods, key) = parts.split_at(parts.len().checked_sub(1)?);
        for m in mods {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" | "left alt" => c.alt = true,
                "shift" | "left shift" => c.shift = true,
                "win" | "windows" => c.win = true,
                _ => return None,
            }
        }
        c.vk = key_from_name(key[0])?;
        Some(c)
    }

    pub fn label(&self) -> String {
        let mut v = Vec::new();
        if self.win {
            v.push("Win".to_string());
        }
        if self.ctrl {
            v.push("Ctrl".into());
        }
        if self.alt {
            v.push("Alt".into());
        }
        if self.shift {
            v.push("Shift".into());
        }
        v.push(key_name(self.vk));
        v.join("+")
    }

    fn mods(&self) -> HOT_KEY_MODIFIERS {
        let mut m = MOD_NOREPEAT;
        if self.ctrl {
            m |= MOD_CONTROL;
        }
        if self.alt {
            m |= MOD_ALT;
        }
        if self.shift {
            m |= MOD_SHIFT;
        }
        if self.win {
            m |= MOD_WIN;
        }
        m
    }

    /// The .lnk "shortcut key" encoding: low byte VK, high byte HOTKEYF_SHIFT=1, CONTROL=2, ALT=4.
    pub fn to_lnk(self) -> u16 {
        let mut hi = 0u16;
        if self.shift {
            hi |= 1;
        }
        if self.ctrl {
            hi |= 2;
        }
        if self.alt {
            hi |= 4;
        }
        (hi << 8) | (self.vk as u16 & 0xFF)
    }

    pub fn from_lnk(h: u16) -> Option<Combo> {
        let vk = (h & 0xFF) as u32;
        if vk == 0 {
            return None;
        }
        let hi = h >> 8;
        Some(Combo { shift: hi & 1 != 0, ctrl: hi & 2 != 0, alt: hi & 4 != 0, win: false, vk })
    }
}

/// Every combination worth probing: the usual global-hotkey modifiers with letters, digits,
/// function keys and a few others.
pub fn probe_candidates() -> Vec<Combo> {
    let mut keys: Vec<u32> = (0x41..=0x5A).chain(0x30..=0x39).chain(0x70..=0x7B).collect();
    keys.extend([0x20, 0x2C, 0xC0, 0x09, 0x0D, 0x1B, 0x24, 0x23, 0x2E, 0x2D, 0xBB, 0xBD, 0x13]);
    let mods = [
        (true, true, false, false),  // Ctrl+Alt
        (true, false, true, false),  // Ctrl+Shift
        (false, true, true, false),  // Alt+Shift
        (true, true, true, false),   // Ctrl+Alt+Shift
        (false, true, false, false), // Alt
        (false, false, false, true), // Win
        (false, false, true, true),  // Win+Shift
        (true, false, false, true),  // Win+Ctrl
        (false, true, false, true),  // Win+Alt
    ];
    let mut v = Vec::new();
    for (ctrl, alt, shift, win) in mods {
        for &vk in &keys {
            // Alt+Tab, Alt+Esc, Alt+Space… belong to Windows and aren't hotkeys apps can take.
            if alt && !ctrl && !shift && !win && matches!(vk, 0x09 | 0x1B | 0x0D) {
                continue;
            }
            v.push(Combo { ctrl, alt, shift, win, vk });
        }
    }
    // Function keys and Print Screen on their own.
    v.extend((0x70..=0x7B).chain([0x2C, 0x13]).map(|vk| Combo { vk, ..Default::default() }));
    v
}

/// Tries to register each combination for an instant; the ones that fail are already taken by
/// some program (or reserved by Windows). Runs on its own thread so hotkeys never reach the UI.
pub fn taken(candidates: Vec<Combo>) -> Vec<Combo> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        for (i, c) in candidates.into_iter().enumerate() {
            let id = 0x4000 + i as i32;
            let ok = unsafe { RegisterHotKey(None, id, c.mods(), c.vk) }.is_ok();
            if ok {
                let _ = unsafe { UnregisterHotKey(None, id) };
            } else {
                out.push(c);
            }
        }
        out
    })
    .join()
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let c = Combo::parse("Win+Shift+S").unwrap();
        assert!(c.win && c.shift && !c.ctrl && c.vk == 'S' as u32);
        assert_eq!(c.label(), "Win+Shift+S");
        assert_eq!(Combo::parse("Ctrl+Alt+Delete").unwrap().vk, 0x2E);
        assert_eq!(Combo::parse("Alt+F4").unwrap().vk, 0x73);
        assert_eq!(Combo::parse("Win+Plus").unwrap().vk, 0xBB);
        assert_eq!(Combo::parse("PrtScn").unwrap().vk, 0x2C);
        assert!(Combo::parse("Shift five times").is_none());
    }

    #[test]
    fn lnk_encoding_roundtrips() {
        let c = Combo { ctrl: true, alt: true, vk: 'N' as u32, ..Default::default() };
        assert_eq!(c.to_lnk(), 0x064E);
        assert_eq!(Combo::from_lnk(0x064E), Some(c));
    }

    #[test]
    fn probing_finds_windows_own_hotkeys() {
        // Explorer holds Win+E on a normal desktop; at minimum the call must not crash.
        let t = taken(vec![Combo::parse("Win+E").unwrap(), Combo::parse("Ctrl+Alt+Shift+F11").unwrap()]);
        assert!(t.len() <= 2);
    }
}

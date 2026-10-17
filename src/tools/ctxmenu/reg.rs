//! Registry helpers for the context menu tool: hives, exact key snapshots and indirect strings.

use std::io;

use serde::{Deserialize, Serialize};
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::core::PCWSTR;
use winreg::enums::*;
use winreg::{RegKey, RegValue};

use crate::win::wide;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Hive {
    /// HKEY_LOCAL_MACHINE: all users, needs administrator rights to change.
    Machine,
    /// HKEY_CURRENT_USER: just you.
    User,
}

impl Hive {
    pub const BOTH: [Hive; 2] = [Hive::Machine, Hive::User];
    pub fn root(self) -> RegKey {
        RegKey::predef(match self {
            Hive::Machine => HKEY_LOCAL_MACHINE,
            Hive::User => HKEY_CURRENT_USER,
        })
    }
    pub fn short(self) -> &'static str {
        match self {
            Hive::Machine => "HKLM",
            Hive::User => "HKCU",
        }
    }
    pub fn open(self, path: &str) -> Option<RegKey> {
        self.root().open_subkey_with_flags(path, KEY_READ).ok()
    }
    pub fn open_write(self, path: &str) -> io::Result<RegKey> {
        self.root().open_subkey_with_flags(path, KEY_ALL_ACCESS)
    }
    pub fn create(self, path: &str) -> io::Result<RegKey> {
        self.root().create_subkey(path).map(|(k, _)| k)
    }
    pub fn exists(self, path: &str) -> bool {
        self.open(path).is_some()
    }
}

/// `Software\Classes\<sub>`: where HKCR's per-user and machine-wide halves live.
pub fn classes(sub: &str) -> String {
    format!(r"Software\Classes\{sub}")
}

/// The merged HKEY_CLASSES_ROOT view Explorer reads.
pub fn hkcr(sub: &str) -> Option<RegKey> {
    RegKey::predef(HKEY_CLASSES_ROOT).open_subkey_with_flags(sub, KEY_READ).ok()
}

pub fn get_str(k: &RegKey, name: &str) -> Option<String> {
    k.get_value::<String, _>(name).ok()
}

pub fn has_value(k: &RegKey, name: &str) -> bool {
    k.get_raw_value(name).is_ok()
}

/// Resolves "@shell32.dll,-8506" and "@{Package?ms-resource://…}" strings to their text.
pub fn indirect(s: &str) -> String {
    let t = s.trim();
    if !t.starts_with('@') {
        return t.to_string();
    }
    let src = wide(&crate::win::fileinfo::expand_env(t));
    let mut buf = [0u16; 512];
    match unsafe { SHLoadIndirectString(PCWSTR(src.as_ptr()), &mut buf, None) } {
        Ok(()) => String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]),
        Err(_) => String::new(),
    }
}

/// An exact copy of a key, its values (any type) and all subkeys.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyDump {
    pub name: String,
    /// (name, registry type, data as hex)
    pub values: Vec<(String, u32, String)>,
    pub subkeys: Vec<KeyDump>,
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

fn reg_type(n: u32) -> RegType {
    match n {
        1 => REG_SZ,
        2 => REG_EXPAND_SZ,
        3 => REG_BINARY,
        4 => REG_DWORD,
        5 => REG_DWORD_BIG_ENDIAN,
        6 => REG_LINK,
        7 => REG_MULTI_SZ,
        8 => REG_RESOURCE_LIST,
        9 => REG_FULL_RESOURCE_DESCRIPTOR,
        10 => REG_RESOURCE_REQUIREMENTS_LIST,
        11 => REG_QWORD,
        _ => REG_NONE,
    }
}

pub fn dump(key: &RegKey, name: &str) -> KeyDump {
    let values = key.enum_values().flatten().map(|(n, v)| (n, v.vtype as u32, to_hex(&v.bytes))).collect();
    let subkeys = key
        .enum_keys()
        .flatten()
        .filter_map(|n| key.open_subkey_with_flags(&n, KEY_READ).ok().map(|k| dump(&k, &n)))
        .collect();
    KeyDump { name: name.to_string(), values, subkeys }
}

/// Snapshot of `path` in `hive`, or None when the key doesn't exist.
pub fn dump_path(hive: Hive, path: &str) -> Option<KeyDump> {
    let k = hive.open(path)?;
    Some(dump(&k, path.rsplit('\\').next().unwrap_or(path)))
}

/// Makes `parent\<d.name>` exactly match the dump. With `prune` off, subkeys that
/// aren't in the dump are kept at the top level (deeper levels are always exact).
pub fn write_exact(parent: &RegKey, d: &KeyDump, prune: bool) -> io::Result<()> {
    let (key, _) = parent.create_subkey(&d.name)?;
    let keep: Vec<&str> = d.values.iter().map(|v| v.0.as_str()).collect();
    let existing: Vec<String> = key.enum_values().flatten().map(|(n, _)| n).collect();
    for n in existing {
        if !keep.iter().any(|k| k.eq_ignore_ascii_case(&n)) {
            key.delete_value(&n)?;
        }
    }
    for (n, t, hex) in &d.values {
        key.set_raw_value(n, &RegValue { bytes: from_hex(hex).into(), vtype: reg_type(*t) })?;
    }
    if prune {
        let subs: Vec<String> = key.enum_keys().flatten().collect();
        for s in subs {
            if !d.subkeys.iter().any(|k| k.name.eq_ignore_ascii_case(&s)) {
                key.delete_subkey_all(&s)?;
            }
        }
    }
    for s in &d.subkeys {
        write_exact(&key, s, true)?;
    }
    Ok(())
}

/// Writes a dump back to `hive\path` (path includes the key's own name).
pub fn restore_path(hive: Hive, path: &str, d: &KeyDump, prune: bool) -> io::Result<()> {
    let parent = path.rsplit_once('\\').map(|(p, _)| p).unwrap_or("");
    let p = hive.create(parent)?;
    write_exact(&p, d, prune)
}

/// Deletes `hive\path` and everything under it. Missing keys count as deleted.
pub fn delete_path(hive: Hive, path: &str) -> io::Result<()> {
    match hive.root().delete_subkey_all(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Error text for a failed write, pointing at admin rights where that's the cause.
pub fn write_err(hive: Hive, e: io::Error) -> String {
    if e.kind() == io::ErrorKind::PermissionDenied {
        if hive == Hive::Machine { "This entry is installed for all users; changing it needs administrator rights.".into() } else { "Windows denied the change.".into() }
    } else {
        e.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_and_restore_roundtrip() {
        let path = r"Software\Wield-test\ctx-dump";
        let _ = delete_path(Hive::User, r"Software\Wield-test\ctx-dump");
        let k = Hive::User.create(path).unwrap();
        k.set_value("", &"hello").unwrap();
        k.set_value("Num", &7u32).unwrap();
        k.create_subkey(r"shell\cmd\command").unwrap().0.set_value("", &"notepad.exe \"%1\"").unwrap();
        let d = dump_path(Hive::User, path).unwrap();

        // Change everything, then restore.
        k.set_value("", &"changed").unwrap();
        k.set_value("Extra", &"x").unwrap();
        k.create_subkey("added").unwrap();
        delete_path(Hive::User, &format!(r"{path}\shell")).unwrap();
        restore_path(Hive::User, path, &d, true).unwrap();
        assert_eq!(dump_path(Hive::User, path).unwrap(), d);

        delete_path(Hive::User, r"Software\Wield-test").unwrap();
        assert!(!Hive::User.exists(path));
    }

    #[test]
    fn resolves_indirect_strings() {
        assert!(!indirect("@shell32.dll,-8506").is_empty());
        assert_eq!(indirect("Plain"), "Plain");
    }
}

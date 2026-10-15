//! Windows Defender Firewall rules created by Wield (grouped as "Wield").
//! All functions need COM on the calling thread; changes need administrator rights.

use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::NetworkManagement::WindowsFirewall::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDispatch};
use windows::Win32::System::Ole::IEnumVARIANT;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

pub const GROUP: &str = "Wield";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleKind {
    /// Blocks a program (inbound and outbound rules share a name).
    BlockProgram { path: String },
    /// Allows inbound connections on a port.
    OpenPort { protocol: i32, ports: String },
}

#[derive(Clone, Debug)]
pub struct WieldRule {
    pub name: String,
    pub kind: RuleKind,
    pub enabled: bool,
}

fn policy() -> Result<INetFwPolicy2, String> {
    unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }.map_err(|e| e.message())
}

fn friendly(e: windows::core::Error) -> String {
    let m = e.message();
    if m.to_lowercase().contains("access is denied") || e.code().0 as u32 == 0x80070005 {
        "Access denied. Changing firewall rules needs administrator rights.".into()
    } else {
        m
    }
}

/// True when the firewall is on for every currently active network profile.
pub fn enabled_for_current_profiles() -> Result<bool, String> {
    let p = policy()?;
    unsafe {
        let current = p.CurrentProfileTypes().map_err(|e| e.message())?;
        for bit in [1, 2, 4] {
            if current & bit != 0 && !p.get_FirewallEnabled(NET_FW_PROFILE_TYPE2(bit)).map_err(|e| e.message())?.as_bool() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn all_rules() -> Result<Vec<INetFwRule>, String> {
    let p = policy()?;
    let mut out = Vec::new();
    unsafe {
        let rules = p.Rules().map_err(|e| e.message())?;
        let e: IEnumVARIANT = rules._NewEnum().map_err(|e| e.message())?.cast().map_err(|e| e.message())?;
        loop {
            let mut v = [VARIANT::default()];
            let mut fetched = 0u32;
            let hr = e.Next(&mut v, &mut fetched);
            if hr.is_err() || fetched == 0 {
                break;
            }
            if let Ok(disp) = IDispatch::try_from(&v[0]) {
                if let Ok(rule) = disp.cast::<INetFwRule>() {
                    out.push(rule);
                }
            }
        }
    }
    Ok(out)
}

pub fn wield_rules() -> Result<Vec<WieldRule>, String> {
    let mut by_name: Vec<WieldRule> = Vec::new();
    for r in all_rules()? {
        unsafe {
            if r.Grouping().map(|g| g.to_string()).unwrap_or_default() != GROUP {
                continue;
            }
            let name = r.Name().map(|n| n.to_string()).unwrap_or_default();
            let enabled = r.Enabled().map(|b| b.as_bool()).unwrap_or(false);
            let app = r.ApplicationName().map(|a| a.to_string()).unwrap_or_default();
            let action = r.Action().unwrap_or(NET_FW_ACTION_ALLOW);
            let kind = if action == NET_FW_ACTION_BLOCK && !app.is_empty() {
                RuleKind::BlockProgram { path: app }
            } else {
                RuleKind::OpenPort {
                    protocol: r.Protocol().unwrap_or(6),
                    ports: r.LocalPorts().map(|p| p.to_string()).unwrap_or_default(),
                }
            };
            if let Some(existing) = by_name.iter_mut().find(|w| w.name == name) {
                existing.enabled |= enabled;
                continue;
            }
            by_name.push(WieldRule { name, kind, enabled });
        }
    }
    by_name.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(by_name)
}

fn short_hash(s: &str) -> String {
    // FNV-1a: stable, dependency-free, enough to keep rule names unique per path.
    let mut h: u32 = 0x811c9dc5;
    for b in s.to_lowercase().bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x01000193);
    }
    format!("{h:08x}")
}

pub fn block_rule_name(path: &str) -> String {
    let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_else(|| path.to_lowercase());
    format!("Wield: block {file} [{}]", short_hash(path))
}

fn new_rule() -> Result<INetFwRule, String> {
    unsafe { CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER) }.map_err(|e| e.message())
}

/// Blocks all inbound and outbound traffic for a program.
pub fn block_program(path: &str) -> Result<(), String> {
    let name = block_rule_name(path);
    remove_rule(&name)?;
    let p = policy()?;
    unsafe {
        let rules = p.Rules().map_err(|e| e.message())?;
        for dir in [NET_FW_RULE_DIR_OUT, NET_FW_RULE_DIR_IN] {
            let r = new_rule()?;
            r.SetName(&BSTR::from(name.as_str())).map_err(friendly)?;
            r.SetDescription(&BSTR::from(format!("Blocks network access for {path}. Created by Wield.").as_str())).map_err(friendly)?;
            r.SetApplicationName(&BSTR::from(path)).map_err(friendly)?;
            r.SetDirection(dir).map_err(friendly)?;
            r.SetAction(NET_FW_ACTION_BLOCK).map_err(friendly)?;
            r.SetProfiles(NET_FW_PROFILE2_ALL.0).map_err(friendly)?;
            r.SetGrouping(&BSTR::from(GROUP)).map_err(friendly)?;
            r.SetEnabled(VARIANT_TRUE).map_err(friendly)?;
            rules.Add(&r).map_err(friendly)?;
        }
    }
    Ok(())
}

/// Allows inbound connections on a TCP or UDP port (e.g. "8080" or "27015-27030").
pub fn open_port(protocol: i32, ports: &str, label: &str) -> Result<String, String> {
    let ports = ports.trim();
    if ports.is_empty() || !ports.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ',') {
        return Err("Enter a port like 8080, a range like 27015-27030, or a list like 80,443.".into());
    }
    let proto = if protocol == NET_FW_IP_PROTOCOL_UDP.0 { "UDP" } else { "TCP" };
    let label = label.trim();
    let name = if label.is_empty() { format!("Wield: open {proto} {ports}") } else { format!("Wield: open {proto} {ports} ({label})") };
    remove_rule(&name)?;
    let p = policy()?;
    unsafe {
        let rules = p.Rules().map_err(|e| e.message())?;
        let r = new_rule()?;
        r.SetName(&BSTR::from(name.as_str())).map_err(friendly)?;
        r.SetDescription(&BSTR::from("Inbound port opened by Wield.")).map_err(friendly)?;
        r.SetProtocol(protocol).map_err(friendly)?;
        r.SetLocalPorts(&BSTR::from(ports)).map_err(friendly)?;
        r.SetDirection(NET_FW_RULE_DIR_IN).map_err(friendly)?;
        r.SetAction(NET_FW_ACTION_ALLOW).map_err(friendly)?;
        r.SetProfiles(NET_FW_PROFILE2_ALL.0).map_err(friendly)?;
        r.SetGrouping(&BSTR::from(GROUP)).map_err(friendly)?;
        r.SetEnabled(VARIANT_TRUE).map_err(friendly)?;
        rules.Add(&r).map_err(friendly)?;
    }
    Ok(name)
}

/// Enables or disables every rule with this name (both directions of a block).
pub fn set_enabled(name: &str, enabled: bool) -> Result<(), String> {
    let mut found = false;
    for r in all_rules()? {
        unsafe {
            if r.Name().map(|n| n.to_string()).unwrap_or_default() == name {
                found = true;
                r.SetEnabled(if enabled { VARIANT_TRUE } else { VARIANT_FALSE }).map_err(friendly)?;
            }
        }
    }
    if found { Ok(()) } else { Err(format!("Rule \"{name}\" not found.")) }
}

/// Deletes every rule with this name. Missing rules are not an error.
pub fn remove_rule(name: &str) -> Result<(), String> {
    let p = policy()?;
    unsafe {
        let rules = p.Rules().map_err(|e| e.message())?;
        let bname = BSTR::from(name);
        // Remove() deletes one rule per call; same-named rules need repeated calls.
        for _ in 0..16 {
            if rules.Item(&bname).is_err() {
                break;
            }
            rules.Remove(&bname).map_err(friendly)?;
        }
    }
    Ok(())
}

/// Removes the block rules for a program path.
pub fn unblock_program(path: &str) -> Result<(), String> {
    remove_rule(&block_rule_name(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_are_stable_and_unique() {
        let a = block_rule_name(r"C:\Program Files\App\app.exe");
        assert_eq!(a, block_rule_name(r"c:\program files\app\APP.EXE"));
        assert_ne!(a, block_rule_name(r"C:\Other\app.exe"));
        assert!(a.starts_with("Wield: block app.exe ["));
    }

    #[test]
    fn reads_firewall() {
        crate::win::with_com(|| {
            assert!(all_rules().map(|r| !r.is_empty()).unwrap_or(true));
            let _ = enabled_for_current_profiles();
            let _ = wield_rules();
        });
    }
}

//! System network settings: DNS servers, DNS cache, hosts file, repairs and public IP.

use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string();
    if out.status.success() { Ok(text) } else { Err(if text.is_empty() { format!("{program} failed") } else { text }) }
}

fn powershell(script: &str) -> Result<String, String> {
    run("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", script])
}

/// Public DNS presets: (label, IPv4 servers, note).
pub const DNS_PRESETS: &[(&str, &[&str], &str)] = &[
    ("Automatic (DHCP)", &[], "Use whatever your router or network provides."),
    ("Cloudflare", &["1.1.1.1", "1.0.0.1"], "Fast, privacy-focused, no filtering."),
    ("Cloudflare (malware blocking)", &["1.1.1.2", "1.0.0.2"], "Blocks known malware domains."),
    ("Google", &["8.8.8.8", "8.8.4.4"], "Fast and reliable."),
    ("Quad9", &["9.9.9.9", "149.112.112.112"], "Blocks malicious domains, privacy-focused."),
    ("AdGuard", &["94.140.14.14", "94.140.15.15"], "Blocks ads and trackers for every app."),
];

/// Sets IPv4 DNS servers for an adapter; an empty list restores DHCP. Needs administrator rights.
pub fn set_dns(if_index: u32, servers: &[&str]) -> Result<(), String> {
    let script = if servers.is_empty() {
        format!("Set-DnsClientServerAddress -InterfaceIndex {if_index} -ResetServerAddresses -ErrorAction Stop")
    } else {
        let list = servers.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",");
        format!("Set-DnsClientServerAddress -InterfaceIndex {if_index} -ServerAddresses ({list}) -ErrorAction Stop")
    };
    powershell(&script).map(|_| ()).map_err(admin_hint)
}

pub fn flush_dns() -> Result<String, String> {
    run("ipconfig.exe", &["/flushdns"])
}

fn admin_hint(e: String) -> String {
    let l = e.to_lowercase();
    if l.contains("access") && l.contains("denied") || l.contains("elevation") || l.contains("administrator") {
        "Access denied. This needs administrator rights.".into()
    } else {
        e.lines().find(|l| !l.trim().is_empty()).unwrap_or("Failed").trim().to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repair {
    RenewIp,
    FlushArp,
    ResetWinsock,
    ResetTcpIp,
}

impl Repair {
    pub const ALL: [Repair; 4] = [Repair::RenewIp, Repair::FlushArp, Repair::ResetWinsock, Repair::ResetTcpIp];
    pub fn title(self) -> &'static str {
        match self {
            Repair::RenewIp => "Renew IP address",
            Repair::FlushArp => "Clear ARP cache",
            Repair::ResetWinsock => "Reset Winsock",
            Repair::ResetTcpIp => "Reset TCP/IP stack",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Repair::RenewIp => "Releases and requests a new address from the router. Drops connections for a few seconds.",
            Repair::FlushArp => "Forgets cached device addresses on your local network.",
            Repair::ResetWinsock => "Fixes broken network catalogs left by VPNs and old security software. Needs a restart.",
            Repair::ResetTcpIp => "Restores TCP/IP settings to defaults. Removes static IPs. Needs a restart.",
        }
    }
    pub fn needs_restart(self) -> bool {
        matches!(self, Repair::ResetWinsock | Repair::ResetTcpIp)
    }
    pub fn run(self) -> Result<String, String> {
        match self {
            Repair::RenewIp => {
                let _ = run("ipconfig.exe", &["/release"]);
                run("ipconfig.exe", &["/renew"]).map_err(admin_hint)
            }
            Repair::FlushArp => run("netsh.exe", &["interface", "ip", "delete", "arpcache"]).map_err(admin_hint),
            Repair::ResetWinsock => run("netsh.exe", &["winsock", "reset"]).map_err(admin_hint),
            Repair::ResetTcpIp => run("netsh.exe", &["int", "ip", "reset"]).map_err(admin_hint),
        }
    }
}

// ------------------------------------------------------------------ hosts file

pub fn hosts_path() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\drivers\etc\hosts")
}

const MARKER: &str = "# added by Wield";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostsEntry {
    pub ip: String,
    pub host: String,
    pub by_wield: bool,
}

pub fn parse_hosts(text: &str) -> Vec<HostsEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let (content, comment) = line.split_once('#').map(|(c, k)| (c, Some(k))).unwrap_or((line, None));
        let mut parts = content.split_whitespace();
        let (Some(ip), hosts) = (parts.next(), parts) else { continue };
        let by_wield = comment.is_some_and(|c| c.trim() == MARKER.trim_start_matches("# "));
        for h in hosts {
            out.push(HostsEntry { ip: ip.to_string(), host: h.to_string(), by_wield });
        }
    }
    out
}

pub fn read_hosts() -> Result<Vec<HostsEntry>, String> {
    std::fs::read_to_string(hosts_path()).map(|t| parse_hosts(&t)).map_err(|e| e.to_string())
}

fn write_hosts(text: &str) -> Result<(), String> {
    let path = hosts_path();
    let backup = path.with_file_name("hosts.wield-backup");
    if !backup.exists() {
        let _ = std::fs::copy(&path, &backup);
    }
    std::fs::write(&path, text).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            "Access denied. Editing the hosts file needs administrator rights (and some antivirus apps protect it).".into()
        } else {
            e.to_string()
        }
    })
}

pub fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.contains('.')
        && d.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Points a domain at 0.0.0.0 so nothing on this PC can reach it.
pub fn block_domain(domain: &str) -> Result<(), String> {
    let d = domain.trim().trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_lowercase();
    if !valid_domain(&d) {
        return Err(format!("\"{domain}\" isn't a valid domain name."));
    }
    let mut text = std::fs::read_to_string(hosts_path()).map_err(|e| e.to_string())?;
    if parse_hosts(&text).iter().any(|e| e.host.eq_ignore_ascii_case(&d)) {
        return Err(format!("{d} is already in the hosts file."));
    }
    if !text.ends_with('\n') && !text.is_empty() {
        text.push_str("\r\n");
    }
    text.push_str(&format!("0.0.0.0 {d} {MARKER}\r\n"));
    write_hosts(&text)
}

/// Removes lines Wield added for a domain.
pub fn unblock_domain(domain: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(hosts_path()).map_err(|e| e.to_string())?;
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !(l.contains(MARKER) && l.split_whitespace().nth(1).is_some_and(|h| h.eq_ignore_ascii_case(domain))))
        .collect();
    write_hosts(&(kept.join("\r\n") + "\r\n"))
}

// ------------------------------------------------------------------ public IP

/// Asks api.ipify.org for this network's public IPv4 address. Only runs when the user asks.
pub fn public_ip() -> Result<String, String> {
    use std::net::ToSocketAddrs;
    let addr = ("api.ipify.org", 80).to_socket_addrs().map_err(|e| e.to_string())?.next().ok_or("DNS lookup failed")?;
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(b"GET / HTTP/1.1\r\nHost: api.ipify.org\r\nUser-Agent: Wield\r\nConnection: close\r\n\r\n").map_err(|e| e.to_string())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| e.to_string())?;
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").trim().to_string();
    body.parse::<std::net::IpAddr>().map(|ip| ip.to_string()).map_err(|_| "Unexpected response from api.ipify.org".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts() {
        let t = "# comment\r\n127.0.0.1 localhost\r\n0.0.0.0 ads.example.com # added by Wield\r\n10.0.0.2 nas nas.local\r\n";
        let e = parse_hosts(t);
        assert_eq!(e.len(), 4);
        assert!(e.iter().any(|x| x.host == "ads.example.com" && x.by_wield));
        assert!(e.iter().any(|x| x.host == "nas.local" && !x.by_wield));
    }

    #[test]
    fn validates_domains() {
        assert!(valid_domain("telemetry.example.com"));
        assert!(!valid_domain("localhost"));
        assert!(!valid_domain("bad domain.com"));
    }
}

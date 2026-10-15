//! What's on the other end of a connection: host names, tracker/telemetry flags and
//! well-known port names.
//!
//! Host names come from the Windows DNS cache first (the name the app actually looked up),
//! then reverse DNS. Resolution runs on a background thread with a shared cache.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::os::windows::process::CommandExt;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, GetNameInfoW, IN_ADDR, IN_ADDR_0, IN6_ADDR, IN6_ADDR_0, NI_NAMEREQD, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6, socklen_t,
    WSADATA, WSAStartup,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    Telemetry,
    Tracking,
    CrashReports,
    Updates,
    Cdn,
    Cloud,
}

impl Tag {
    pub fn label(self) -> &'static str {
        match self {
            Tag::Telemetry => "Telemetry",
            Tag::Tracking => "Ads / tracking",
            Tag::CrashReports => "Crash reports",
            Tag::Updates => "Updates",
            Tag::Cdn => "CDN",
            Tag::Cloud => "Cloud",
        }
    }
    /// Whether this deserves the user's attention in a privacy sense.
    pub fn is_privacy_relevant(self) -> bool {
        matches!(self, Tag::Telemetry | Tag::Tracking | Tag::CrashReports)
    }
}

/// Hostname substrings (lowercase). First match wins, so specific entries come first.
const HOSTS: &[(&str, Tag)] = &[
    ("events.data.microsoft.com", Tag::Telemetry),
    ("vortex", Tag::Telemetry),
    ("telemetry", Tag::Telemetry),
    ("watson.", Tag::CrashReports),
    ("settings-win.data.microsoft.com", Tag::Telemetry),
    ("self.events.data", Tag::Telemetry),
    ("browser.events.data", Tag::Telemetry),
    ("data.microsoft.com", Tag::Telemetry),
    ("applicationinsights", Tag::Telemetry),
    ("dc.services.visualstudio.com", Tag::Telemetry),
    ("nr-data.net", Tag::Telemetry),
    ("datadoghq", Tag::Telemetry),
    ("mixpanel", Tag::Tracking),
    ("segment.io", Tag::Tracking),
    ("amplitude.com", Tag::Tracking),
    ("google-analytics", Tag::Tracking),
    ("googletagmanager", Tag::Tracking),
    ("doubleclick", Tag::Tracking),
    ("googlesyndication", Tag::Tracking),
    ("googleadservices", Tag::Tracking),
    ("app-measurement", Tag::Tracking),
    ("scorecardresearch", Tag::Tracking),
    ("adnxs", Tag::Tracking),
    ("adsrvr", Tag::Tracking),
    ("criteo", Tag::Tracking),
    ("taboola", Tag::Tracking),
    ("outbrain", Tag::Tracking),
    ("amazon-adsystem", Tag::Tracking),
    ("hotjar", Tag::Tracking),
    ("branch.io", Tag::Tracking),
    ("adjust.com", Tag::Tracking),
    ("appsflyer", Tag::Tracking),
    ("facebook.net", Tag::Tracking),
    ("graph.facebook.com", Tag::Tracking),
    ("ads.", Tag::Tracking),
    ("sentry.io", Tag::CrashReports),
    ("bugsnag", Tag::CrashReports),
    ("crashlytics", Tag::CrashReports),
    ("appcenter.ms", Tag::CrashReports),
    ("windowsupdate", Tag::Updates),
    ("update.microsoft", Tag::Updates),
    ("delivery.mp.microsoft.com", Tag::Updates),
    ("dl.delivery", Tag::Updates),
    ("update", Tag::Updates),
    ("akamai", Tag::Cdn),
    ("cloudfront.net", Tag::Cdn),
    ("fastly", Tag::Cdn),
    ("azureedge.net", Tag::Cdn),
    ("edgecastcdn", Tag::Cdn),
    ("msedge.net", Tag::Cdn),
    ("cloudflare", Tag::Cdn),
    ("1e100.net", Tag::Cloud),
    ("googleusercontent", Tag::Cloud),
    ("amazonaws.com", Tag::Cloud),
    ("cloudapp.azure.com", Tag::Cloud),
    ("cloudapp.net", Tag::Cloud),
];

pub fn classify_host(host: &str) -> Option<Tag> {
    let h = host.to_lowercase();
    HOSTS.iter().find(|(k, _)| h.contains(k)).map(|(_, t)| *t)
}

/// Common service names for ports, for the Ports and Scanner views.
pub fn service_name(port: u16) -> Option<&'static str> {
    Some(match port {
        20 | 21 => "FTP",
        22 => "SSH",
        23 => "Telnet",
        25 => "SMTP",
        53 => "DNS",
        67 | 68 => "DHCP",
        80 => "HTTP",
        110 => "POP3",
        123 => "NTP",
        135 => "RPC endpoint mapper",
        137 => "NetBIOS name",
        138 => "NetBIOS datagram",
        139 => "NetBIOS session",
        143 => "IMAP",
        161 => "SNMP",
        389 => "LDAP",
        443 => "HTTPS",
        445 => "SMB file sharing",
        500 => "IPsec IKE",
        515 => "LPD printing",
        548 => "AFP",
        554 => "RTSP",
        631 => "IPP printing",
        853 => "DNS over TLS",
        993 => "IMAPS",
        995 => "POP3S",
        1080 => "SOCKS proxy",
        1194 => "OpenVPN",
        1433 => "SQL Server",
        1900 => "SSDP / UPnP",
        2049 => "NFS",
        3074 => "Xbox Live",
        3306 => "MySQL",
        3389 => "Remote Desktop",
        3702 => "WS-Discovery",
        5000 => "UPnP / dev server",
        5040 => "Windows CDP",
        5050 => "Windows Connected Devices",
        5353 => "mDNS",
        5355 => "LLMNR",
        5357 => "WSD",
        5432 => "PostgreSQL",
        5900 => "VNC",
        6379 => "Redis",
        7680 => "Delivery Optimization",
        8080 => "HTTP alt / proxy",
        8443 => "HTTPS alt",
        9100 => "Raw printing",
        11434 => "Ollama",
        27017 => "MongoDB",
        27036 => "Steam",
        32400 => "Plex",
        49664..=49680 => "Windows RPC",
        _ => return None,
    })
}

/// Ports scanned by "Common ports" in the port scanner.
pub const COMMON_PORTS: &[u16] = &[
    21, 22, 23, 25, 53, 80, 110, 135, 139, 143, 389, 443, 445, 548, 554, 631, 993, 995, 1433, 1900, 2049, 3306, 3389, 5000, 5357,
    5432, 5900, 6379, 8000, 8008, 8080, 8443, 8888, 9000, 9100, 32400,
];

fn reverse_dns(ip: IpAddr) -> Option<String> {
    let mut host = [0u16; 1025];
    let rc = unsafe {
        match ip {
            IpAddr::V4(a) => {
                let sa = SOCKADDR_IN {
                    sin_family: AF_INET,
                    sin_port: 0,
                    sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: u32::from_ne_bytes(a.octets()) } },
                    sin_zero: [0; 8],
                };
                GetNameInfoW(&sa as *const _ as *const SOCKADDR, socklen_t(size_of::<SOCKADDR_IN>() as i32), Some(&mut host), None, NI_NAMEREQD as i32)
            }
            IpAddr::V6(a) => {
                let sa = SOCKADDR_IN6 { sin6_family: AF_INET6, sin6_addr: IN6_ADDR { u: IN6_ADDR_0 { Byte: a.octets() } }, ..Default::default() };
                GetNameInfoW(&sa as *const _ as *const SOCKADDR, socklen_t(size_of::<SOCKADDR_IN6>() as i32), Some(&mut host), None, NI_NAMEREQD as i32)
            }
        }
    };
    if rc != 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&host[..host.iter().position(|&c| c == 0).unwrap_or(0)]);
    (!s.is_empty()).then_some(s)
}

/// Reads the Windows DNS client cache: IP address -> the name that was looked up.
pub fn dns_cache() -> HashMap<IpAddr, String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-DnsClientCache | Where-Object { $_.Type -eq 1 -or $_.Type -eq 28 } | ForEach-Object { \"$($_.Data)`t$($_.Entry)\" }",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let mut map = HashMap::new();
    if let Ok(o) = out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            if let Some((ip, name)) = line.trim().split_once('\t') {
                if let Ok(ip) = ip.parse::<IpAddr>() {
                    map.entry(ip).or_insert_with(|| name.to_string());
                }
            }
        }
    }
    map
}

/// Background resolver shared by the UI. Unknown IPs are queued; results land in the cache.
#[derive(Clone)]
pub struct Resolver {
    names: Arc<Mutex<HashMap<IpAddr, Option<String>>>>,
    tx: Sender<IpAddr>,
}

impl Resolver {
    pub fn start(ctx: eframe::egui::Context) -> Self {
        let names: Arc<Mutex<HashMap<IpAddr, Option<String>>>> = Arc::default();
        let (tx, rx): (Sender<IpAddr>, Receiver<IpAddr>) = channel();
        let shared = names.clone();
        std::thread::spawn(move || {
            unsafe {
                let mut data = WSADATA::default();
                WSAStartup(0x0202, &mut data);
            }
            let mut cache = dns_cache();
            let mut cache_at = std::time::Instant::now();
            let mut pending: HashSet<IpAddr> = HashSet::new();
            while let Ok(first) = rx.recv() {
                pending.insert(first);
                while let Ok(ip) = rx.try_recv() {
                    pending.insert(ip);
                }
                if cache_at.elapsed().as_secs() > 20 {
                    cache = dns_cache();
                    cache_at = std::time::Instant::now();
                }
                for ip in pending.drain() {
                    let name = cache.get(&ip).cloned().or_else(|| reverse_dns(ip));
                    shared.lock().unwrap().insert(ip, name);
                }
                ctx.request_repaint();
            }
        });
        Self { names, tx }
    }

    /// The host name for an IP, or None while unknown. Queues a lookup the first time.
    pub fn name(&self, ip: IpAddr) -> Option<String> {
        let mut names = self.names.lock().unwrap();
        match names.get(&ip) {
            Some(n) => n.clone(),
            None => {
                names.insert(ip, None);
                let _ = self.tx.send(ip);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_hosts() {
        assert_eq!(classify_host("v10.events.data.microsoft.com"), Some(Tag::Telemetry));
        assert_eq!(classify_host("stats.g.doubleclick.net"), Some(Tag::Tracking));
        assert_eq!(classify_host("o123.ingest.sentry.io"), Some(Tag::CrashReports));
        assert_eq!(classify_host("example.org"), None);
    }

    #[test]
    fn port_names() {
        assert_eq!(service_name(3389), Some("Remote Desktop"));
        assert_eq!(service_name(12345), None);
    }
}

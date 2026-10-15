//! Network adapters, ping, local network discovery and TCP port scanning.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR, SOCKADDR_IN, SOCKADDR_IN6};

use crate::win::from_pwstr;

#[derive(Clone, Debug, Default)]
pub struct Adapter {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub mac: String,
    pub up: bool,
    pub kind: &'static str,
    pub speed_bps: u64,
    pub ipv4: Vec<(Ipv4Addr, u8)>,
    pub ipv6: Vec<Ipv6Addr>,
    pub gateways: Vec<IpAddr>,
    pub dns: Vec<IpAddr>,
}

unsafe fn sockaddr_ip(sa: *const SOCKADDR) -> Option<IpAddr> {
    if sa.is_null() {
        return None;
    }
    unsafe {
        match (*sa).sa_family {
            AF_INET => {
                let s = &*(sa as *const SOCKADDR_IN);
                Some(IpAddr::V4(Ipv4Addr::from(s.sin_addr.S_un.S_addr.to_ne_bytes())))
            }
            AF_INET6 => {
                let s = &*(sa as *const SOCKADDR_IN6);
                Some(IpAddr::V6(Ipv6Addr::from(s.sin6_addr.u.Byte)))
            }
            _ => None,
        }
    }
}

fn if_kind(t: u32) -> &'static str {
    match t {
        6 => "Ethernet",
        71 => "Wi-Fi",
        24 => "Loopback",
        23 | 131 => "VPN / tunnel",
        243 | 244 => "Mobile broadband",
        _ => "Other",
    }
}

pub fn adapters() -> Vec<Adapter> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size = 32 * 1024u32;
    let mut buf: Vec<u64> = vec![0; size as usize / 8];
    unsafe {
        let mut rc = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buf.as_mut_ptr() as *mut _), &mut size);
        if rc == 111 {
            // ERROR_BUFFER_OVERFLOW
            buf = vec![0; (size as usize).div_ceil(8)];
            rc = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(buf.as_mut_ptr() as *mut _), &mut size);
        }
        if rc != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            let mut ad = Adapter {
                index: a.Anonymous1.Anonymous.IfIndex,
                name: from_pwstr(a.FriendlyName),
                description: from_pwstr(a.Description),
                mac: a.PhysicalAddress[..a.PhysicalAddressLength.min(8) as usize].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-"),
                up: a.OperStatus == IfOperStatusUp,
                kind: if_kind(a.IfType),
                speed_bps: a.ReceiveLinkSpeed.max(a.TransmitLinkSpeed),
                ..Default::default()
            };
            let mut u = a.FirstUnicastAddress;
            while !u.is_null() {
                match sockaddr_ip((*u).Address.lpSockaddr) {
                    Some(IpAddr::V4(v)) => ad.ipv4.push((v, (*u).OnLinkPrefixLength)),
                    Some(IpAddr::V6(v)) => ad.ipv6.push(v),
                    None => {}
                }
                u = (*u).Next;
            }
            let mut g = a.FirstGatewayAddress;
            while !g.is_null() {
                if let Some(ip) = sockaddr_ip((*g).Address.lpSockaddr) {
                    ad.gateways.push(ip);
                }
                g = (*g).Next;
            }
            let mut d = a.FirstDnsServerAddress;
            while !d.is_null() {
                if let Some(ip) = sockaddr_ip((*d).Address.lpSockaddr) {
                    ad.dns.push(ip);
                }
                d = (*d).Next;
            }
            if a.IfType != 24 {
                out.push(ad);
            }
            p = a.Next;
        }
        out.sort_by_key(|a| (!a.up, a.gateways.is_empty(), a.name.to_lowercase()));
        out
    }
}

/// ICMP echo. Returns the round-trip time in ms, or None on timeout.
pub fn ping(ip: Ipv4Addr, timeout_ms: u32) -> Option<u32> {
    unsafe {
        let h = IcmpCreateFile().ok()?;
        let data = *b"wield-ping";
        let mut reply = vec![0u8; size_of::<ICMP_ECHO_REPLY>() + data.len() + 8];
        let n = IcmpSendEcho(
            h,
            u32::from_ne_bytes(ip.octets()),
            data.as_ptr() as *const _,
            data.len() as u16,
            None,
            reply.as_mut_ptr() as *mut _,
            reply.len() as u32,
            timeout_ms,
        );
        let _ = IcmpCloseHandle(h);
        if n == 0 {
            return None;
        }
        let r = &*(reply.as_ptr() as *const ICMP_ECHO_REPLY);
        (r.Status == 0).then_some(r.RoundTripTime)
    }
}

/// IPv4 address -> MAC from the neighbor (ARP) cache.
pub fn arp_table() -> Vec<(Ipv4Addr, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut t: *mut MIB_IPNET_TABLE2 = std::ptr::null_mut();
        if GetIpNetTable2(AF_INET, &mut t).0 != 0 || t.is_null() {
            return out;
        }
        for r in std::slice::from_raw_parts((*t).Table.as_ptr(), (*t).NumEntries as usize) {
            let len = r.PhysicalAddressLength as usize;
            // State 1 = unreachable, 0 = incomplete.
            if len == 0 || r.State.0 <= 1 || r.PhysicalAddress[..len].iter().all(|b| *b == 0 || *b == 0xff) {
                continue;
            }
            let ip = Ipv4Addr::from(r.Address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes());
            if ip.is_multicast() || ip.is_broadcast() {
                continue;
            }
            out.push((ip, r.PhysicalAddress[..len].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-")));
        }
        FreeMibTable(t as *const _);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Device {
    pub ip: Ipv4Addr,
    pub mac: Option<String>,
    pub latency_ms: Option<u32>,
    pub is_gateway: bool,
    pub is_self: bool,
}

impl Device {
    /// Locally administered MACs are usually randomized "private" addresses of phones/laptops.
    pub fn randomized_mac(&self) -> bool {
        self.mac
            .as_ref()
            .and_then(|m| u8::from_str_radix(&m[..2], 16).ok())
            .is_some_and(|b| b & 0x02 != 0)
    }
}

pub enum ScanMsg {
    Progress(usize, usize),
    Device(Device),
    Done(String),
}

/// Hosts to sweep for an adapter address. Large subnets are limited to the local /24.
pub fn hosts_for(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    let prefix = prefix.clamp(16, 30).max(if prefix < 22 { 24 } else { prefix });
    let mask = u32::MAX << (32 - prefix);
    let base = u32::from(ip) & mask;
    let count = 1u32 << (32 - prefix);
    (1..count - 1).map(|i| Ipv4Addr::from(base + i)).collect()
}

/// Pings every address on the networks of active adapters, then merges in the ARP cache.
pub fn lan_scan(tx: Sender<ScanMsg>, ctx: eframe::egui::Context) {
    let adapters = adapters();
    let mut targets: Vec<Ipv4Addr> = Vec::new();
    let mut own = Vec::new();
    let mut gateways = Vec::new();
    let mut nets = Vec::new();
    for a in adapters.iter().filter(|a| a.up && !a.gateways.is_empty()) {
        for (ip, prefix) in &a.ipv4 {
            if ip.is_link_local() {
                continue;
            }
            own.push(*ip);
            nets.push(format!("{ip}/{prefix}"));
            targets.extend(hosts_for(*ip, *prefix));
        }
        gateways.extend(a.gateways.iter().filter_map(|g| if let IpAddr::V4(v) = g { Some(*v) } else { None }));
    }
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        let _ = tx.send(ScanMsg::Done("No connected IPv4 network with a gateway was found.".into()));
        ctx.request_repaint();
        return;
    }

    let total = targets.len();
    let queue = Arc::new(Mutex::new(targets));
    let done = Arc::new(AtomicUsize::new(0));
    let found = Arc::new(Mutex::new(Vec::<Ipv4Addr>::new()));
    let mut handles = Vec::new();
    for _ in 0..64 {
        let (queue, done, found, tx, ctx, own, gateways) =
            (queue.clone(), done.clone(), found.clone(), tx.clone(), ctx.clone(), own.clone(), gateways.clone());
        handles.push(std::thread::spawn(move || {
            loop {
                let Some(ip) = queue.lock().unwrap().pop() else { break };
                if let Some(ms) = ping(ip, 700) {
                    found.lock().unwrap().push(ip);
                    let _ = tx.send(ScanMsg::Device(Device {
                        ip,
                        mac: None,
                        latency_ms: Some(ms),
                        is_gateway: gateways.contains(&ip),
                        is_self: own.contains(&ip),
                    }));
                }
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 16 == 0 || n == total {
                    let _ = tx.send(ScanMsg::Progress(n, total));
                    ctx.request_repaint();
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    // Devices that ignore ping still answer ARP; the sweep has just refreshed the cache.
    let found = found.lock().unwrap().clone();
    let in_nets = |ip: &Ipv4Addr| {
        adapters.iter().flat_map(|a| a.ipv4.iter()).any(|(net, p)| {
            let p = (*p).clamp(16, 30);
            let m = u32::MAX << (32 - p);
            u32::from(*net) & m == u32::from(*ip) & m
        })
    };
    for (ip, mac) in arp_table() {
        if !in_nets(&ip) {
            continue;
        }
        let _ = tx.send(ScanMsg::Device(Device {
            ip,
            mac: Some(mac),
            latency_ms: None,
            is_gateway: gateways.contains(&ip),
            is_self: own.contains(&ip),
        }));
        let _ = found;
    }
    for ip in &own {
        let _ = tx.send(ScanMsg::Device(Device { ip: *ip, mac: None, latency_ms: Some(0), is_gateway: false, is_self: true }));
    }
    let _ = tx.send(ScanMsg::Done(format!("Scanned {total} addresses on {}.", nets.join(", "))));
    ctx.request_repaint();
}

pub enum PortMsg {
    Progress(usize, usize),
    Open(u16, u32),
    Done(String),
}

/// Parses "80,443,8000-8100" into a sorted, de-duplicated list (max 65535 ports).
pub fn parse_ports(spec: &str) -> Result<Vec<u16>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            let a: u16 = a.trim().parse().map_err(|_| format!("Bad port: {part}"))?;
            let b: u16 = b.trim().parse().map_err(|_| format!("Bad port: {part}"))?;
            if a == 0 || b < a {
                return Err(format!("Bad range: {part}"));
            }
            out.extend(a..=b);
        } else {
            let p: u16 = part.parse().map_err(|_| format!("Bad port: {part}"))?;
            if p == 0 {
                return Err("Port 0 is not valid.".into());
            }
            out.push(p);
        }
    }
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        return Err("Enter ports like 80,443 or 1-1024.".into());
    }
    Ok(out)
}

/// TCP connect scan. Only scan devices you own or are allowed to test.
pub fn port_scan(host: String, ports: Vec<u16>, tx: Sender<PortMsg>, ctx: eframe::egui::Context) {
    let ip: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => match std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), 80)).ok().and_then(|mut a| a.next()) {
            Some(a) => a.ip(),
            None => {
                let _ = tx.send(PortMsg::Done(format!("Can't resolve \"{host}\".")));
                ctx.request_repaint();
                return;
            }
        },
    };
    let total = ports.len();
    let queue = Arc::new(Mutex::new(ports));
    let done = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..128 {
        let (queue, done, open, tx, ctx) = (queue.clone(), done.clone(), open.clone(), tx.clone(), ctx.clone());
        handles.push(std::thread::spawn(move || {
            loop {
                let Some(port) = queue.lock().unwrap().pop() else { break };
                let start = std::time::Instant::now();
                if TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_millis(600)).is_ok() {
                    open.fetch_add(1, Ordering::Relaxed);
                    let _ = tx.send(PortMsg::Open(port, start.elapsed().as_millis() as u32));
                }
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 32 == 0 || n == total {
                    let _ = tx.send(PortMsg::Progress(n, total));
                    ctx.request_repaint();
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let _ = tx.send(PortMsg::Done(format!("{} open port(s) on {ip} out of {total} checked.", open.load(Ordering::Relaxed))));
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_specs() {
        assert_eq!(parse_ports("80, 443,8000-8002").unwrap(), vec![80, 443, 8000, 8001, 8002]);
        assert!(parse_ports("0").is_err());
        assert!(parse_ports("10-5").is_err());
        assert!(parse_ports("abc").is_err());
    }

    #[test]
    fn subnet_hosts() {
        let h = hosts_for("192.168.1.42".parse().unwrap(), 24);
        assert_eq!(h.len(), 254);
        assert_eq!(h[0], "192.168.1.1".parse::<Ipv4Addr>().unwrap());
        // /16 networks are limited to the local /24.
        assert_eq!(hosts_for("10.1.2.3".parse().unwrap(), 16).len(), 254);
    }

    #[test]
    fn finds_an_open_local_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        port_scan("127.0.0.1".into(), vec![port], tx, eframe::egui::Context::default());
        let open: Vec<u16> = rx.try_iter().filter_map(|m| if let PortMsg::Open(p, _) = m { Some(p) } else { None }).collect();
        assert_eq!(open, vec![port]);
    }

    #[test]
    fn reads_adapters_and_pings_loopback() {
        assert!(!adapters().is_empty());
        assert!(ping(Ipv4Addr::LOCALHOST, 1000).is_some());
    }
}

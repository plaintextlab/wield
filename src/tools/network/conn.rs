//! Live TCP/UDP connection tables with owning processes, and per-connection traffic.
//!
//! Traffic comes from the TCP extended statistics (`TcpConnectionEstatsData`), which Windows
//! only collects once enabled per connection and only lets administrators enable. UDP has no
//! per-socket counters, so UDP shows up as sockets without byte counts.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, IN6_ADDR, IN6_ADDR_0};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Tcp => "TCP",
            Proto::Udp => "UDP",
        }
    }
}

/// Raw table values kept so a connection can be addressed again (stats, close).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawRow {
    V4 { local_addr: u32, local_port: u32, remote_addr: u32, remote_port: u32 },
    V6 { local: [u8; 16], local_scope: u32, local_port: u32, remote: [u8; 16], remote_scope: u32, remote_port: u32 },
    Udp,
}

#[derive(Clone, Debug)]
pub struct Conn {
    pub proto: Proto,
    pub local: SocketAddr,
    pub remote: Option<SocketAddr>,
    pub state: u32,
    pub pid: u32,
    pub raw: RawRow,
    /// Bytes per second over the last sample (TCP + admin only).
    pub rate_in: f64,
    pub rate_out: f64,
}

pub const STATE_LISTEN: u32 = 2;
pub const STATE_ESTABLISHED: u32 = 5;

pub fn state_label(proto: Proto, s: u32) -> &'static str {
    if proto == Proto::Udp {
        return "Bound";
    }
    match s {
        1 => "Closed",
        2 => "Listening",
        3 => "SYN sent",
        4 => "SYN received",
        5 => "Established",
        6 => "FIN wait 1",
        7 => "FIN wait 2",
        8 => "Close wait",
        9 => "Closing",
        10 => "Last ACK",
        11 => "Time wait",
        12 => "Delete TCB",
        _ => "Unknown",
    }
}

fn port(p: u32) -> u16 {
    u16::from_be(p as u16)
}

fn v4(a: u32) -> Ipv4Addr {
    Ipv4Addr::from(u32::from_be(a))
}

/// Calls a GetExtended*Table function with a growing buffer.
fn fetch(get: impl Fn(Option<*mut core::ffi::c_void>, &mut u32) -> u32) -> Option<Vec<u64>> {
    let mut size = 0u32;
    get(None, &mut size);
    for _ in 0..4 {
        let mut buf = vec![0u64; (size as usize).div_ceil(8) + 16];
        let mut sz = (buf.len() * 8) as u32;
        match get(Some(buf.as_mut_ptr() as *mut _), &mut sz) {
            0 => return Some(buf),
            122 => size = sz, // ERROR_INSUFFICIENT_BUFFER: table grew, retry
            _ => return None,
        }
    }
    None
}

pub fn connections() -> Vec<Conn> {
    let mut out = Vec::new();
    unsafe {
        if let Some(buf) = fetch(|p, s| GetExtendedTcpTable(p, s, false, AF_INET.0 as u32, TCP_TABLE_OWNER_PID_ALL, 0)) {
            let t = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
            for r in std::slice::from_raw_parts(t.table.as_ptr(), t.dwNumEntries as usize) {
                let listening = r.dwState == STATE_LISTEN;
                out.push(Conn {
                    proto: Proto::Tcp,
                    local: SocketAddr::new(IpAddr::V4(v4(r.dwLocalAddr)), port(r.dwLocalPort)),
                    remote: (!listening).then(|| SocketAddr::new(IpAddr::V4(v4(r.dwRemoteAddr)), port(r.dwRemotePort))),
                    state: r.dwState,
                    pid: r.dwOwningPid,
                    raw: RawRow::V4 { local_addr: r.dwLocalAddr, local_port: r.dwLocalPort, remote_addr: r.dwRemoteAddr, remote_port: r.dwRemotePort },
                    rate_in: 0.0,
                    rate_out: 0.0,
                });
            }
        }
        if let Some(buf) = fetch(|p, s| GetExtendedTcpTable(p, s, false, AF_INET6.0 as u32, TCP_TABLE_OWNER_PID_ALL, 0)) {
            let t = &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
            for r in std::slice::from_raw_parts(t.table.as_ptr(), t.dwNumEntries as usize) {
                let listening = r.dwState == STATE_LISTEN;
                out.push(Conn {
                    proto: Proto::Tcp,
                    local: SocketAddr::new(IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)), port(r.dwLocalPort)),
                    remote: (!listening).then(|| SocketAddr::new(IpAddr::V6(Ipv6Addr::from(r.ucRemoteAddr)), port(r.dwRemotePort))),
                    state: r.dwState,
                    pid: r.dwOwningPid,
                    raw: RawRow::V6 {
                        local: r.ucLocalAddr,
                        local_scope: r.dwLocalScopeId,
                        local_port: r.dwLocalPort,
                        remote: r.ucRemoteAddr,
                        remote_scope: r.dwRemoteScopeId,
                        remote_port: r.dwRemotePort,
                    },
                    rate_in: 0.0,
                    rate_out: 0.0,
                });
            }
        }
        if let Some(buf) = fetch(|p, s| GetExtendedUdpTable(p, s, false, AF_INET.0 as u32, UDP_TABLE_OWNER_PID, 0)) {
            let t = &*(buf.as_ptr() as *const MIB_UDPTABLE_OWNER_PID);
            for r in std::slice::from_raw_parts(t.table.as_ptr(), t.dwNumEntries as usize) {
                out.push(Conn {
                    proto: Proto::Udp,
                    local: SocketAddr::new(IpAddr::V4(v4(r.dwLocalAddr)), port(r.dwLocalPort)),
                    remote: None,
                    state: 0,
                    pid: r.dwOwningPid,
                    raw: RawRow::Udp,
                    rate_in: 0.0,
                    rate_out: 0.0,
                });
            }
        }
        if let Some(buf) = fetch(|p, s| GetExtendedUdpTable(p, s, false, AF_INET6.0 as u32, UDP_TABLE_OWNER_PID, 0)) {
            let t = &*(buf.as_ptr() as *const MIB_UDP6TABLE_OWNER_PID);
            for r in std::slice::from_raw_parts(t.table.as_ptr(), t.dwNumEntries as usize) {
                out.push(Conn {
                    proto: Proto::Udp,
                    local: SocketAddr::new(IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)), port(r.dwLocalPort)),
                    remote: None,
                    state: 0,
                    pid: r.dwOwningPid,
                    raw: RawRow::Udp,
                    rate_in: 0.0,
                    rate_out: 0.0,
                });
            }
        }
    }
    out
}

fn v4_row(raw: &RawRow, state: i32) -> Option<MIB_TCPROW_LH> {
    match *raw {
        RawRow::V4 { local_addr, local_port, remote_addr, remote_port } => Some(MIB_TCPROW_LH {
            Anonymous: MIB_TCPROW_LH_0 { dwState: state as u32 },
            dwLocalAddr: local_addr,
            dwLocalPort: local_port,
            dwRemoteAddr: remote_addr,
            dwRemotePort: remote_port,
        }),
        _ => None,
    }
}

fn v6_row(raw: &RawRow) -> Option<MIB_TCP6ROW> {
    match *raw {
        RawRow::V6 { local, local_scope, local_port, remote, remote_scope, remote_port } => Some(MIB_TCP6ROW {
            State: MIB_TCP_STATE(STATE_ESTABLISHED as i32),
            LocalAddr: IN6_ADDR { u: IN6_ADDR_0 { Byte: local } },
            dwLocalScopeId: local_scope,
            dwLocalPort: local_port,
            RemoteAddr: IN6_ADDR { u: IN6_ADDR_0 { Byte: remote } },
            dwRemoteScopeId: remote_scope,
            dwRemotePort: remote_port,
        }),
        _ => None,
    }
}

/// Turns on byte counting for a connection. Returns false without administrator rights.
fn enable_stats(raw: &RawRow) -> bool {
    let rw = TCP_ESTATS_DATA_RW_v0 { EnableCollection: true };
    let bytes = unsafe { std::slice::from_raw_parts(&rw as *const _ as *const u8, size_of::<TCP_ESTATS_DATA_RW_v0>()) };
    unsafe {
        if let Some(row) = v4_row(raw, STATE_ESTABLISHED as i32) {
            SetPerTcpConnectionEStats(&row, TcpConnectionEstatsData, bytes, 0, 0) == 0
        } else if let Some(row) = v6_row(raw) {
            SetPerTcp6ConnectionEStats(&row, TcpConnectionEstatsData, bytes, 0, 0) == 0
        } else {
            false
        }
    }
}

/// Cumulative (bytes in, bytes out) since counting was enabled.
fn read_stats(raw: &RawRow) -> Option<(u64, u64)> {
    let mut rod = vec![0u8; size_of::<TCP_ESTATS_DATA_ROD_v0>()];
    let rc = unsafe {
        if let Some(row) = v4_row(raw, STATE_ESTABLISHED as i32) {
            GetPerTcpConnectionEStats(&row, TcpConnectionEstatsData, None, 0, None, 0, Some(&mut rod), 0)
        } else if let Some(row) = v6_row(raw) {
            GetPerTcp6ConnectionEStats(&row, TcpConnectionEstatsData, None, 0, None, 0, Some(&mut rod), 0)
        } else {
            return None;
        }
    };
    if rc != 0 {
        return None;
    }
    let d = unsafe { &*(rod.as_ptr() as *const TCP_ESTATS_DATA_ROD_v0) };
    Some((d.DataBytesIn, d.DataBytesOut))
}

/// Closes a TCP connection (IPv4 only; Windows offers no API for IPv6). Needs administrator rights.
pub fn close(raw: &RawRow) -> Result<(), String> {
    let row = v4_row(raw, MIB_TCP_STATE_DELETE_TCB.0).ok_or("Only IPv4 TCP connections can be closed.")?;
    match unsafe { SetTcpEntry(&row) } {
        0 => Ok(()),
        5 | 317 => Err("Access denied. Closing connections needs administrator rights.".into()),
        e => Err(format!("SetTcpEntry failed (error {e}).")),
    }
}

/// Sum of bytes received/sent on all physical interfaces.
fn interface_totals() -> (u64, u64) {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return (0, 0);
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let mut seen = HashSet::new();
        let (mut i, mut o) = (0u64, 0u64);
        for r in rows {
            // Skip loopback (24) and the many filter/virtual duplicates: count each LUID's
            // hardware interface once.
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 1 != 0;
            if r.Type == 24 || !hardware || !seen.insert(r.InterfaceIndex) {
                continue;
            }
            i += r.InOctets;
            o += r.OutOctets;
        }
        FreeMibTable(table as *const _);
        (i, o)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcNet {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
    pub tcp: usize,
    pub udp: usize,
    pub listening: usize,
    pub rate_in: f64,
    pub rate_out: f64,
    /// Bytes since Wield started watching (TCP only).
    pub total_in: u64,
    pub total_out: u64,
    pub remotes: Vec<IpAddr>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub conns: Vec<Conn>,
    pub procs: Vec<ProcNet>,
    /// Whole-PC throughput in bytes/second.
    pub total_rate_in: f64,
    pub total_rate_out: f64,
    /// True when per-connection traffic could be measured (administrator).
    pub traffic_available: bool,
}

/// Keeps state between samples to turn cumulative counters into rates.
pub struct Sampler {
    sys: System,
    enabled: HashSet<RawRow>,
    last_conn: HashMap<RawRow, (u64, u64)>,
    proc_totals: HashMap<u32, (u64, u64)>,
    last_if: Option<(u64, u64, std::time::Instant)>,
    last_tick: std::time::Instant,
    stats_denied: bool,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            sys: System::new(),
            enabled: HashSet::new(),
            last_conn: HashMap::new(),
            proc_totals: HashMap::new(),
            last_if: None,
            last_tick: std::time::Instant::now(),
            stats_denied: false,
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().max(0.2);
        self.last_tick = now;

        let mut conns = connections();
        let mut live = HashSet::new();
        for c in conns.iter_mut().filter(|c| c.proto == Proto::Tcp && c.state == STATE_ESTABLISHED) {
            if self.stats_denied {
                break;
            }
            if !self.enabled.contains(&c.raw) {
                if enable_stats(&c.raw) {
                    self.enabled.insert(c.raw);
                } else if !matches!(c.raw, RawRow::Udp) {
                    // Without admin rights every call fails the same way; stop trying.
                    self.stats_denied = !crate::win::elevation::is_elevated();
                    continue;
                }
            }
            if let Some((bin, bout)) = read_stats(&c.raw) {
                live.insert(c.raw);
                let (pin, pout) = self.last_conn.insert(c.raw, (bin, bout)).unwrap_or((0, 0));
                let (din, dout) = (bin.saturating_sub(pin), bout.saturating_sub(pout));
                c.rate_in = din as f64 / dt;
                c.rate_out = dout as f64 / dt;
                let t = self.proc_totals.entry(c.pid).or_default();
                t.0 += din;
                t.1 += dout;
            }
        }
        self.last_conn.retain(|k, _| live.contains(k));
        self.enabled.retain(|k| live.contains(k) || conns.iter().any(|c| c.raw == *k));

        // Process names.
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );
        let mut procs: HashMap<u32, ProcNet> = HashMap::new();
        for c in &conns {
            let p = procs.entry(c.pid).or_insert_with(|| {
                let (name, exe) = match c.pid {
                    0 => ("System Idle Process".to_string(), None),
                    4 => ("System".to_string(), None),
                    pid => self
                        .sys
                        .process(Pid::from_u32(pid))
                        .map(|p| (p.name().to_string_lossy().to_string(), p.exe().map(|e| e.to_path_buf())))
                        .unwrap_or_else(|| (format!("PID {pid}"), None)),
                };
                ProcNet { pid: c.pid, name, exe, ..Default::default() }
            });
            match c.proto {
                Proto::Tcp => p.tcp += 1,
                Proto::Udp => p.udp += 1,
            }
            if c.state == STATE_LISTEN || (c.proto == Proto::Udp) {
                p.listening += usize::from(c.state == STATE_LISTEN);
            }
            p.rate_in += c.rate_in;
            p.rate_out += c.rate_out;
            if let Some(r) = c.remote {
                if !r.ip().is_unspecified() && !p.remotes.contains(&r.ip()) {
                    p.remotes.push(r.ip());
                }
            }
        }
        for p in procs.values_mut() {
            if let Some((i, o)) = self.proc_totals.get(&p.pid) {
                p.total_in = *i;
                p.total_out = *o;
            }
        }
        let mut procs: Vec<ProcNet> = procs.into_values().collect();
        let internet = |p: &ProcNet| p.remotes.iter().filter(|r| !is_local(r)).count();
        procs.sort_by(|a, b| {
            (b.rate_in + b.rate_out)
                .total_cmp(&(a.rate_in + a.rate_out))
                .then((b.total_in + b.total_out).cmp(&(a.total_in + a.total_out)))
                .then(internet(b).cmp(&internet(a)))
                .then(b.tcp.cmp(&a.tcp))
        });

        let (ti, to) = interface_totals();
        let (rate_in, rate_out) = match self.last_if {
            Some((pi, po, t)) => {
                let d = now.duration_since(t).as_secs_f64().max(0.2);
                (ti.saturating_sub(pi) as f64 / d, to.saturating_sub(po) as f64 / d)
            }
            None => (0.0, 0.0),
        };
        self.last_if = Some((ti, to, now));

        Snapshot { conns, procs, total_rate_in: rate_in, total_rate_out: rate_out, traffic_available: !self.stats_denied }
    }
}

/// True for loopback, private, link-local and other non-internet addresses.
pub fn is_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback() || a.is_private() || a.is_link_local() || a.is_unspecified() || a.is_broadcast(),
        IpAddr::V6(a) => {
            a.is_loopback() || a.is_unspecified() || (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tables() {
        let c = connections();
        assert!(c.iter().any(|c| c.proto == Proto::Tcp), "some TCP socket always exists");
        let mut s = Sampler::new();
        let snap = s.sample();
        assert!(!snap.procs.is_empty());
    }

    #[test]
    fn local_addresses() {
        assert!(is_local(&"192.168.1.5".parse().unwrap()));
        assert!(is_local(&"::1".parse().unwrap()));
        assert!(!is_local(&"8.8.8.8".parse().unwrap()));
    }
}

//! Network Toolkit screen: Activity, Ports, Firewall, Scanner and Tools tabs.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui, pos2, vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::conn::{self, Conn, ProcNet, Proto, Sampler, Snapshot};
use super::firewall::{self, RuleKind, WieldRule};
use super::intel::{self, Resolver};
use super::scan::{self, Adapter, Device, PortMsg, ScanMsg};
use super::sysnet::{self, HostsEntry, Repair};
use crate::theme;
use crate::toast::Toasts;
use crate::win::fileinfo::format_bytes;
use crate::win::{elevation, shell, with_com};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Activity,
    Ports,
    Firewall,
    Scanner,
    Tools,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Activity, Tab::Ports, Tab::Firewall, Tab::Scanner, Tab::Tools];
    fn label(self) -> (&'static str, &'static str) {
        match self {
            Tab::Activity => (icon::PULSE, "Activity"),
            Tab::Ports => (icon::PLUGS, "Ports"),
            Tab::Firewall => (icon::SHIELD_CHECK, "Firewall"),
            Tab::Scanner => (icon::BROADCAST, "Scanner"),
            Tab::Tools => (icon::WRENCH, "Tools"),
        }
    }
}

fn rate(b: f64) -> String {
    if b < 1.0 { "—".into() } else { format!("{}/s", format_bytes(b as u64)) }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A pending block that needs confirmation because it targets a Windows component.
struct ConfirmBlock {
    path: String,
    name: String,
}

pub struct NetworkManager {
    tab: Tab,
    // Activity
    snap_rx: Option<Receiver<Snapshot>>,
    last_view: Arc<AtomicU64>,
    snap: Option<Snapshot>,
    history: VecDeque<(f64, f64)>,
    resolver: Option<Resolver>,
    selected_pid: Option<u32>,
    search: String,
    only_active: bool,
    confirm_block: Option<ConfirmBlock>,
    // Firewall state (refreshed on demand)
    blocked: HashSet<String>,
    rules: Vec<WieldRule>,
    fw_on: Option<bool>,
    fw_loaded: bool,
    pick_rx: Option<Receiver<Option<std::path::PathBuf>>>,
    add_path: String,
    open_proto_udp: bool,
    open_ports: String,
    open_label: String,
    // Ports
    ports_search: String,
    only_exposed: bool,
    // Scanner
    devices: BTreeMap<Ipv4Addr, Device>,
    lan_rx: Option<Receiver<ScanMsg>>,
    lan_progress: (usize, usize),
    lan_msg: String,
    port_host: String,
    port_spec: String,
    port_rx: Option<Receiver<PortMsg>>,
    port_results: Vec<(u16, u32)>,
    port_progress: (usize, usize),
    port_msg: String,
    // Tools
    adapters: Vec<Adapter>,
    dns_choice: BTreeMap<u32, usize>,
    hosts: Vec<HostsEntry>,
    hosts_error: Option<String>,
    block_domain: String,
    ping_host: String,
    ping_rx: Option<Receiver<String>>,
    ping_log: Vec<String>,
    public_ip: Option<String>,
    ip_rx: Option<Receiver<Result<String, String>>>,
    job_rx: Option<Receiver<(String, Result<String, String>)>>,
    confirm_repair: Option<Repair>,
    tools_loaded: bool,
}

impl NetworkManager {
    pub fn new() -> Self {
        Self {
            tab: Tab::Activity,
            snap_rx: None,
            last_view: Arc::new(AtomicU64::new(0)),
            snap: None,
            history: VecDeque::new(),
            resolver: None,
            selected_pid: None,
            search: String::new(),
            only_active: false,
            confirm_block: None,
            blocked: HashSet::new(),
            rules: Vec::new(),
            fw_on: None,
            fw_loaded: false,
            pick_rx: None,
            add_path: String::new(),
            open_proto_udp: false,
            open_ports: String::new(),
            open_label: String::new(),
            ports_search: String::new(),
            only_exposed: false,
            devices: BTreeMap::new(),
            lan_rx: None,
            lan_progress: (0, 0),
            lan_msg: String::new(),
            port_host: String::new(),
            port_spec: String::new(),
            port_rx: None,
            port_results: Vec::new(),
            port_progress: (0, 0),
            port_msg: String::new(),
            adapters: Vec::new(),
            dns_choice: BTreeMap::new(),
            hosts: Vec::new(),
            hosts_error: None,
            block_domain: String::new(),
            ping_host: String::new(),
            ping_rx: None,
            ping_log: Vec::new(),
            public_ip: None,
            ip_rx: None,
            job_rx: None,
            confirm_repair: None,
            tools_loaded: false,
        }
    }

    /// Selects a tab by name (used by `--tab` on the command line).
    pub fn set_tab(&mut self, name: &str) {
        self.tab = match name.to_lowercase().as_str() {
            "ports" => Tab::Ports,
            "firewall" => Tab::Firewall,
            "scanner" => Tab::Scanner,
            "tools" => Tab::Tools,
            _ => Tab::Activity,
        };
    }

    pub fn summary_line(&self) -> Option<String> {
        self.snap.as_ref().map(|s| {
            let active = s.procs.iter().filter(|p| p.tcp > 0).count();
            format!("{active} apps online · ↓ {} ↑ {}", rate(s.total_rate_in), rate(s.total_rate_out))
        })
    }

    fn start_sampler(&mut self, ctx: &egui::Context) {
        if self.snap_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let last_view = self.last_view.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let mut sampler = Sampler::new();
            loop {
                // Only sample while the Network Toolkit is on screen.
                if now_ms().saturating_sub(last_view.load(Ordering::Relaxed)) < 3000 {
                    if tx.send(sampler.sample()).is_err() {
                        break;
                    }
                    c.request_repaint();
                }
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
        });
        self.snap_rx = Some(rx);
        self.resolver = Some(Resolver::start(ctx.clone()));
    }

    fn refresh_firewall(&mut self) {
        let (rules, on) = with_com(|| (firewall::wield_rules(), firewall::enabled_for_current_profiles()));
        self.rules = rules.unwrap_or_default();
        self.blocked = self
            .rules
            .iter()
            .filter(|r| r.enabled)
            .filter_map(|r| if let RuleKind::BlockProgram { path } = &r.kind { Some(path.to_lowercase()) } else { None })
            .collect();
        self.fw_on = on.ok();
        self.fw_loaded = true;
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if let Some(rx) = &self.snap_rx {
            let mut latest = None;
            while let Ok(s) = rx.try_recv() {
                latest = Some(s);
            }
            if let Some(s) = latest {
                self.history.push_back((s.total_rate_in, s.total_rate_out));
                while self.history.len() > 90 {
                    self.history.pop_front();
                }
                self.snap = Some(s);
            }
        }
        if let Some(rx) = &self.pick_rx {
            if let Ok(p) = rx.try_recv() {
                self.pick_rx = None;
                if let Some(p) = p {
                    self.add_path = p.display().to_string();
                }
            }
        }
        if let Some(rx) = &self.lan_rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    ScanMsg::Progress(a, b) => self.lan_progress = (a, b),
                    ScanMsg::Device(d) => {
                        let e = self.devices.entry(d.ip).or_insert(d.clone());
                        e.mac = e.mac.clone().or(d.mac);
                        e.latency_ms = e.latency_ms.or(d.latency_ms);
                        e.is_gateway |= d.is_gateway;
                        e.is_self |= d.is_self;
                    }
                    ScanMsg::Done(msg) => {
                        self.lan_msg = msg;
                        self.lan_rx = None;
                        break;
                    }
                }
            }
        }
        if let Some(rx) = &self.port_rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    PortMsg::Progress(a, b) => self.port_progress = (a, b),
                    PortMsg::Open(p, ms) => {
                        self.port_results.push((p, ms));
                        self.port_results.sort();
                    }
                    PortMsg::Done(msg) => {
                        self.port_msg = msg;
                        self.port_rx = None;
                        break;
                    }
                }
            }
        }
        if let Some(rx) = &self.ping_rx {
            while let Ok(line) = rx.try_recv() {
                if line.is_empty() {
                    self.ping_rx = None;
                    break;
                }
                self.ping_log.push(line);
            }
        }
        if let Some(rx) = &self.ip_rx {
            if let Ok(r) = rx.try_recv() {
                self.ip_rx = None;
                match r {
                    Ok(ip) => self.public_ip = Some(ip),
                    Err(e) => toasts.error(ctx, format!("Public IP lookup failed: {e}")),
                }
            }
        }
        if let Some(rx) = &self.job_rx {
            if let Ok((title, r)) = rx.try_recv() {
                self.job_rx = None;
                match r {
                    Ok(out) => toasts.success(ctx, format!("{title}: done. {}", out.lines().last().unwrap_or("").trim())),
                    Err(e) => toasts.error(ctx, format!("{title}: {e}")),
                }
                self.adapters = scan::adapters();
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        self.last_view.store(now_ms(), Ordering::Relaxed);
        self.start_sampler(&ctx);
        if !self.fw_loaded {
            self.refresh_firewall();
        }
        self.poll(&ctx, toasts);
        if self.lan_rx.is_some() || self.port_rx.is_some() || self.ping_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        egui::Panel::top("net_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        match self.tab {
            Tab::Activity => self.activity(ui, elevated, toasts),
            Tab::Ports => {
                egui::CentralPanel::no_frame().show(ui, |ui| pad(ui, |ui| self.ports(ui)));
            }
            Tab::Firewall => {
                egui::CentralPanel::no_frame().show(ui, |ui| scroll_pad(ui, |ui| self.firewall_tab(ui, elevated, toasts)));
            }
            Tab::Scanner => {
                egui::CentralPanel::no_frame().show(ui, |ui| scroll_pad(ui, |ui| self.scanner(ui)));
            }
            Tab::Tools => {
                if !self.tools_loaded {
                    self.adapters = scan::adapters();
                    self.reload_hosts();
                    self.tools_loaded = true;
                }
                egui::CentralPanel::no_frame().show(ui, |ui| scroll_pad(ui, |ui| self.tools(ui, elevated, toasts)));
            }
        }
        self.dialogs(&ctx, elevated, toasts);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::NETWORK).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Network Toolkit");
                ui.label(
                    RichText::new("See which apps use the network and who they talk to, block apps, check ports and scan your network.")
                        .color(theme::text_dim()),
                );
            });
        });
        ui.add_space(6.0);
        if !elevated {
            let mut clicked = false;
            banner(
                ui,
                theme::orange(),
                icon::LOCK,
                "Per-app traffic, blocking apps, closing connections and most repairs need administrator rights.",
                Some(("Restart as administrator", &mut clicked)),
            );
            if clicked {
                relaunch(&ctx, toasts);
            }
        }
        if self.fw_on == Some(false) {
            banner(ui, theme::red(), icon::WARNING, "Windows Firewall is off for your current network, so blocking apps and opening ports has no effect.", None);
        }
        ui.horizontal(|ui| {
            for t in Tab::ALL {
                let (ic, label) = t.label();
                let sel = self.tab == t;
                let text = RichText::new(format!("{ic} {label}")).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                    if t == Tab::Firewall {
                        self.refresh_firewall();
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------------ Activity

    fn activity(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let Some(snap) = self.snap.clone() else {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(theme::accent()));
                    ui.label(RichText::new("Reading network activity…").color(theme::text_dim()));
                });
            });
            return;
        };

        egui::Panel::right("net_details")
            .resizable(true)
            .default_size(380.0)
            .size_range(300.0..=640.0)
            .frame(egui::Frame::new().fill(theme::sidebar()).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.process_details(ui, &snap, elevated, toasts));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 20, right: 12, top: 4, bottom: 12 }).show(ui, |ui| {
                self.throughput_card(ui, &snap);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.search).hint_text(format!("{} Search apps…", icon::MAGNIFYING_GLASS)).desired_width(240.0));
                    ui.checkbox(&mut self.only_active, "Only apps with internet connections");
                });
                ui.add_space(4.0);
                self.process_table(ui, &snap, elevated, toasts);
            });
        });
    }

    fn throughput_card(&self, ui: &mut Ui, snap: &Snapshot) {
        theme::card_frame().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(220.0);
                    ui.label(RichText::new(format!("{} {}", icon::ARROW_DOWN, rate(snap.total_rate_in))).font(FontId::new(20.0, theme::heading_family())).color(theme::green()));
                    ui.label(RichText::new(format!("{} {}", icon::ARROW_UP, rate(snap.total_rate_out))).font(FontId::new(20.0, theme::heading_family())).color(theme::accent()));
                    let online = snap.procs.iter().filter(|p| p.remotes.iter().any(|r| !conn::is_local(r))).count();
                    ui.label(RichText::new(format!("{online} apps talking to the internet · {} sockets", snap.conns.len())).small().color(theme::text_dim()));
                    if !snap.traffic_available {
                        ui.label(RichText::new(format!("{} Per-app speeds need administrator", icon::LOCK)).small().color(theme::orange()));
                    }
                });
                // Sparkline of the last 90 seconds.
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 70.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(6), theme::bg());
                let max = self.history.iter().map(|(a, b)| a.max(*b)).fold(1024.0, f64::max);
                for (series, color) in [(0usize, theme::green()), (1, theme::accent())] {
                    let pts: Vec<egui::Pos2> = self
                        .history
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let val = if series == 0 { v.0 } else { v.1 };
                            let x = rect.right() - (self.history.len() - 1 - i) as f32 * rect.width() / 89.0;
                            pos2(x, rect.bottom() - 4.0 - (val / max) as f32 * (rect.height() - 8.0))
                        })
                        .collect();
                    if pts.len() > 1 {
                        p.add(egui::Shape::line(pts, Stroke::new(1.8, color)));
                    }
                }
                p.text(rect.left_top() + vec2(8.0, 6.0), egui::Align2::LEFT_TOP, format!("peak {}", rate(max)), FontId::proportional(11.0), theme::text_dim());
            });
        });
    }

    fn process_table(&mut self, ui: &mut Ui, snap: &Snapshot, elevated: bool, toasts: &mut Toasts) {
        let q = self.search.to_lowercase();
        let rows: Vec<&ProcNet> = snap
            .procs
            .iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.exe.as_ref().is_some_and(|e| e.to_string_lossy().to_lowercase().contains(&q)))
            .filter(|p| !self.only_active || p.remotes.iter().any(|r| !conn::is_local(r)))
            .collect();
        let resolver = self.resolver.clone();
        let blocked = &self.blocked;
        let selected = self.selected_pid;
        let mut clicked = None;
        let mut toggle: Option<(String, String, bool)> = None;

        TableBuilder::new(ui)
            .id_salt("net_procs")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(48.0))
            .column(Column::remainder().at_least(170.0).clip(true))
            .column(Column::initial(88.0).at_least(70.0))
            .column(Column::initial(88.0).at_least(70.0))
            .column(Column::initial(100.0).at_least(70.0))
            .column(Column::initial(70.0).at_least(50.0))
            .column(Column::initial(150.0).at_least(90.0).clip(true))
            .header(26.0, |mut h| {
                for t in ["NET", "APP", "DOWN", "UP", "SEEN", "SOCKETS", "TALKING TO"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().strong().color(theme::text_dim()));
                    });
                }
            })
            .body(|body| {
                body.rows(44.0, rows.len(), |mut row| {
                    let p = rows[row.index()];
                    row.set_selected(selected == Some(p.pid));
                    let path = p.exe.as_ref().map(|e| e.to_string_lossy().to_string());
                    let is_blocked = path.as_ref().is_some_and(|x| blocked.contains(&x.to_lowercase()));
                    row.col(|ui| {
                        let can = path.is_some();
                        let r = theme::switch(ui, !is_blocked, if is_blocked { theme::red() } else { theme::green() }, can);
                        let r = if !can {
                            r.on_hover_text("No program path (system process or access denied).")
                        } else if is_blocked {
                            r.on_hover_text("Blocked by Wield. Click to allow network access.")
                        } else {
                            r.on_hover_text("Click to block this program's network access.")
                        };
                        if r.clicked() {
                            if let Some(x) = &path {
                                toggle = Some((x.clone(), p.name.clone(), !is_blocked));
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            let mut t = RichText::new(&p.name).strong();
                            if is_blocked {
                                t = t.color(theme::red());
                            }
                            ui.add(Label::new(t).truncate().selectable(false));
                            let sub = path.clone().unwrap_or_else(|| format!("PID {}", p.pid));
                            ui.add(Label::new(RichText::new(sub).small().color(theme::text_dim())).truncate().selectable(false));
                        });
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(rate(p.rate_in)).color(if p.rate_in > 1.0 { theme::green() } else { theme::text_dim() }));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(rate(p.rate_out)).color(if p.rate_out > 1.0 { theme::accent() } else { theme::text_dim() }));
                    });
                    row.col(|ui| {
                        let t = p.total_in + p.total_out;
                        ui.label(RichText::new(if t == 0 { "—".into() } else { format_bytes(t) }).color(theme::text_dim()))
                            .on_hover_text(format!("Since Wield started watching: ↓ {} ↑ {}", format_bytes(p.total_in), format_bytes(p.total_out)));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(format!("{}", p.tcp + p.udp)).color(theme::text_dim()))
                            .on_hover_text(format!("{} TCP, {} UDP, {} listening", p.tcp, p.udp, p.listening));
                    });
                    row.col(|ui| {
                        let internet: Vec<&IpAddr> = p.remotes.iter().filter(|r| !conn::is_local(r)).collect();
                        let flagged = internet.iter().filter_map(|ip| resolver.as_ref().and_then(|r| r.name(**ip))).filter_map(|n| intel::classify_host(&n)).find(|t| t.is_privacy_relevant());
                        if let Some(tag) = flagged {
                            theme::badge(ui, format!("{} {}", icon::EYE, tag.label()), theme::purple());
                        } else if internet.is_empty() {
                            ui.label(RichText::new("local only").small().color(theme::text_dim()));
                        } else {
                            ui.label(RichText::new(format!("{} host(s)", internet.len())).color(theme::text_dim()));
                        }
                    });
                    if row.response().clicked() {
                        clicked = Some(p.pid);
                    }
                });
            });
        if let Some(pid) = clicked {
            self.selected_pid = Some(pid);
        }
        if let Some((path, name, block)) = toggle {
            self.request_block(&path, &name, block, elevated, ui.ctx(), toasts);
        }
    }

    fn request_block(&mut self, path: &str, name: &str, block: bool, elevated: bool, ctx: &egui::Context, toasts: &mut Toasts) {
        if !elevated {
            toasts.error(ctx, "Blocking apps needs administrator rights. Use \"Restart as administrator\".");
            return;
        }
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()).to_lowercase();
        if block && path.to_lowercase().starts_with(&windir) {
            self.confirm_block = Some(ConfirmBlock { path: path.into(), name: name.into() });
            return;
        }
        self.apply_block(path, name, block, ctx, toasts);
    }

    fn apply_block(&mut self, path: &str, name: &str, block: bool, ctx: &egui::Context, toasts: &mut Toasts) {
        let p = path.to_string();
        let res = with_com(move || if block { firewall::block_program(&p) } else { firewall::unblock_program(&p) });
        match res {
            Ok(()) => toasts.success(
                ctx,
                if block {
                    format!("Blocked {name}. New connections are refused; restart the app to drop existing ones.")
                } else {
                    format!("{name} can use the network again.")
                },
            ),
            Err(e) => toasts.error(ctx, format!("{name}: {e}")),
        }
        self.refresh_firewall();
    }

    fn process_details(&mut self, ui: &mut Ui, snap: &Snapshot, elevated: bool, toasts: &mut Toasts) {
        let Some(p) = self.selected_pid.and_then(|pid| snap.procs.iter().find(|p| p.pid == pid)).cloned() else {
            ui.add_space(50.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::CURSOR_CLICK).size(40.0).color(theme::text_dim()));
                ui.add_space(8.0);
                ui.label(RichText::new("Select an app to see every connection,\nwho's on the other end and how much\ndata it moves.").color(theme::text_dim()));
                ui.add_space(16.0);
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(format!("{} About \"what data is shared\"", icon::INFO)).strong());
                    ui.add(Label::new(RichText::new(
                        "Almost all traffic is encrypted, so no tool can read what's inside without breaking that encryption. \
                         Wield shows who each app talks to, how much it sends, and flags known telemetry and ad/tracking servers.",
                    ).small().color(theme::text_dim())).wrap());
                });
            });
            return;
        };
        let conns: Vec<Conn> = snap.conns.iter().filter(|c| c.pid == p.pid).cloned().collect();
        let path = p.exe.as_ref().map(|e| e.to_string_lossy().to_string());
        let is_blocked = path.as_ref().is_some_and(|x| self.blocked.contains(&x.to_lowercase()));
        let resolver = self.resolver.clone();
        let mut close: Option<Conn> = None;
        let mut toggle = false;

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new(&p.name).font(FontId::new(19.0, theme::heading_family())));
            if let Some(x) = &path {
                ui.add(Label::new(RichText::new(x).small().monospace().color(theme::text_dim())).wrap());
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                theme::badge(ui, format!("PID {}", p.pid), theme::text_dim());
                theme::badge(ui, format!("↓ {}", rate(p.rate_in)), theme::green());
                theme::badge(ui, format!("↑ {}", rate(p.rate_out)), theme::accent());
                if is_blocked {
                    theme::badge(ui, format!("{} Blocked", icon::PROHIBIT), theme::red());
                }
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if path.is_some() {
                    let label = if is_blocked { format!("{} Allow network", icon::CHECK_CIRCLE) } else { format!("{} Block network", icon::PROHIBIT) };
                    let color = if is_blocked { theme::green() } else { theme::red() };
                    if theme::danger_button(ui, label, color, true).clicked() {
                        toggle = true;
                    }
                }
                if let Some(x) = &path {
                    if ui.button(format!("{} Open folder", icon::FOLDER_OPEN)).clicked() {
                        shell::reveal_in_explorer(std::path::Path::new(x));
                    }
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Connections ({})", conns.len())).strong());
            if conns.is_empty() {
                ui.label(RichText::new("No open sockets right now.").color(theme::text_dim()));
            }
            let mut sorted = conns.clone();
            sorted.sort_by(|a, b| (b.rate_in + b.rate_out).total_cmp(&(a.rate_in + a.rate_out)).then(a.state.cmp(&b.state)));
            for c in sorted.iter().take(200) {
                egui::Frame::new()
                    .fill(theme::card())
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let host = c.remote.and_then(|r| resolver.as_ref().and_then(|res| res.name(r.ip())));
                        let tag = host.as_deref().and_then(intel::classify_host);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(c.proto.label()).small().strong().color(theme::aqua()));
                            match c.remote {
                                Some(r) => {
                                    let title = host.clone().unwrap_or_else(|| r.ip().to_string());
                                    ui.add(Label::new(RichText::new(title).strong()).truncate());
                                }
                                None => {
                                    ui.label(RichText::new(format!("listening on {}", c.local)).color(theme::text_dim()));
                                }
                            }
                            if let Some(t) = tag {
                                let color = if t.is_privacy_relevant() { theme::purple() } else { theme::text_dim() };
                                theme::badge(ui, t.label(), color);
                            }
                            if c.remote.is_some_and(|r| conn::is_local(&r.ip())) {
                                theme::badge(ui, "Local network", theme::text_dim());
                            }
                        });
                        if let Some(r) = c.remote {
                            let svc = intel::service_name(r.port()).map(|s| format!(" ({s})")).unwrap_or_default();
                            ui.label(RichText::new(format!("{}:{}{svc} · {}", r.ip(), r.port(), conn::state_label(c.proto, c.state))).small().color(theme::text_dim()));
                        }
                        if c.rate_in + c.rate_out > 1.0 {
                            ui.label(RichText::new(format!("↓ {}  ↑ {}", rate(c.rate_in), rate(c.rate_out))).small().color(theme::green()));
                        }
                        let closable = c.proto == Proto::Tcp && c.state == conn::STATE_ESTABLISHED && matches!(c.raw, conn::RawRow::V4 { .. });
                        if closable && ui.add_enabled(elevated, egui::Button::new(RichText::new(format!("{} Close", icon::X_CIRCLE)).small())).on_disabled_hover_text("Needs administrator").clicked() {
                            close = Some(c.clone());
                        }
                    });
            }
        });
        if toggle {
            if let Some(x) = path {
                self.request_block(&x, &p.name, !is_blocked, elevated, ui.ctx(), toasts);
            }
        }
        if let Some(c) = close {
            match conn::close(&c.raw) {
                Ok(()) => toasts.success(ui.ctx(), "Connection closed."),
                Err(e) => toasts.error(ui.ctx(), e),
            }
        }
    }

    // ------------------------------------------------------------------ Ports

    fn ports(&mut self, ui: &mut Ui) {
        let Some(snap) = self.snap.clone() else {
            ui.spinner();
            return;
        };
        let names: std::collections::HashMap<u32, String> = snap.procs.iter().map(|p| (p.pid, p.name.clone())).collect();
        let mut listeners: Vec<&Conn> = snap.conns.iter().filter(|c| c.state == conn::STATE_LISTEN || c.proto == Proto::Udp).collect();
        listeners.sort_by_key(|c| (c.local.port(), c.proto));
        let exposed = |c: &Conn| !c.local.ip().is_loopback();
        let n_exposed = listeners.iter().filter(|c| exposed(c) && c.proto == Proto::Tcp).count();
        let q = self.ports_search.to_lowercase();
        let rows: Vec<&Conn> = listeners
            .into_iter()
            .filter(|c| !self.only_exposed || exposed(c))
            .filter(|c| {
                q.is_empty()
                    || c.local.port().to_string().contains(&q)
                    || names.get(&c.pid).is_some_and(|n| n.to_lowercase().contains(&q))
                    || intel::service_name(c.local.port()).is_some_and(|s| s.to_lowercase().contains(&q))
            })
            .collect();

        ui.horizontal(|ui| {
            theme::badge(ui, format!("{} TCP port(s) reachable from the network", n_exposed), theme::orange());
            ui.add(TextEdit::singleline(&mut self.ports_search).hint_text(format!("{} Port, app or service…", icon::MAGNIFYING_GLASS)).desired_width(220.0));
            ui.checkbox(&mut self.only_exposed, "Only reachable from the network");
        });
        ui.label(
            RichText::new("\"All interfaces\" means other devices can try to connect, if the firewall allows it. \"This PC only\" can't be reached from outside.")
                .small()
                .color(theme::text_dim()),
        );
        ui.add_space(4.0);
        let mut jump: Option<u32> = None;
        TableBuilder::new(ui)
            .id_salt("net_ports")
            .striped(true)
            .resizable(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(70.0))
            .column(Column::initial(60.0))
            .column(Column::initial(170.0).clip(true))
            .column(Column::initial(200.0).clip(true))
            .column(Column::remainder().at_least(150.0).clip(true))
            .header(26.0, |mut h| {
                for t in ["PORT", "PROTO", "SERVICE", "LISTENING ON", "PROCESS"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().strong().color(theme::text_dim()));
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, rows.len(), |mut row| {
                    let c = rows[row.index()];
                    row.col(|ui| {
                        ui.label(RichText::new(c.local.port().to_string()).strong().monospace());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(c.proto.label()).color(theme::aqua()));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(intel::service_name(c.local.port()).unwrap_or("")).color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        let v6 = if c.local.is_ipv6() { " (IPv6)" } else { "" };
                        if c.local.ip().is_loopback() {
                            theme::badge(ui, format!("This PC only{v6}"), theme::green());
                        } else if c.local.ip().is_unspecified() {
                            theme::badge(ui, format!("All interfaces{v6}"), theme::orange());
                        } else {
                            theme::badge(ui, c.local.ip().to_string(), theme::yellow());
                        }
                    });
                    row.col(|ui| {
                        let name = names.get(&c.pid).cloned().unwrap_or_else(|| format!("PID {}", c.pid));
                        if ui.add(Label::new(format!("{name}  (PID {})", c.pid)).sense(Sense::click()).truncate()).on_hover_text("Show in Activity").clicked() {
                            jump = Some(c.pid);
                        }
                    });
                });
            });
        if let Some(pid) = jump {
            self.selected_pid = Some(pid);
            self.tab = Tab::Activity;
        }
    }

    // ------------------------------------------------------------------ Firewall

    fn firewall_tab(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            match self.fw_on {
                Some(true) => theme::badge(ui, format!("{} Windows Firewall is on", icon::SHIELD_CHECK), theme::green()),
                Some(false) => theme::badge(ui, format!("{} Windows Firewall is off", icon::WARNING), theme::red()),
                None => theme::badge(ui, "Firewall status unknown", theme::text_dim()),
            };
            if ui.button(format!("{} Refresh", icon::ARROWS_CLOCKWISE)).clicked() {
                self.refresh_firewall();
            }
            if ui.button(format!("{} Advanced firewall (wf.msc)", icon::ARROW_SQUARE_OUT)).clicked() {
                shell::open("wf.msc");
            }
        });
        ui.add_space(10.0);

        section_title(ui, icon::PROHIBIT, "Blocked programs", "Programs here can't send or receive anything over the network.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.add_path).hint_text(r"C:\Path\to\program.exe").desired_width(ui.available_width() - 260.0));
                if ui.add_enabled(self.pick_rx.is_none(), egui::Button::new(format!("{} Browse…", icon::FOLDER_OPEN))).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(shell::pick_program());
                        c.request_repaint();
                    });
                    self.pick_rx = Some(rx);
                }
                let path = self.add_path.trim().trim_matches('"').to_string();
                if theme::danger_button(ui, format!("{} Block", icon::PROHIBIT), theme::red(), !path.is_empty()).clicked() {
                    if !std::path::Path::new(&path).is_file() {
                        toasts.error(&ctx, "That file doesn't exist.");
                    } else {
                        let name = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        self.request_block(&path, &name, true, elevated, &ctx, toasts);
                        self.add_path.clear();
                    }
                }
            });
            ui.add_space(6.0);
            let blocks: Vec<WieldRule> = self.rules.iter().filter(|r| matches!(r.kind, RuleKind::BlockProgram { .. })).cloned().collect();
            if blocks.is_empty() {
                ui.label(RichText::new("No programs blocked yet. Block one here or with the switch in Activity.").color(theme::text_dim()));
            }
            for r in blocks {
                let RuleKind::BlockProgram { path } = &r.kind else { continue };
                ui.separator();
                ui.horizontal(|ui| {
                    if theme::switch(ui, r.enabled, theme::red(), elevated).on_hover_text("On = blocked").clicked() {
                        let (name, en) = (r.name.clone(), !r.enabled);
                        if let Err(e) = with_com(move || firewall::set_enabled(&name, en)) {
                            toasts.error(&ctx, e);
                        }
                        self.refresh_firewall();
                    }
                    ui.vertical(|ui| {
                        let file = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        ui.label(RichText::new(file).strong());
                        ui.add(Label::new(RichText::new(path).small().monospace().color(theme::text_dim())).truncate());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(elevated, egui::Button::new(format!("{} Remove", icon::TRASH))).clicked() {
                            let name = r.name.clone();
                            match with_com(move || firewall::remove_rule(&name)) {
                                Ok(()) => toasts.success(&ctx, "Rule removed. The program can use the network again."),
                                Err(e) => toasts.error(&ctx, e),
                            }
                            self.refresh_firewall();
                        }
                    });
                });
            }
        });

        ui.add_space(14.0);
        section_title(ui, icon::PLUGS_CONNECTED, "Opened ports", "Let other devices connect to a server on this PC (game server, NAS client, dev server).");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.open_proto_udp, false, "TCP");
                ui.selectable_value(&mut self.open_proto_udp, true, "UDP");
                ui.add(TextEdit::singleline(&mut self.open_ports).hint_text("8080 or 27015-27030").desired_width(170.0));
                ui.add(TextEdit::singleline(&mut self.open_label).hint_text("Label (optional)").desired_width(170.0));
                if theme::primary_button(ui, format!("{} Open port", icon::PLUS)).clicked() {
                    if !elevated {
                        toasts.error(&ctx, "Opening ports needs administrator rights.");
                    } else {
                        let proto = if self.open_proto_udp { 17 } else { 6 };
                        let (ports, label) = (self.open_ports.clone(), self.open_label.clone());
                        match with_com(move || firewall::open_port(proto, &ports, &label)) {
                            Ok(name) => {
                                toasts.success(&ctx, format!("Added \"{name}\"."));
                                self.open_ports.clear();
                                self.open_label.clear();
                            }
                            Err(e) => toasts.error(&ctx, e),
                        }
                        self.refresh_firewall();
                    }
                }
            });
            ui.label(RichText::new("This only opens the Windows Firewall. To reach it from the internet you also need port forwarding on your router.").small().color(theme::text_dim()));
            let opens: Vec<WieldRule> = self.rules.iter().filter(|r| matches!(r.kind, RuleKind::OpenPort { .. })).cloned().collect();
            for r in opens {
                let RuleKind::OpenPort { protocol, ports } = &r.kind else { continue };
                ui.separator();
                ui.horizontal(|ui| {
                    if theme::switch(ui, r.enabled, theme::orange(), elevated).on_hover_text("On = port open").clicked() {
                        let (name, en) = (r.name.clone(), !r.enabled);
                        if let Err(e) = with_com(move || firewall::set_enabled(&name, en)) {
                            toasts.error(&ctx, e);
                        }
                        self.refresh_firewall();
                    }
                    ui.label(RichText::new(format!("{} {ports}", if *protocol == 17 { "UDP" } else { "TCP" })).strong().monospace());
                    ui.label(RichText::new(&r.name).color(theme::text_dim()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(elevated, egui::Button::new(format!("{} Remove", icon::TRASH))).clicked() {
                            let name = r.name.clone();
                            if let Err(e) = with_com(move || firewall::remove_rule(&name)) {
                                toasts.error(&ctx, e);
                            }
                            self.refresh_firewall();
                        }
                    });
                });
            }
        });
    }

    // ------------------------------------------------------------------ Scanner

    fn scanner(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        section_title(ui, icon::DEVICES, "Devices on your network", "Finds phones, TVs, printers and other devices on your local network.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let busy = self.lan_rx.is_some();
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("{} Scan network", icon::BROADCAST)).color(theme::on_accent())).fill(theme::accent())).clicked() {
                    self.devices.clear();
                    self.lan_msg.clear();
                    self.lan_progress = (0, 0);
                    let (tx, rx) = std::sync::mpsc::channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || scan::lan_scan(tx, c));
                    self.lan_rx = Some(rx);
                }
                if busy {
                    let (a, b) = self.lan_progress;
                    ui.add(egui::ProgressBar::new(if b == 0 { 0.0 } else { a as f32 / b as f32 }).desired_width(260.0).fill(theme::accent()).text(format!("{a} / {b}")));
                } else if !self.lan_msg.is_empty() {
                    ui.label(RichText::new(&self.lan_msg).small().color(theme::text_dim()));
                }
            });
            if !self.devices.is_empty() {
                ui.add_space(6.0);
                let resolver = self.resolver.clone();
                let mut target: Option<Ipv4Addr> = None;
                egui::Grid::new("lan_devices").num_columns(5).striped(true).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
                    for t in ["ADDRESS", "NAME", "MAC", "PING", ""] {
                        ui.label(RichText::new(t).small().strong().color(theme::text_dim()));
                    }
                    ui.end_row();
                    for d in self.devices.values() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(d.ip.to_string()).monospace().strong());
                            if d.is_gateway {
                                theme::badge(ui, "Router", theme::yellow());
                            }
                            if d.is_self {
                                theme::badge(ui, "This PC", theme::accent());
                            }
                        });
                        let name = resolver.as_ref().and_then(|r| r.name(IpAddr::V4(d.ip))).unwrap_or_else(|| "—".into());
                        ui.label(name);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(d.mac.clone().unwrap_or_else(|| "—".into())).monospace().color(theme::text_dim()));
                            if d.randomized_mac() {
                                theme::badge(ui, "private MAC", theme::text_dim()).on_hover_text("Randomized address, typical of phones and laptops.");
                            }
                        });
                        ui.label(d.latency_ms.map(|m| format!("{m} ms")).unwrap_or_else(|| "no reply".into()));
                        if ui.small_button(format!("{} Scan ports", icon::PLUGS)).clicked() {
                            target = Some(d.ip);
                        }
                        ui.end_row();
                    }
                });
                if let Some(ip) = target {
                    self.port_host = ip.to_string();
                    if self.port_spec.is_empty() {
                        self.port_spec = "common".into();
                    }
                }
            }
        });

        ui.add_space(14.0);
        section_title(ui, icon::PLUGS, "Port scanner", "Checks which TCP ports accept connections on a device. Only scan devices you own or are allowed to test.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.port_host).hint_text("192.168.1.1 or hostname").desired_width(200.0));
                ui.add(TextEdit::singleline(&mut self.port_spec).hint_text("common, 1-1024 or 80,443").desired_width(200.0));
                if ui.button("Common").clicked() {
                    self.port_spec = "common".into();
                }
                if ui.button("1-1024").clicked() {
                    self.port_spec = "1-1024".into();
                }
                let busy = self.port_rx.is_some();
                if ui.add_enabled(!busy && !self.port_host.trim().is_empty(), egui::Button::new(RichText::new(format!("{} Scan", icon::PLAY)).color(theme::on_accent())).fill(theme::accent())).clicked() {
                    let ports = if self.port_spec.trim().eq_ignore_ascii_case("common") || self.port_spec.trim().is_empty() {
                        Ok(intel::COMMON_PORTS.to_vec())
                    } else {
                        scan::parse_ports(&self.port_spec)
                    };
                    match ports {
                        Ok(ports) => {
                            self.port_results.clear();
                            self.port_msg.clear();
                            self.port_progress = (0, ports.len());
                            let (tx, rx) = std::sync::mpsc::channel();
                            let (host, c) = (self.port_host.trim().to_string(), ctx.clone());
                            std::thread::spawn(move || scan::port_scan(host, ports, tx, c));
                            self.port_rx = Some(rx);
                        }
                        Err(e) => self.port_msg = e,
                    }
                }
            });
            if self.port_rx.is_some() {
                let (a, b) = self.port_progress;
                ui.add(egui::ProgressBar::new(if b == 0 { 0.0 } else { a as f32 / b as f32 }).fill(theme::accent()).text(format!("{a} / {b} ports")));
            } else if !self.port_msg.is_empty() {
                ui.label(RichText::new(&self.port_msg).small().color(theme::text_dim()));
            }
            for (port, ms) in &self.port_results {
                ui.horizontal(|ui| {
                    theme::badge(ui, format!("{port} open"), theme::green());
                    ui.label(intel::service_name(*port).unwrap_or("Unknown service"));
                    ui.label(RichText::new(format!("{ms} ms")).small().color(theme::text_dim()));
                });
            }
        });
    }

    // ------------------------------------------------------------------ Tools

    fn reload_hosts(&mut self) {
        match sysnet::read_hosts() {
            Ok(h) => {
                self.hosts = h;
                self.hosts_error = None;
            }
            Err(e) => self.hosts_error = Some(e),
        }
    }

    fn run_job(&mut self, ctx: &egui::Context, title: &str, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (c, t) = (ctx.clone(), title.to_string());
        std::thread::spawn(move || {
            let _ = tx.send((t, f()));
            c.request_repaint();
        });
        self.job_rx = Some(rx);
    }

    fn tools(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        let busy = self.job_rx.is_some();

        section_title(ui, icon::GLOBE, "Network adapters & DNS", "Your connections, their addresses, and which DNS servers resolve names for you.");
        let adapters = self.adapters.clone();
        for a in adapters.iter().filter(|a| a.up) {
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if a.kind == "Wi-Fi" { icon::WIFI_HIGH } else { icon::PLUGS_CONNECTED }).size(20.0).color(theme::accent()));
                    ui.label(RichText::new(&a.name).strong());
                    theme::badge(ui, a.kind, theme::aqua());
                    if a.speed_bps > 0 {
                        theme::badge(ui, format!("{} Mbps", a.speed_bps / 1_000_000), theme::text_dim());
                    }
                });
                ui.label(RichText::new(&a.description).small().color(theme::text_dim()));
                egui::Grid::new(("adapter", a.index)).num_columns(2).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.label(RichText::new(k).small().color(theme::text_dim()));
                        ui.label(RichText::new(v).monospace());
                        ui.end_row();
                    };
                    row("IPv4", a.ipv4.iter().map(|(ip, p)| format!("{ip}/{p}")).collect::<Vec<_>>().join(", "));
                    if !a.ipv6.is_empty() {
                        row("IPv6", format!("{} address(es)", a.ipv6.len()));
                    }
                    row("Gateway", a.gateways.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(", "));
                    row("DNS", a.dns.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(", "));
                    row("MAC", a.mac.clone());
                });
                ui.horizontal(|ui| {
                    let choice = self.dns_choice.entry(a.index).or_insert(0);
                    egui::ComboBox::from_id_salt(("dns", a.index)).selected_text(sysnet::DNS_PRESETS[*choice].0).show_ui(ui, |ui| {
                        for (i, (label, servers, note)) in sysnet::DNS_PRESETS.iter().enumerate() {
                            ui.selectable_value(choice, i, *label).on_hover_text(format!("{} {}", servers.join(", "), note));
                        }
                    });
                    let idx = *choice;
                    if ui.add_enabled(!busy && elevated, egui::Button::new("Set DNS")).on_disabled_hover_text("Needs administrator").clicked() {
                        let if_index = a.index;
                        let servers: Vec<&'static str> = sysnet::DNS_PRESETS[idx].1.to_vec();
                        self.run_job(&ctx, &format!("DNS for {}", a.name), move || sysnet::set_dns(if_index, &servers).map(|_| String::new()));
                    }
                });
            });
            ui.add_space(6.0);
        }
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(format!("{} Flush DNS cache", icon::BROOM))).clicked() {
                self.run_job(&ctx, "Flush DNS cache", sysnet::flush_dns);
            }
            if ui.add_enabled(self.ip_rx.is_none(), egui::Button::new(format!("{} Show public IP", icon::GLOBE_HEMISPHERE_WEST))).on_hover_text("Asks api.ipify.org").clicked() {
                let (tx, rx) = std::sync::mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(sysnet::public_ip());
                    c.request_repaint();
                });
                self.ip_rx = Some(rx);
            }
            if let Some(ip) = &self.public_ip {
                ui.label(RichText::new(ip).monospace().strong());
                if ui.small_button(icon::COPY).clicked() {
                    ctx.copy_text(ip.clone());
                }
            }
        });

        ui.add_space(14.0);
        section_title(ui, icon::PULSE, "Ping", "Check whether a device or website answers and how fast.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.ping_host).hint_text("1.1.1.1 or example.com").desired_width(240.0));
                if ui.add_enabled(self.ping_rx.is_none() && !self.ping_host.trim().is_empty(), egui::Button::new(format!("{} Ping", icon::PLAY))).clicked() {
                    self.ping_log.clear();
                    let host = self.ping_host.trim().to_string();
                    let (tx, rx) = std::sync::mpsc::channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || {
                        let ip = host.parse::<Ipv4Addr>().ok().or_else(|| {
                            std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), 0)).ok()?.find_map(|a| if let IpAddr::V4(v) = a.ip() { Some(v) } else { None })
                        });
                        match ip {
                            None => {
                                let _ = tx.send(format!("Can't resolve {host} to an IPv4 address."));
                            }
                            Some(ip) => {
                                for i in 1..=4 {
                                    let line = match scan::ping(ip, 2000) {
                                        Some(ms) => format!("Reply from {ip}: {ms} ms"),
                                        None => format!("Request {i} to {ip} timed out"),
                                    };
                                    let _ = tx.send(line);
                                    c.request_repaint();
                                    std::thread::sleep(std::time::Duration::from_millis(400));
                                }
                            }
                        }
                        let _ = tx.send(String::new());
                        c.request_repaint();
                    });
                    self.ping_rx = Some(rx);
                }
            });
            for l in &self.ping_log {
                ui.label(RichText::new(l).monospace().small());
            }
        });

        ui.add_space(14.0);
        section_title(ui, icon::PROHIBIT, "Block websites (hosts file)", "Domains listed here resolve to 0.0.0.0, so no app on this PC can reach them.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.block_domain).hint_text("ads.example.com").desired_width(260.0));
                if ui.add_enabled(elevated && !self.block_domain.trim().is_empty(), egui::Button::new(format!("{} Block domain", icon::PROHIBIT))).on_disabled_hover_text("Needs administrator").clicked() {
                    match sysnet::block_domain(&self.block_domain) {
                        Ok(()) => {
                            toasts.success(&ctx, format!("Blocked {}.", self.block_domain.trim()));
                            self.block_domain.clear();
                            let _ = sysnet::flush_dns();
                        }
                        Err(e) => toasts.error(&ctx, e),
                    }
                    self.reload_hosts();
                }
                if ui.button(format!("{} Open in Notepad", icon::NOTEPAD)).clicked() {
                    let _ = std::process::Command::new("notepad.exe").arg(sysnet::hosts_path()).spawn();
                }
            });
            if let Some(e) = &self.hosts_error {
                ui.label(RichText::new(e).color(theme::red()));
            }
            let entries: Vec<HostsEntry> = self.hosts.iter().filter(|e| !(e.host.eq_ignore_ascii_case("localhost"))).cloned().collect();
            if entries.is_empty() {
                ui.label(RichText::new("The hosts file has no custom entries.").small().color(theme::text_dim()));
            }
            for e in entries {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&e.ip).monospace().color(theme::text_dim()));
                    ui.label(RichText::new(&e.host).monospace());
                    if e.by_wield {
                        theme::badge(ui, "Wield", theme::accent());
                        if ui.add_enabled(elevated, egui::Button::new(RichText::new(format!("{} Unblock", icon::X)).small())).clicked() {
                            match sysnet::unblock_domain(&e.host) {
                                Ok(()) => toasts.success(&ctx, format!("Unblocked {}.", e.host)),
                                Err(err) => toasts.error(&ctx, err),
                            }
                            self.reload_hosts();
                        }
                    }
                });
            }
        });

        ui.add_space(14.0);
        section_title(ui, icon::FIRST_AID_KIT, "Repair", "Fixes for \"connected but no internet\", broken VPN leftovers and DNS problems.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for r in Repair::ALL {
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new(r.title())).clicked() {
                        self.confirm_repair = Some(r);
                    }
                    ui.label(RichText::new(r.blurb()).small().color(theme::text_dim()));
                });
            }
            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Working…");
                });
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, _elevated: bool, toasts: &mut Toasts) {
        if let Some(cb) = &self.confirm_block {
            let (path, name) = (cb.path.clone(), cb.name.clone());
            let mut go = false;
            let mut close = false;
            let m = egui::Modal::new(Id::new("net_confirm_block"))
                .frame(egui::Frame::new().fill(theme::card()).stroke(Stroke::new(2.0, theme::red())).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20)))
                .show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::WARNING_OCTAGON).size(28.0).color(theme::red()));
                        ui.label(RichText::new(format!("Block {name}?")).font(FontId::new(18.0, theme::heading_family())));
                    });
                    ui.add_space(6.0);
                    ui.add(Label::new(format!(
                        "{path} is part of Windows. Blocking it can stop Windows Update, Microsoft Store, time sync or networking itself — \
                         svchost.exe, for example, hosts dozens of services at once."
                    )).wrap());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if theme::danger_button(ui, "Block anyway", theme::red(), true).clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if go {
                self.confirm_block = None;
                self.apply_block(&path, &name, true, ctx, toasts);
            } else if close || m.should_close() {
                self.confirm_block = None;
            }
        }

        if let Some(r) = self.confirm_repair {
            let mut go = false;
            let mut close = false;
            let m = egui::Modal::new(Id::new("net_repair")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(r.title()).font(FontId::new(18.0, theme::heading_family())));
                ui.add_space(6.0);
                ui.add(Label::new(r.blurb()).wrap());
                if r.needs_restart() {
                    ui.label(RichText::new("Restart Windows afterwards for this to take effect.").color(theme::orange()));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::danger_button(ui, "Run", theme::orange(), true).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if go {
                self.confirm_repair = None;
                self.run_job(ctx, r.title(), move || r.run());
            } else if close || m.should_close() {
                self.confirm_repair = None;
            }
        }
    }
}

fn pad(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 12 }).show(ui, f);
}

fn scroll_pad(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| pad(ui, f));
}

fn section_title(ui: &mut Ui, ic: &str, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(20.0).color(theme::accent()));
        ui.label(RichText::new(title).font(FontId::new(17.0, theme::heading_family())));
    });
    ui.label(RichText::new(blurb).color(theme::text_dim()));
    ui.add_space(6.0);
}

fn relaunch(ctx: &egui::Context, toasts: &mut Toasts) {
    match elevation::relaunch_as_admin() {
        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        Err(e) => toasts.error(ctx, e),
    }
}

fn banner(ui: &mut Ui, color: Color32, ic: &str, text: &str, button: Option<(&str, &mut bool)>) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic).color(color).size(16.0));
                ui.add(Label::new(text).wrap());
                if let Some((label, clicked)) = button {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        *clicked = ui.button(label).clicked();
                    });
                }
            });
        });
    ui.add_space(6.0);
}

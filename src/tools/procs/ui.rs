//! Process Explorer screen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui, pos2, vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::control;
use super::inspect::{self, Locker, Module};
use super::snapshot::{self, Kind, Proc, Sampler, Snapshot};
use crate::theme;
use crate::toast::Toasts;
use crate::tools::malware::inspect::{self as sig, Signature};
use crate::tools::network::{conn, firewall};
use crate::tools::startup::journal;
use crate::win::fileinfo::format_bytes;
use crate::win::{elevation, shell, with_com};

type Key = (u32, u64);

/// Requests for other tools, handled by the app shell.
pub enum Intent {
    Scan(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Tree,
    List,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Kind(Kind),
    Suspended,
    Elevated,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Name,
    Pid,
    Cpu,
    Memory,
    Io,
    Threads,
    Handles,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Overview,
    Performance,
    Files,
    Modules,
    Network,
}

struct Confirm {
    key: Key,
    name: String,
    tree: bool,
    warning: Option<&'static str>,
}

struct LockFinder {
    paths: Vec<PathBuf>,
    result: Option<Result<Vec<Locker>, String>>,
    pick_rx: Option<Receiver<Vec<PathBuf>>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn rate(b: f64) -> String {
    if b < 1.0 { "—".into() } else { format!("{}/s", format_bytes(b as u64)) }
}

fn duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {}s", secs % 60)
    }
}

pub struct ProcessManager {
    rx: Option<Receiver<Snapshot>>,
    last_view: Arc<AtomicU64>,
    snap: Option<Snapshot>,
    history: VecDeque<(f32, f32, f64)>,
    proc_history: HashMap<Key, VecDeque<(f32, u64)>>,
    search: String,
    view: View,
    filter: Filter,
    sort: (SortCol, bool),
    collapsed: HashSet<Key>,
    selected: Option<Key>,
    tab: DetailTab,
    files: Option<(Key, Result<Vec<String>, String>)>,
    files_rx: Option<Receiver<(Key, Result<Vec<String>, String>)>>,
    files_filter: String,
    modules: Option<(Key, Result<Vec<Module>, String>)>,
    modules_filter: String,
    signature: HashMap<PathBuf, Option<Signature>>,
    sig_rx: Option<Receiver<(PathBuf, Signature)>>,
    efficiency_set: HashSet<Key>,
    confirm: Option<Confirm>,
    affinity: Option<(Key, String, usize, usize)>,
    locks: Option<LockFinder>,
    intent: Option<Intent>,
    pending_pid: Option<u32>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            rx: None,
            last_view: Arc::new(AtomicU64::new(0)),
            snap: None,
            history: VecDeque::new(),
            proc_history: HashMap::new(),
            search: String::new(),
            view: View::Tree,
            filter: Filter::All,
            sort: (SortCol::Cpu, true),
            collapsed: HashSet::new(),
            selected: None,
            tab: DetailTab::Overview,
            files: None,
            files_rx: None,
            files_filter: String::new(),
            modules: None,
            modules_filter: String::new(),
            signature: HashMap::new(),
            sig_rx: None,
            efficiency_set: HashSet::new(),
            confirm: None,
            affinity: None,
            locks: None,
            intent: None,
            pending_pid: None,
        }
    }

    /// Selects a process by PID once it appears (used by `--pid`).
    pub fn select_pid(&mut self, pid: u32) {
        self.pending_pid = Some(pid);
    }

    pub fn take_intent(&mut self) -> Option<Intent> {
        self.intent.take()
    }

    pub fn summary_line(&self) -> Option<String> {
        self.snap.as_ref().map(|s| {
            format!(
                "{} processes · CPU {:.0}% · RAM {:.0}%",
                s.totals.processes,
                s.totals.cpu_pct,
                s.totals.mem_used as f64 / s.totals.mem_total.max(1) as f64 * 100.0
            )
        })
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let (last, c) = (self.last_view.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut s = Sampler::new();
            loop {
                if now_ms().saturating_sub(last.load(Ordering::Relaxed)) < 3000 {
                    if tx.send(s.sample()).is_err() {
                        break;
                    }
                    c.request_repaint();
                }
                std::thread::sleep(std::time::Duration::from_millis(1500));
            }
        });
        self.rx = Some(rx);
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            let mut latest = None;
            while let Ok(s) = rx.try_recv() {
                latest = Some(s);
            }
            if let Some(s) = latest {
                let t = &s.totals;
                self.history.push_back((t.cpu_pct, t.mem_used as f32 / t.mem_total.max(1) as f32 * 100.0, t.read_rate + t.write_rate));
                while self.history.len() > 80 {
                    self.history.pop_front();
                }
                let live: HashSet<Key> = s.procs.iter().map(|p| p.key()).collect();
                self.proc_history.retain(|k, _| live.contains(k));
                self.efficiency_set.retain(|k| live.contains(k));
                for p in &s.procs {
                    let h = self.proc_history.entry(p.key()).or_default();
                    h.push_back((p.cpu_pct, p.private_bytes));
                    while h.len() > 80 {
                        h.pop_front();
                    }
                }
                if let Some(pid) = self.pending_pid.take() {
                    self.selected = s.procs.iter().find(|p| p.pid == pid).map(|p| p.key());
                }
                self.snap = Some(s);
            }
        }
        if let Some(rx) = &self.files_rx {
            if let Ok(r) = rx.try_recv() {
                self.files = Some(r);
                self.files_rx = None;
            }
        }
        if let Some(rx) = &self.sig_rx {
            if let Ok((p, s)) = rx.try_recv() {
                self.signature.insert(p, Some(s));
                self.sig_rx = None;
            }
        }
        if let Some(l) = &mut self.locks {
            if let Some(rx) = &l.pick_rx {
                if let Ok(paths) = rx.try_recv() {
                    l.pick_rx = None;
                    if !paths.is_empty() {
                        l.result = Some(inspect::who_locks(&paths));
                        l.paths = paths;
                    }
                }
            }
        }
    }

    fn selected_proc(&self) -> Option<Proc> {
        let key = self.selected?;
        self.snap.as_ref()?.procs.iter().find(|p| p.key() == key).cloned()
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        self.last_view.store(now_ms(), Ordering::Relaxed);
        self.start(&ctx);
        self.poll();

        egui::Panel::top("procs_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        let Some(snap) = self.snap.clone() else {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(theme::accent()));
                });
            });
            return;
        };

        egui::Panel::right("procs_details")
            .resizable(true)
            .default_size(380.0)
            .size_range(320.0..=720.0)
            .frame(egui::Frame::new().fill(theme::sidebar()).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.details(ui, &snap, elevated, toasts));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 20, right: 12, top: 4, bottom: 12 }).show(ui, |ui| {
                self.toolbar(ui, &snap);
                ui.add_space(4.0);
                self.table(ui, &snap);
            });
        });
        self.dialogs(&ctx, &snap, toasts);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CPU).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Process Explorer");
                ui.label(RichText::new("Every running process: what it uses, what it has open, who started it, and full control over it.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(format!("{} Who is locking a file?", icon::LOCK_SIMPLE)).clicked() {
                    self.locks = Some(LockFinder { paths: Vec::new(), result: None, pick_rx: None });
                }
            });
        });
        ui.add_space(6.0);
        if !elevated {
            let mut clicked = false;
            banner(ui, theme::orange(), icon::LOCK, "System and other users' processes can be viewed but not changed, and their files and modules stay hidden, until you run as administrator.", Some(("Restart as administrator", &mut clicked)));
            if clicked {
                relaunch(&ctx, toasts);
            }
        }
        let Some(s) = &self.snap else { return };
        let t = s.totals.clone();
        let hist: Vec<(f32, f32, f64)> = self.history.iter().copied().collect();
        ui.columns(4, |cols| {
            tile(&mut cols[0], icon::CPU, "CPU", &format!("{:.0}%", t.cpu_pct), theme::accent(), hist.iter().map(|h| h.0 as f64).collect(), Some(100.0));
            let pct = t.mem_used as f64 / t.mem_total.max(1) as f64 * 100.0;
            tile(
                &mut cols[1],
                icon::MEMORY,
                &format!("Memory · {} / {}", format_bytes(t.mem_used), format_bytes(t.mem_total)),
                &format!("{pct:.0}%"),
                theme::purple(),
                hist.iter().map(|h| h.1 as f64).collect(),
                Some(100.0),
            );
            tile(&mut cols[2], icon::HARD_DRIVES, "I/O (read + write)", &rate(t.read_rate + t.write_rate), theme::aqua(), hist.iter().map(|h| h.2).collect(), None);
            tile(&mut cols[3], icon::LIST, &format!("{} threads · {} handles", t.threads, t.handles), &format!("{} processes", t.processes), theme::yellow(), Vec::new(), None);
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.horizontal_wrapped(|ui| {
            ui.add(TextEdit::singleline(&mut self.search).hint_text(format!("{} Name, PID, path, window or service…", icon::MAGNIFYING_GLASS)).desired_width(280.0));
            ui.selectable_value(&mut self.view, View::Tree, format!("{} Tree", icon::TREE_STRUCTURE));
            ui.selectable_value(&mut self.view, View::List, format!("{} List", icon::LIST));
            ui.separator();
            let count = |f: Filter| snap.procs.iter().filter(|p| matches_filter(p, f)).count();
            for (f, label) in [
                (Filter::All, "All"),
                (Filter::Kind(Kind::App), "Apps"),
                (Filter::Kind(Kind::Background), "Background"),
                (Filter::Kind(Kind::Windows), "Windows"),
                (Filter::Suspended, "Suspended"),
                (Filter::Elevated, "Elevated"),
            ] {
                let sel = self.filter == f;
                let text = RichText::new(format!("{label} {}", count(f))).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.filter = f;
                }
            }
        });
    }

    /// Rows to show, in order, with tree depth.
    fn rows(&self, snap: &Snapshot) -> Vec<(usize, usize, bool)> {
        let q = self.search.trim().to_lowercase();
        let visible = |p: &Proc| matches_filter(p, self.filter) && (q.is_empty() || p.haystack().contains(&q));
        let cmp = |a: &Proc, b: &Proc| {
            let o = match self.sort.0 {
                SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortCol::Pid => a.pid.cmp(&b.pid),
                SortCol::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct).then(a.cpu_time.cmp(&b.cpu_time)),
                SortCol::Memory => a.private_bytes.cmp(&b.private_bytes),
                SortCol::Io => (a.read_rate + a.write_rate).total_cmp(&(b.read_rate + b.write_rate)),
                SortCol::Threads => a.threads.cmp(&b.threads),
                SortCol::Handles => a.handles.cmp(&b.handles),
            };
            if self.sort.1 { o.reverse() } else { o }
        };
        let procs = &snap.procs;
        if self.view == View::List || !q.is_empty() || self.filter != Filter::All {
            let mut idx: Vec<usize> = (0..procs.len()).filter(|&i| visible(&procs[i])).collect();
            idx.sort_by(|&a, &b| cmp(&procs[a], &procs[b]));
            return idx.into_iter().map(|i| (i, 0, false)).collect();
        }
        // Tree: a parent must exist and be older than the child (PIDs get reused).
        let by_pid: HashMap<u32, usize> = procs.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut roots = Vec::new();
        for (i, p) in procs.iter().enumerate() {
            match by_pid.get(&p.ppid) {
                Some(&pi) if pi != i && procs[pi].create_time <= p.create_time && p.pid != 0 => children.entry(pi).or_default().push(i),
                _ => roots.push(i),
            }
        }
        let sort = |v: &mut Vec<usize>| v.sort_by(|&a, &b| cmp(&procs[a], &procs[b]));
        sort(&mut roots);
        for v in children.values_mut() {
            sort(v);
        }
        let mut out = Vec::new();
        fn walk(i: usize, depth: usize, ch: &HashMap<usize, Vec<usize>>, collapsed: &HashSet<Key>, procs: &[Proc], out: &mut Vec<(usize, usize, bool)>) {
            let has = ch.get(&i).is_some_and(|c| !c.is_empty());
            out.push((i, depth, has));
            if has && !collapsed.contains(&procs[i].key()) && depth < 40 {
                for &c in &ch[&i] {
                    walk(c, depth + 1, ch, collapsed, procs, out);
                }
            }
        }
        for r in roots {
            walk(r, 0, &children, &self.collapsed, procs, &mut out);
        }
        out
    }

    fn table(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let rows = self.rows(snap);
        let procs = &snap.procs;
        let selected = self.selected;
        let tree = self.view == View::Tree && self.search.trim().is_empty() && self.filter == Filter::All;
        let collapsed = &self.collapsed;
        let eff_set = &self.efficiency_set;
        let sort = self.sort;
        let mut clicked: Option<Key> = None;
        let mut toggle: Option<Key> = None;
        let mut sort_click: Option<SortCol> = None;

        let header = |ui: &mut Ui, t: &str, c: SortCol, sort_click: &mut Option<SortCol>| {
            let arrow = if sort.0 == c { if sort.1 { " ▼" } else { " ▲" } } else { "" };
            if ui.add(Label::new(RichText::new(format!("{t}{arrow}")).small().strong().color(theme::text_dim())).sense(Sense::click()).selectable(false)).clicked() {
                *sort_click = Some(c);
            }
        };

        TableBuilder::new(ui)
            .id_salt("procs_table")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::initial(54.0).at_least(46.0))
            .column(Column::initial(80.0).at_least(64.0))
            .column(Column::initial(76.0).at_least(60.0))
            .column(Column::initial(74.0).at_least(56.0))
            .column(Column::initial(44.0).at_least(40.0))
            .column(Column::initial(60.0).at_least(50.0))
            .column(Column::initial(92.0).at_least(60.0).clip(true))
            .header(26.0, |mut h| {
                h.col(|ui| header(ui, "NAME", SortCol::Name, &mut sort_click));
                h.col(|ui| header(ui, "PID", SortCol::Pid, &mut sort_click));
                h.col(|ui| header(ui, "CPU", SortCol::Cpu, &mut sort_click));
                h.col(|ui| header(ui, "MEMORY", SortCol::Memory, &mut sort_click));
                h.col(|ui| header(ui, "I/O", SortCol::Io, &mut sort_click));
                h.col(|ui| header(ui, "THR", SortCol::Threads, &mut sort_click));
                h.col(|ui| header(ui, "HANDLES", SortCol::Handles, &mut sort_click));
                h.col(|ui| {
                    ui.label(RichText::new("STATUS").small().strong().color(theme::text_dim()));
                });
            })
            .body(|body| {
                body.rows(36.0, rows.len(), |mut row| {
                    let (i, depth, has_children) = rows[row.index()];
                    let p = &procs[i];
                    row.set_selected(selected == Some(p.key()));
                    row.col(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if tree {
                            ui.add_space(depth as f32 * 14.0);
                            if has_children {
                                let ic = if collapsed.contains(&p.key()) { icon::CARET_RIGHT } else { icon::CARET_DOWN };
                                if ui.add(Label::new(RichText::new(ic).color(theme::text_dim())).sense(Sense::click()).selectable(false)).clicked() {
                                    toggle = Some(p.key());
                                }
                            } else {
                                ui.add_space(14.0);
                            }
                        }
                        let kind_icon = match p.kind {
                            Kind::App => icon::APP_WINDOW,
                            Kind::Background => icon::GEAR_SIX,
                            Kind::Windows => icon::WINDOWS_LOGO,
                        };
                        ui.label(RichText::new(kind_icon).color(theme::text_dim()));
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let mut name = RichText::new(&p.name).strong();
                            if p.suspended {
                                name = name.color(theme::text_dim());
                            }
                            ui.add(Label::new(name).truncate().selectable(false));
                            let sub = p.window_title.clone().or_else(|| p.services.first().map(|s| if p.services.len() > 1 { format!("{} +{} services", s.1, p.services.len() - 1) } else { s.1.clone() }));
                            if let Some(s) = sub {
                                ui.add(Label::new(RichText::new(s).small().color(theme::text_dim())).truncate().selectable(false));
                            }
                        });
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(p.pid.to_string()).monospace().color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        usage_cell(ui, p.cpu_pct / 100.0, &format!("{:.1}%", p.cpu_pct), theme::accent());
                    });
                    row.col(|ui| {
                        let c = if p.private_bytes > 1 << 30 { theme::orange() } else { theme::text() };
                        ui.label(RichText::new(format_bytes(p.private_bytes)).color(c));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(rate(p.read_rate + p.write_rate)).color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(p.threads.to_string()).color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(p.handles.to_string()).color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if p.suspended {
                            theme::badge(ui, icon::PAUSE, theme::yellow()).on_hover_text("Suspended");
                        }
                        if p.efficiency || eff_set.contains(&p.key()) {
                            theme::badge(ui, icon::LEAF, theme::green()).on_hover_text("Efficiency mode");
                        }
                        if p.info.elevated {
                            theme::badge(ui, icon::SHIELD_CHECKERED, theme::orange()).on_hover_text("Running as administrator");
                        }
                        if p.info.critical {
                            theme::badge(ui, icon::WARNING_OCTAGON, theme::red()).on_hover_text("Critical: ending it crashes Windows");
                        }
                    });
                    if row.response().clicked() {
                        clicked = Some(p.key());
                    }
                });
            });
        if let Some(c) = sort_click {
            self.sort = if self.sort.0 == c { (c, !self.sort.1) } else { (c, !matches!(c, SortCol::Name | SortCol::Pid)) };
        }
        if let Some(k) = toggle {
            if !self.collapsed.remove(&k) {
                self.collapsed.insert(k);
            }
        }
        if let Some(k) = clicked {
            if self.selected != Some(k) {
                self.selected = Some(k);
                self.files = None;
                self.modules = None;
            }
        }
    }

    // ------------------------------------------------------------------ details

    fn details(&mut self, ui: &mut Ui, snap: &Snapshot, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        let Some(p) = self.selected_proc() else {
            ui.add_space(50.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::CURSOR_CLICK).size(40.0).color(theme::text_dim()));
                ui.label(RichText::new("Select a process to see its details\nand control it.").color(theme::text_dim()));
            });
            return;
        };
        let key = p.key();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.name).font(FontId::new(19.0, theme::heading_family())));
            ui.label(RichText::new(format!("PID {}", p.pid)).color(theme::text_dim()));
        });
        if let Some(t) = &p.window_title {
            ui.label(RichText::new(t).small().color(theme::text_dim()));
        }
        ui.add_space(6.0);
        self.actions(ui, &p, snap, elevated, toasts);
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (t, label) in [
                (DetailTab::Overview, "Overview"),
                (DetailTab::Performance, "Performance"),
                (DetailTab::Files, "Open files"),
                (DetailTab::Modules, "Modules"),
                (DetailTab::Network, "Network"),
            ] {
                let sel = self.tab == t;
                if ui.add(egui::Button::new(RichText::new(label).color(if sel { theme::text() } else { theme::text_dim() })).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.tab {
            DetailTab::Overview => self.overview(ui, &p, snap),
            DetailTab::Performance => self.performance(ui, &p),
            DetailTab::Files => {
                let stale = self.files.as_ref().is_none_or(|(k, _)| *k != key);
                if stale && self.files_rx.is_none() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (pid, c) = (p.pid, ctx.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send((key, inspect::open_files(pid)));
                        c.request_repaint();
                    });
                    self.files_rx = Some(rx);
                }
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut self.files_filter).hint_text("Filter…").desired_width(200.0));
                    if ui.add_enabled(self.files_rx.is_none(), egui::Button::new(format!("{} Refresh", icon::ARROWS_CLOCKWISE))).clicked() {
                        self.files = None;
                    }
                });
                match &self.files {
                    Some((k, Ok(files))) if *k == key => {
                        let q = self.files_filter.to_lowercase();
                        let list: Vec<&String> = files.iter().filter(|f| q.is_empty() || f.to_lowercase().contains(&q)).collect();
                        ui.label(RichText::new(format!("{} open file(s) and folder(s)", list.len())).small().color(theme::text_dim()));
                        for f in list {
                            ui.horizontal(|ui| {
                                if ui.small_button(icon::FOLDER_OPEN).on_hover_text("Show in Explorer").clicked() {
                                    shell::reveal_in_explorer(std::path::Path::new(f));
                                }
                                ui.add(Label::new(RichText::new(f).small().monospace()).truncate());
                            });
                        }
                    }
                    Some((k, Err(e))) if *k == key => {
                        ui.label(RichText::new(e).color(theme::orange()));
                    }
                    _ => {
                        ui.spinner();
                    }
                }
            }
            DetailTab::Modules => {
                if self.modules.as_ref().is_none_or(|(k, _)| *k != key) {
                    self.modules = Some((key, inspect::modules(p.pid)));
                }
                ui.add(TextEdit::singleline(&mut self.modules_filter).hint_text("Filter DLLs…").desired_width(200.0));
                match &self.modules {
                    Some((_, Ok(mods))) => {
                        let q = self.modules_filter.to_lowercase();
                        let windir = std::env::var("SystemRoot").unwrap_or_default().to_lowercase();
                        let list: Vec<&Module> = mods.iter().filter(|m| q.is_empty() || m.path.to_string_lossy().to_lowercase().contains(&q)).collect();
                        let third = list.iter().filter(|m| !m.path.to_string_lossy().to_lowercase().starts_with(&windir)).count();
                        ui.label(RichText::new(format!("{} module(s), {third} from outside Windows", list.len())).small().color(theme::text_dim()));
                        for m in list {
                            let outside = !m.path.to_string_lossy().to_lowercase().starts_with(&windir);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&m.name).strong().color(if outside { theme::yellow() } else { theme::text() }));
                                ui.label(RichText::new(format_bytes(u64::from(m.size))).small().color(theme::text_dim()));
                            });
                            ui.add(Label::new(RichText::new(m.path.display().to_string()).small().monospace().color(theme::text_dim())).truncate());
                        }
                    }
                    Some((_, Err(e))) => {
                        ui.label(RichText::new(e).color(theme::orange()));
                    }
                    None => {}
                }
            }
            DetailTab::Network => {
                let conns: Vec<conn::Conn> = conn::connections().into_iter().filter(|c| c.pid == p.pid).collect();
                if conns.is_empty() {
                    ui.label(RichText::new("No network sockets.").color(theme::text_dim()));
                }
                for c in conns {
                    ui.horizontal(|ui| {
                        theme::badge(ui, c.proto.label(), theme::aqua());
                        match c.remote {
                            Some(r) => ui.label(RichText::new(format!("{} → {}", c.local, r)).small().monospace()),
                            None => ui.label(RichText::new(format!("listening on {}", c.local)).small().monospace()),
                        };
                        ui.label(RichText::new(conn::state_label(c.proto, c.state)).small().color(theme::text_dim()));
                    });
                }
            }
        });
    }

    fn actions(&mut self, ui: &mut Ui, p: &Proc, snap: &Snapshot, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        let key = p.key();
        let protected = p.info.critical || p.pid <= 4;
        let windows_warning: Option<&'static str> = if p.pid == std::process::id() {
            Some("This is Wield itself.")
        } else if p.kind == Kind::Windows {
            Some("This is part of Windows. Ending it can log you out, freeze the desktop or stop system features until you restart.")
        } else {
            None
        };
        ui.horizontal_wrapped(|ui| {
            let end = theme::danger_button(ui, format!("{} End task", icon::X_CIRCLE), theme::red(), !protected);
            if protected {
                end.on_disabled_hover_text("Critical Windows process: ending it would crash Windows.");
            } else if end.clicked() {
                self.confirm = Some(Confirm { key, name: p.name.clone(), tree: false, warning: windows_warning });
            }
            let children = snap.procs.iter().any(|c| c.ppid == p.pid && c.create_time >= p.create_time && c.pid != p.pid);
            if children && ui.add_enabled(!protected, egui::Button::new(format!("{} End tree", icon::TREE_STRUCTURE))).clicked() {
                self.confirm = Some(Confirm { key, name: p.name.clone(), tree: true, warning: windows_warning });
            }
            let (label, ic) = if p.suspended { ("Resume", icon::PLAY) } else { ("Suspend", icon::PAUSE) };
            if ui.add_enabled(!protected && p.kind != Kind::Windows, egui::Button::new(format!("{ic} {label}")))
                .on_hover_text("Freeze the process temporarily. It uses no CPU until resumed.")
                .on_disabled_hover_text("Suspending Windows processes can freeze the desktop.")
                .clicked()
            {
                let r = if p.suspended { control::resume(p.pid) } else { control::suspend(p.pid) };
                report(&ctx, toasts, r, &format!("{label}d {}", p.name));
            }
            let eff = p.efficiency || self.efficiency_set.contains(&key);
            if ui.add_enabled(!protected, egui::Button::new(format!("{} {}", icon::LEAF, if eff { "Leave efficiency mode" } else { "Efficiency mode" })))
                .on_hover_text("Send to the background: lowest priority and power-throttled, like Task Manager's Efficiency mode.")
                .clicked()
            {
                match control::set_efficiency(p.pid, !eff) {
                    Ok(()) => {
                        if eff {
                            self.efficiency_set.remove(&key);
                        } else {
                            self.efficiency_set.insert(key);
                        }
                        toasts.success(&ctx, format!("{} {} efficiency mode.", p.name, if eff { "left" } else { "is in" }));
                    }
                    Err(e) => toasts.error(&ctx, e),
                }
            }
            ui.menu_button(format!("{} Priority: {}", icon::GAUGE, snapshot::priority_label(p.priority_class)), |ui| {
                for (class, label) in control::PRIORITIES {
                    if ui.selectable_label(p.priority_class == *class, *label).clicked() {
                        report(&ctx, toasts, control::set_priority(p.pid, *class), &format!("{} priority set to {label}", p.name));
                        ui.close();
                    }
                }
            });
            ui.menu_button(format!("{} More", icon::DOTS_THREE), |ui| {
                if ui.button(format!("{} CPU affinity…", icon::CPU)).clicked() {
                    if let Some((m, s)) = control::affinity(p.pid) {
                        self.affinity = Some((key, p.name.clone(), m, s));
                    } else {
                        toasts.error(&ctx, "Can't read this process's CPU affinity.");
                    }
                    ui.close();
                }
                if ui.add_enabled(p.info.exe.is_some() && !protected && p.kind != Kind::Windows, egui::Button::new(format!("{} Restart", icon::ARROWS_CLOCKWISE))).clicked() {
                    report(&ctx, toasts, control::restart(p), &format!("Restarted {}", p.name));
                    ui.close();
                }
                if let Some(exe) = p.info.exe.clone() {
                    if ui.button(format!("{} Open file location", icon::FOLDER_OPEN)).clicked() {
                        shell::reveal_in_explorer(&exe);
                        ui.close();
                    }
                    if ui.button(format!("{} Scan with Malware Scanner", icon::SHIELD_WARNING)).clicked() {
                        self.intent = Some(Intent::Scan(exe.clone()));
                        ui.close();
                    }
                    if ui.add_enabled(elevated, egui::Button::new(format!("{} Block its network access", icon::PROHIBIT))).on_disabled_hover_text("Needs administrator").clicked() {
                        let path = exe.display().to_string();
                        report(&ctx, toasts, with_com(move || firewall::block_program(&path)), &format!("Blocked network access for {}", p.name));
                        ui.close();
                    }
                }
                if ui.button(format!("{} Search online", icon::GLOBE)).clicked() {
                    shell::search_web(&format!("{} process what is it", p.name));
                    ui.close();
                }
                if let Some(c) = &p.info.cmdline {
                    if ui.button(format!("{} Copy command line", icon::COPY)).clicked() {
                        ctx.copy_text(c.clone());
                        ui.close();
                    }
                }
                if ui.button(format!("{} Copy PID", icon::COPY)).clicked() {
                    ctx.copy_text(p.pid.to_string());
                    ui.close();
                }
            });
        });
    }

    fn overview(&mut self, ui: &mut Ui, p: &Proc, snap: &Snapshot) {
        let ctx = ui.ctx().clone();
        // Signature, computed in the background once per path.
        if let Some(exe) = p.info.exe.clone() {
            if !self.signature.contains_key(&exe) && self.sig_rx.is_none() {
                self.signature.insert(exe.clone(), None);
                let (tx, rx) = std::sync::mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let s = sig::signature(&exe);
                    let _ = tx.send((exe, s));
                    c.request_repaint();
                });
                self.sig_rx = Some(rx);
            }
        }
        let parent = snap.procs.iter().find(|q| q.pid == p.ppid && q.create_time <= p.create_time).cloned();
        let children: Vec<Proc> = snap.procs.iter().filter(|c| c.ppid == p.pid && c.create_time >= p.create_time && c.pid != p.pid).cloned().collect();
        let mem = inspect::memory(p.pid);
        let io = inspect::io(p.pid);
        let mut jump: Option<Key> = None;
        egui::Grid::new("proc_overview").num_columns(2).spacing(vec2(10.0, 5.0)).show(ui, |ui| {
            let row = |ui: &mut Ui, k: &str, v: String, mono: bool, color: Option<Color32>| {
                ui.label(RichText::new(k).small().color(theme::text_dim()));
                let mut t = if mono { RichText::new(v).monospace().small() } else { RichText::new(v) };
                if let Some(c) = color {
                    t = t.color(c);
                }
                ui.add(Label::new(t).wrap());
                ui.end_row();
            };
            if let Some(e) = &p.info.exe {
                row(ui, "Path", e.display().to_string(), true, None);
                let s = self.signature.get(e).cloned().flatten();
                let (t, c) = match s {
                    Some(Signature::Signed { signer }) => (format!("Signed by {signer}"), Some(theme::green())),
                    Some(Signature::Catalog { signer }) => (format!("Windows catalog ({signer})"), Some(theme::green())),
                    Some(Signature::Bad { reason }) => (reason, Some(theme::orange())),
                    Some(Signature::Unsigned) => ("Not signed".into(), Some(theme::yellow())),
                    None => ("Checking…".into(), None),
                };
                row(ui, "Signature", t, false, c);
            } else {
                row(ui, "Path", "Not available (protected or needs administrator)".into(), false, Some(theme::text_dim()));
            }
            if let Some(c) = &p.info.cmdline {
                row(ui, "Command line", c.clone(), true, None);
            }
            ui.label(RichText::new("Parent").small().color(theme::text_dim()));
            match &parent {
                Some(par) => {
                    if ui.link(format!("{} (PID {})", par.name, par.pid)).clicked() {
                        jump = Some(par.key());
                    }
                }
                None => {
                    ui.label(RichText::new(format!("PID {} (exited)", p.ppid)).color(theme::text_dim()));
                }
            }
            ui.end_row();
            let started = snapshot::filetime_to_unix(p.create_time);
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if p.create_time > 0 {
                row(ui, "Started", format!("{} (running {})", journal::format_time(started), duration(now.saturating_sub(started))), false, None);
            }
            row(ui, "CPU time", duration(p.cpu_time / 10_000_000), false, None);
            row(ui, "User", p.info.user.clone().unwrap_or_else(|| "—".into()), false, None);
            row(ui, "Integrity", inspect::integrity(p.pid).unwrap_or("—").into(), false, None);
            row(ui, "Elevated", if p.info.elevated { "Yes" } else { "No" }.into(), false, p.info.elevated.then(theme::orange));
            row(ui, "Architecture", inspect::architecture(p.pid).unwrap_or("—").into(), false, None);
            row(ui, "Session", p.session.to_string(), false, None);
            row(ui, "Priority", snapshot::priority_label(p.priority_class).into(), false, None);
            row(ui, "Threads / handles", format!("{} / {}", p.threads, p.handles), false, None);
            if let Some(m) = &mem {
                row(ui, "Private memory", format!("{} (peak {})", format_bytes(m.private), format_bytes(m.peak_commit)), false, None);
                row(ui, "Working set", format!("{} (peak {}, private {})", format_bytes(m.working_set), format_bytes(m.peak_working_set), format_bytes(p.private_ws)), false, None);
                row(ui, "Page faults", m.page_faults.to_string(), false, None);
                row(ui, "Kernel pools", format!("paged {} · non-paged {}", format_bytes(m.paged_pool), format_bytes(m.nonpaged_pool)), false, None);
            } else {
                row(ui, "Private memory", format_bytes(p.private_bytes), false, None);
                row(ui, "Working set", format_bytes(p.working_set), false, None);
            }
            match io {
                Some(c) => row(
                    ui,
                    "I/O total",
                    format!(
                        "read {} ({} ops) · write {} ({} ops) · other {}",
                        format_bytes(c.ReadTransferCount),
                        c.ReadOperationCount,
                        format_bytes(c.WriteTransferCount),
                        c.WriteOperationCount,
                        format_bytes(c.OtherTransferCount)
                    ),
                    false,
                    None,
                ),
                None => row(ui, "I/O total", format!("read {} · write {}", format_bytes(p.io_read), format_bytes(p.io_write)), false, None),
            }
        });
        if !p.services.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Hosts {} service(s)", p.services.len())).strong());
            for (name, display) in &p.services {
                ui.label(RichText::new(format!("{display}  ({name})")).small());
            }
        }
        if !children.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("Child processes ({})", children.len())).strong());
            for c in &children {
                if ui.link(format!("{} (PID {}) · {:.1}% · {}", c.name, c.pid, c.cpu_pct, format_bytes(c.private_bytes))).clicked() {
                    jump = Some(c.key());
                }
            }
        }
        if let Some(k) = jump {
            self.selected = Some(k);
        }
    }

    fn performance(&self, ui: &mut Ui, p: &Proc) {
        let h: Vec<(f32, u64)> = self.proc_history.get(&p.key()).map(|v| v.iter().copied().collect()).unwrap_or_default();
        ui.label(RichText::new(format!("CPU {:.1}%", p.cpu_pct)).strong().color(theme::accent()));
        sparkline(ui, &h.iter().map(|x| x.0 as f64).collect::<Vec<_>>(), Some(100.0), theme::accent(), 90.0);
        ui.add_space(8.0);
        ui.label(RichText::new(format!("Private memory {}", format_bytes(p.private_bytes))).strong().color(theme::purple()));
        sparkline(ui, &h.iter().map(|x| x.1 as f64).collect::<Vec<_>>(), None, theme::purple(), 90.0);
        ui.add_space(8.0);
        ui.label(RichText::new(format!("I/O now: read {} · write {}", rate(p.read_rate), rate(p.write_rate))).color(theme::aqua()));
        ui.label(RichText::new("Graphs cover the last two minutes while this screen is open.").small().color(theme::text_dim()));
    }

    // ------------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context, snap: &Snapshot, toasts: &mut Toasts) {
        if let Some(c) = &self.confirm {
            let (key, name, tree, warning) = (c.key, c.name.clone(), c.tree, c.warning);
            let mut go = false;
            let mut close = false;
            let color = if warning.is_some() { theme::red() } else { theme::orange() };
            let m = egui::Modal::new(Id::new("proc_end"))
                .frame(egui::Frame::new().fill(theme::card()).stroke(Stroke::new(2.0, color)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20)))
                .show(ctx, |ui| {
                    ui.set_width(440.0);
                    ui.label(RichText::new(format!("End {}{}?", name, if tree { " and its children" } else { "" })).font(FontId::new(18.0, theme::heading_family())));
                    ui.add_space(6.0);
                    ui.label("Unsaved work in it will be lost.");
                    if let Some(w) = warning {
                        ui.label(RichText::new(w).color(theme::red()));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if theme::danger_button(ui, if tree { "End process tree" } else { "End task" }, color, true).clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if go {
                self.confirm = None;
                if tree {
                    let (n, errs) = control::end_tree(key.0, &snap.procs);
                    if errs.is_empty() {
                        toasts.success(ctx, format!("Ended {n} process(es)."));
                    } else {
                        toasts.error(ctx, format!("Ended {n}; {} failed: {}", errs.len(), errs[0]));
                    }
                } else {
                    report(ctx, toasts, control::end(key.0), &format!("Ended {name}"));
                }
            } else if close || m.should_close() {
                self.confirm = None;
            }
        }

        if let Some((key, name, mask, sys)) = self.affinity.clone() {
            let mut new_mask = mask;
            let mut close = false;
            let m = egui::Modal::new(Id::new("proc_affinity")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(format!("CPU affinity: {name}")).font(FontId::new(18.0, theme::heading_family())));
                ui.label(RichText::new("Choose which logical processors this process may run on.").small().color(theme::text_dim()));
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    for b in 0..usize::BITS as usize {
                        if sys & (1 << b) == 0 {
                            continue;
                        }
                        let mut on = new_mask & (1 << b) != 0;
                        if ui.checkbox(&mut on, format!("CPU {b}")).changed() {
                            new_mask ^= 1 << b;
                        }
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Apply").clicked() {
                        report(ctx, toasts, control::set_affinity(key.0, new_mask), &format!("Affinity updated for {name}"));
                        close = true;
                    }
                    if ui.button("All").clicked() {
                        new_mask = sys;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close || m.should_close() {
                self.affinity = None;
            } else if let Some(a) = &mut self.affinity {
                a.2 = new_mask;
            }
        }

        if self.locks.is_some() {
            let mut close = false;
            let mut end_pid: Option<u32> = None;
            let mut select_pid: Option<u32> = None;
            let l = self.locks.as_mut().unwrap();
            let m = egui::Modal::new(Id::new("proc_locks")).show(ctx, |ui| {
                ui.set_width(560.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::LOCK_SIMPLE).size(24.0).color(theme::accent()));
                    ui.label(RichText::new("Who is locking a file?").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.label(RichText::new("Find the programs that keep a file or folder busy (\"file is open in another program\").").small().color(theme::text_dim()));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(l.pick_rx.is_none(), egui::Button::new(format!("{} Choose file…", icon::FILE))).clicked() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(shell::pick_files("Choose a locked file"));
                            c.request_repaint();
                        });
                        l.pick_rx = Some(rx);
                    }
                    if ui.add_enabled(l.pick_rx.is_none(), egui::Button::new(format!("{} Choose folder…", icon::FOLDER_OPEN))).clicked() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(with_com(|| shell::pick_folder("Choose a folder")).into_iter().collect());
                            c.request_repaint();
                        });
                        l.pick_rx = Some(rx);
                    }
                    if !l.paths.is_empty() && ui.button(format!("{} Check again", icon::ARROWS_CLOCKWISE)).clicked() {
                        l.result = Some(inspect::who_locks(&l.paths));
                    }
                });
                for p in &l.paths {
                    ui.label(RichText::new(p.display().to_string()).small().monospace());
                }
                match &l.result {
                    Some(Ok(v)) if v.is_empty() => {
                        ui.label(RichText::new(format!("{} Nothing is using it.", icon::CHECK_CIRCLE)).color(theme::green()));
                    }
                    Some(Ok(v)) => {
                        for lk in v {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&lk.app).strong());
                                ui.label(RichText::new(format!("PID {}", lk.pid)).color(theme::text_dim()));
                                if !lk.service.is_empty() {
                                    theme::badge(ui, format!("service {}", lk.service), theme::aqua());
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if theme::danger_button(ui, "End", theme::red(), true).clicked() {
                                        end_pid = Some(lk.pid);
                                    }
                                    if ui.button("Show").clicked() {
                                        select_pid = Some(lk.pid);
                                    }
                                });
                            });
                        }
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(theme::orange()));
                    }
                    None => {}
                }
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            if let Some(pid) = end_pid {
                report(ctx, toasts, control::end(pid), &format!("Ended PID {pid}"));
                if let Some(l) = &mut self.locks {
                    l.result = Some(inspect::who_locks(&l.paths));
                }
            }
            if let Some(pid) = select_pid {
                self.selected = snap.procs.iter().find(|p| p.pid == pid).map(|p| p.key());
                close = true;
            }
            if close || m.should_close() {
                self.locks = None;
            }
        }
    }
}

fn matches_filter(p: &Proc, f: Filter) -> bool {
    match f {
        Filter::All => true,
        Filter::Kind(k) => p.kind == k,
        Filter::Suspended => p.suspended,
        Filter::Elevated => p.info.elevated,
    }
}

fn report(ctx: &egui::Context, toasts: &mut Toasts, r: Result<(), String>, ok: &str) {
    match r {
        Ok(()) => toasts.success(ctx, format!("{ok}.")),
        Err(e) => toasts.error(ctx, e),
    }
}

fn usage_cell(ui: &mut Ui, frac: f32, text: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(80.0), 18.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), theme::border().gamma_multiply(0.5));
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    p.rect_filled(fill, CornerRadius::same(3), color.gamma_multiply(0.6));
    p.text(rect.left_center() + vec2(5.0, 0.0), egui::Align2::LEFT_CENTER, text, FontId::proportional(12.5), theme::text());
}

fn sparkline(ui: &mut Ui, values: &[f64], max: Option<f64>, color: Color32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(6), theme::bg());
    if values.len() < 2 {
        return;
    }
    let top = max.unwrap_or_else(|| values.iter().copied().fold(1.0, f64::max) * 1.15);
    let n = values.len();
    let pts: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(rect.right() - (n - 1 - i) as f32 * rect.width() / 79.0, rect.bottom() - 3.0 - (v / top) as f32 * (rect.height() - 6.0)))
        .collect();
    let mut fill = pts.clone();
    fill.push(pos2(pts.last().unwrap().x, rect.bottom()));
    fill.push(pos2(pts[0].x, rect.bottom()));
    p.add(egui::Shape::convex_polygon(fill, color.gamma_multiply(0.12), Stroke::NONE));
    p.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
}

fn tile(ui: &mut Ui, ic: &str, label: &str, value: &str, color: Color32, hist: Vec<f64>, max: Option<f64>) {
    theme::card_frame().inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(ic).size(20.0).color(color));
            ui.label(RichText::new(value).font(FontId::new(20.0, theme::heading_family())).color(color));
        });
        ui.label(RichText::new(label).small().color(theme::text_dim()));
        if !hist.is_empty() {
            sparkline(ui, &hist, max, color, 30.0);
        }
    });
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

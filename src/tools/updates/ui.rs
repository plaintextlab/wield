//! Update Control screen.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Label, Layout, Margin, RichText, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::appupdates::{self, InstalledApp, Upgrade, norm};
use super::winupdate::{self, History, WinUpdate};
use crate::theme;
use crate::toast::Toasts;
use crate::tools::apps::winget::{Outcome, Progress};
use crate::tools::drivers::wu;
use crate::win::fileinfo::format_bytes;
use crate::win::{elevation, shell};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Windows,
    Apps,
}

struct WuStatus {
    paused_until: Option<u64>,
    service: (String, String),
    history: Result<History, String>,
}

enum Msg {
    Status(WuStatus),
    WuScan(Result<Vec<WinUpdate>, String>),
    AppScan(Result<Vec<Upgrade>, String>, Option<BTreeSet<String>>),
    Op(String, Result<String, String>),
    Line(String),
}

/// A running batch of app updates or downloads.
struct Batch {
    list: Vec<Upgrade>,
    download_only: bool,
    rx: Receiver<Progress>,
    stop: Arc<AtomicBool>,
    current: Option<usize>,
    results: Vec<Option<Outcome>>,
    line: String,
    pct: Option<f32>,
    log: Vec<String>,
    started: Instant,
    done: bool,
    log_file: Option<std::path::PathBuf>,
}

pub struct UpdateManager {
    tab: Tab,
    rx: Option<Receiver<Msg>>,
    busy_label: String,
    status: Option<WuStatus>,
    wu: Option<Result<Vec<WinUpdate>, String>>,
    wu_selected: BTreeSet<String>,
    wu_log: Vec<String>,
    pause_weeks: u32,
    // Apps
    installed: Vec<InstalledApp>,
    excluded: BTreeSet<String>,
    search: String,
    scanned: Option<Result<Vec<Upgrade>, String>>,
    known: Option<BTreeSet<String>>,
    selected: BTreeSet<String>,
    batch: Option<Batch>,
    winget_rx: Option<Receiver<Progress>>,
    scan_on_load: bool,
}

impl UpdateManager {
    pub fn new() -> Self {
        Self {
            tab: Tab::Windows,
            rx: None,
            busy_label: String::new(),
            status: None,
            wu: None,
            wu_selected: BTreeSet::new(),
            wu_log: Vec::new(),
            pause_weeks: 1,
            installed: Vec::new(),
            excluded: appupdates::excluded(),
            search: String::new(),
            scanned: None,
            known: None,
            selected: BTreeSet::new(),
            batch: None,
            winget_rx: None,
            scan_on_load: false,
        }
    }

    pub fn set_tab(&mut self, name: &str) {
        self.tab = match name.to_lowercase().as_str() {
            "apps" => Tab::Apps,
            // Opening this from the command line starts the scan right away.
            "appscan" => {
                self.scan_on_load = true;
                Tab::Apps
            }
            _ => Tab::Windows,
        };
    }

    pub fn summary_line(&self) -> Option<String> {
        if let Some(Ok(v)) = &self.scanned {
            return Some(format!("{} app update(s) available", v.iter().filter(|u| !self.is_excluded(&u.name)).count()));
        }
        let st = self.status.as_ref()?;
        Some(match st.paused_until {
            Some(t) => format!("Windows updates paused until {}", &winupdate::iso(t)[..10]),
            None => "Windows updates on".into(),
        })
    }

    fn is_excluded(&self, name: &str) -> bool {
        self.excluded.contains(&norm(name))
    }

    fn busy(&self) -> bool {
        self.rx.is_some() || self.batch.as_ref().is_some_and(|b| !b.done) || self.winget_rx.is_some()
    }

    fn spawn(&mut self, ctx: &egui::Context, label: &str, f: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            c.request_repaint();
        });
        self.busy_label = label.to_string();
        self.rx = Some(rx);
    }

    fn load_status(&mut self, ctx: &egui::Context) {
        self.spawn(ctx, "Reading Windows Update status…", || Msg::Status(WuStatus { paused_until: winupdate::paused_until(), service: winupdate::service(), history: winupdate::history() }));
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if let Some(rx) = &self.rx {
            let msgs: Vec<Msg> = rx.try_iter().collect();
            for m in msgs {
                match m {
                    Msg::Line(l) => {
                        self.busy_label = l.clone();
                        self.wu_log.push(l);
                        continue;
                    }
                    Msg::Status(s) => self.status = Some(s),
                    Msg::WuScan(r) => {
                        if let Ok(v) = &r {
                            self.wu_selected = v.iter().filter(|u| !u.optional).map(|u| u.id.clone()).collect();
                            if v.is_empty() {
                                toasts.success(ctx, "Windows is up to date.");
                            }
                        }
                        self.wu = Some(r);
                    }
                    Msg::AppScan(r, known) => {
                        if let Ok(v) = &r {
                            self.selected = v.iter().filter(|u| !self.is_excluded(&u.name)).map(|u| u.id.clone()).collect();
                            let n = self.selected.len();
                            if n == 0 {
                                toasts.success(ctx, "All your apps are up to date.");
                            } else {
                                toasts.info(ctx, format!("{n} app update(s) available."));
                            }
                        }
                        self.scanned = Some(r);
                        self.known = known;
                    }
                    Msg::Op(t, r) => {
                        match r {
                            Ok(m) => toasts.success(ctx, m),
                            Err(e) => toasts.error(ctx, format!("{t}: {e}")),
                        }
                        self.rx = None;
                        self.load_status(ctx);
                        return;
                    }
                }
                self.rx = None;
            }
        }
        if let Some(rx) = &self.winget_rx {
            let msgs: Vec<Progress> = rx.try_iter().collect();
            for p in msgs {
                match p {
                    Progress::Line(l) => self.busy_label = l,
                    Progress::Finished(_, Outcome::Failed(e)) => toasts.error(ctx, format!("Installing winget: {e}")),
                    Progress::Finished(..) => toasts.success(ctx, "winget is installed. You can scan for app updates now."),
                    Progress::Done(_) => self.winget_rx = None,
                    _ => {}
                }
            }
        }
        let mut finished = false;
        if let Some(b) = &mut self.batch {
            for p in b.rx.try_iter() {
                match p {
                    Progress::Start(i) => {
                        b.current = Some(i);
                        b.pct = None;
                    }
                    Progress::Line(l) => {
                        b.line = l.clone();
                        b.log.push(l);
                    }
                    Progress::Percent(p) => b.pct = Some(p),
                    Progress::Finished(i, o) => b.results[i] = Some(o),
                    Progress::Done(f) => {
                        b.done = true;
                        b.current = None;
                        b.log_file = f;
                        finished = true;
                    }
                }
            }
        }
        if finished {
            let b = self.batch.as_ref().unwrap();
            let ok = b.results.iter().filter(|r| matches!(r, Some(o) if o.ok())).count();
            let failed = b.results.iter().filter(|r| matches!(r, Some(Outcome::Failed(_)))).count();
            if b.download_only {
                toasts.success(ctx, format!("{ok} installer(s) saved to {}.", appupdates::download_dir().display()));
            } else if failed == 0 {
                toasts.success(ctx, format!("{ok} app(s) updated."));
            } else {
                toasts.error(ctx, format!("{ok} updated, {failed} failed. See the list for why."));
            }
            if !b.download_only {
                // Rescan so updated apps drop off the list.
                let done: Vec<String> = b.list.iter().zip(&b.results).filter(|(_, r)| matches!(r, Some(o) if o.ok())).map(|(u, _)| u.id.clone()).collect();
                if let Some(Ok(v)) = &mut self.scanned {
                    v.retain(|u| !done.contains(&u.id));
                }
                for id in done {
                    self.selected.remove(&id);
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if self.status.is_none() && self.rx.is_none() {
            self.load_status(&ctx);
        }
        if self.installed.is_empty() {
            self.installed = appupdates::installed();
        }
        self.poll(&ctx, toasts);
        if self.scan_on_load && !self.busy() {
            self.scan_on_load = false;
            self.start_scan(&ctx);
        }
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        egui::Panel::top("upd_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 20 }).show(ui, |ui| match self.tab {
                    Tab::Windows => self.windows_tab(ui, elevated),
                    Tab::Apps => self.apps_tab(ui),
                });
            });
        });
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CLOUD_ARROW_DOWN).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Update Control");
                ui.label(RichText::new("Pause or run Windows Update, and keep your apps up to date from their official sources.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.rx.is_some() || self.winget_rx.is_some() {
                    ui.spinner();
                    ui.add(Label::new(RichText::new(&self.busy_label).color(theme::text_dim())).truncate());
                }
            });
        });
        ui.add_space(6.0);
        if !elevated && self.tab == Tab::Windows {
            let mut clicked = false;
            banner(ui, theme::orange(), icon::LOCK, "Checking works now. Pausing, resuming and installing Windows updates need administrator rights.", Some(("Restart as administrator", &mut clicked)));
            if clicked {
                match elevation::relaunch_as_admin() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => toasts.error(&ctx, e),
                }
            }
        }
        ui.horizontal(|ui| {
            let n = match &self.scanned {
                Some(Ok(v)) => v.iter().filter(|u| !self.is_excluded(&u.name)).count(),
                _ => 0,
            };
            for (t, ic, label) in [(Tab::Windows, icon::WINDOWS_LOGO, "Windows Update".to_string()), (Tab::Apps, icon::APP_WINDOW, if n > 0 { format!("App updates ({n})") } else { "App updates".into() })] {
                let sel = self.tab == t;
                let text = RichText::new(format!("{ic} {label}")).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    // ------------------------------------------------------------------ Windows Update

    fn windows_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        section(ui, icon::PAUSE_CIRCLE, "Pause or resume", "Pausing stops Windows from downloading and installing updates (including feature updates) until the date you choose. Windows allows up to 5 weeks; after that it installs pending updates before you can pause again.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match self.status.as_ref().map(|s| (s.paused_until, s.service.clone())) {
                None => {
                    ui.spinner();
                }
                Some((paused_until, service)) => {
                    ui.horizontal(|ui| {
                        match paused_until {
                            Some(t) => {
                                ui.label(RichText::new(icon::PAUSE_CIRCLE).size(22.0).color(theme::yellow()));
                                ui.label(RichText::new(format!("Updates are paused until {}", &winupdate::iso(t)[..10])).strong().color(theme::yellow()));
                            }
                            None => {
                                ui.label(RichText::new(icon::CHECK_CIRCLE).size(22.0).color(theme::green()));
                                ui.label(RichText::new("Updates are on").strong().color(theme::green()));
                            }
                        }
                        if service.1.eq_ignore_ascii_case("Disabled") {
                            theme::badge(ui, "Windows Update service is disabled", theme::red()).on_hover_text("Another tool turned the Windows Update service off; updates can't run until it's set back to Manual.");
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("pause_weeks").selected_text(format!("{} week{}", self.pause_weeks, if self.pause_weeks == 1 { "" } else { "s" })).width(110.0).show_ui(ui, |ui| {
                            for w in 1..=5 {
                                ui.selectable_value(&mut self.pause_weeks, w, format!("{w} week{}", if w == 1 { "" } else { "s" }));
                            }
                        });
                        let w = self.pause_weeks;
                        if ui.add_enabled(elevated && !busy, egui::Button::new(format!("{} Pause updates", icon::PAUSE))).on_disabled_hover_text("Needs administrator rights").clicked() {
                            self.spawn(&ctx, "Pausing…", move || Msg::Op("Pause".into(), winupdate::pause(w * 7)));
                        }
                        if paused_until.is_some() && ui.add_enabled(elevated && !busy, egui::Button::new(format!("{} Resume updates", icon::PLAY))).clicked() {
                            self.spawn(&ctx, "Resuming…", || Msg::Op("Resume".into(), winupdate::resume()));
                        }
                        if ui.button(format!("{} Open Windows Update", icon::ARROW_SQUARE_OUT)).clicked() {
                            shell::open("ms-settings:windowsupdate");
                        }
                    });
                }
            }
        });
        ui.add_space(14.0);

        section(ui, icon::MAGNIFYING_GLASS, "Available updates", "Security fixes, cumulative updates, .NET and other Microsoft updates for this PC. Optional updates are listed unticked.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("{} Check for updates", icon::MAGNIFYING_GLASS)).color(theme::on_accent())).fill(theme::accent())).clicked() {
                    self.spawn(&ctx, "Checking Windows Update (up to a minute)…", || Msg::WuScan(winupdate::search()));
                }
                if let Some(Ok(h)) = self.status.as_ref().map(|s| &s.history) {
                    ui.label(RichText::new(format!("Last checked {} · last installed {}", or_never(&h.last_search), or_never(&h.last_install))).small().color(theme::text_dim()));
                }
            });
            match &self.wu {
                None => {}
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                Some(Ok(v)) if v.is_empty() => {
                    ui.label(RichText::new(format!("{} Windows is up to date.", icon::CHECK_CIRCLE)).color(theme::green()));
                }
                Some(Ok(v)) => {
                    let v = v.clone();
                    ui.add_space(6.0);
                    for u in &v {
                        ui.horizontal(|ui| {
                            let mut on = self.wu_selected.contains(&u.id);
                            if ui.checkbox(&mut on, RichText::new(&u.title).strong()).changed() {
                                if on {
                                    self.wu_selected.insert(u.id.clone());
                                } else {
                                    self.wu_selected.remove(&u.id);
                                }
                            }
                            if !u.severity.is_empty() {
                                theme::badge(ui, &u.severity, if u.severity == "Critical" { theme::red() } else { theme::yellow() });
                            }
                            if u.optional {
                                theme::badge(ui, "Optional", theme::text_dim());
                            }
                            ui.label(RichText::new(format!("{} · {}", u.category, format_bytes(u.size))).small().color(theme::text_dim()));
                        });
                    }
                    ui.add_space(6.0);
                    let n = self.wu_selected.len();
                    if ui.add_enabled(n > 0 && elevated && !busy, egui::Button::new(RichText::new(format!("{} Install {n} selected", icon::DOWNLOAD_SIMPLE)).color(theme::on_accent())).fill(theme::accent())).on_disabled_hover_text(if elevated { "Select updates first" } else { "Needs administrator rights" }).clicked() {
                        let ids: Vec<String> = self.wu_selected.iter().cloned().collect();
                        let (tx, rx) = std::sync::mpsc::channel();
                        let (otx, orx) = std::sync::mpsc::channel();
                        let c = ctx.clone();
                        std::thread::spawn(move || wu::install_kind(ids, "Software", tx, c));
                        let c2 = ctx.clone();
                        std::thread::spawn(move || {
                            for p in rx {
                                let m = match p {
                                    wu::Progress::Line(l) => Msg::Line(l),
                                    wu::Progress::Done(r) => Msg::Op("Windows Update".into(), r),
                                };
                                let _ = otx.send(m);
                                c2.request_repaint();
                            }
                        });
                        self.wu_log.clear();
                        self.busy_label = "Installing Windows updates…".into();
                        self.rx = Some(orx);
                        self.wu = None;
                    }
                }
            }
            if !self.wu_log.is_empty() {
                egui::CollapsingHeader::new("Details").id_salt("wu_install_log").show(ui, |ui| {
                    for l in &self.wu_log {
                        ui.label(RichText::new(l).small().monospace().color(theme::text_dim()));
                    }
                });
            }
        });
        ui.add_space(14.0);

        section(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Update history", "");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match self.status.as_ref().map(|s| &s.history) {
                None => {
                    ui.spinner();
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                Some(Ok(h)) if h.items.is_empty() => {
                    ui.label(RichText::new("No update history.").color(theme::text_dim()));
                }
                Some(Ok(h)) => {
                    for it in h.items.iter().take(25) {
                        ui.horizontal(|ui| {
                            let (ic, c) = match it.result {
                                2 | 3 => (icon::CHECK_CIRCLE, theme::green()),
                                4 => (icon::X_CIRCLE, theme::red()),
                                _ => (icon::MINUS_CIRCLE, theme::text_dim()),
                            };
                            ui.label(RichText::new(ic).color(c));
                            ui.label(RichText::new(&it.date).small().monospace().color(theme::text_dim()));
                            ui.add(Label::new(&it.title).truncate());
                        });
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------------ App updates

    fn apps_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        let has_winget = crate::tools::apps::winget::winget_path().is_some();
        if !has_winget {
            let mut clicked = false;
            banner(ui, theme::orange(), icon::WARNING, "App updates use the Windows Package Manager (winget), which isn't on this PC (normal on LTSC). Wield can install it from Microsoft's official release (about 300 MB, verified).", Some(("Install winget", &mut clicked)));
            if clicked && !busy {
                let (tx, rx) = std::sync::mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || crate::tools::apps::winget::install_winget(tx, c));
                self.winget_rx = Some(rx);
                self.busy_label = "Installing winget…".into();
            }
        }
        self.batch_card(ui);

        // Results of the last scan.
        if let Some(r) = self.scanned.clone() {
            section(ui, icon::DOWNLOAD_SIMPLE, "Updates found", "Newer versions from the winget catalog, which downloads each app from its publisher. Untick any you want to skip this time.");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                match r {
                    Err(e) => {
                        ui.label(RichText::new(e).color(theme::red()));
                    }
                    Ok(v) => {
                        let v: Vec<Upgrade> = v.into_iter().filter(|u| !self.is_excluded(&u.name)).collect();
                        if v.is_empty() {
                            ui.label(RichText::new(format!("{} All the apps Wield could check are up to date.", icon::CHECK_CIRCLE)).color(theme::green()));
                        }
                        for u in &v {
                            ui.horizontal(|ui| {
                                let mut on = self.selected.contains(&u.id);
                                if ui.add_enabled(!busy, egui::Checkbox::new(&mut on, RichText::new(&u.name).strong())).changed() {
                                    if on {
                                        self.selected.insert(u.id.clone());
                                    } else {
                                        self.selected.remove(&u.id);
                                    }
                                }
                                ui.label(RichText::new(format!("{}  →  ", if u.version == "Unknown" { "unknown version" } else { &u.version })).color(theme::text_dim()));
                                ui.label(RichText::new(&u.available).color(theme::green()).strong());
                                ui.label(RichText::new(&u.id).small().monospace().color(theme::text_dim()));
                            });
                        }
                        if !v.is_empty() {
                            ui.add_space(8.0);
                            let n = self.selected.len();
                            ui.horizontal(|ui| {
                                let chosen: Vec<Upgrade> = v.iter().filter(|u| self.selected.contains(&u.id)).cloned().collect();
                                if ui.add_enabled(n > 0 && !busy, egui::Button::new(RichText::new(format!("{} Update {n} app{}", icon::DOWNLOAD_SIMPLE, if n == 1 { "" } else { "s" })).color(theme::on_accent())).fill(theme::accent()).min_size(vec2(0.0, 32.0))).on_hover_text("Downloads and installs each update silently, one after another. Close the apps first for the smoothest update.").clicked() {
                                    self.start_batch(&ctx, chosen.clone(), false);
                                }
                                if ui.add_enabled(n > 0 && !busy, egui::Button::new(format!("{} Download only", icon::FOLDER_OPEN))).on_hover_text(format!("Saves the installers to {} without installing them.", appupdates::download_dir().display())).clicked() {
                                    self.start_batch(&ctx, chosen, true);
                                }
                                ui.label(RichText::new("Some updates ask for permission (UAC) unless Wield runs as administrator.").small().color(theme::text_dim()));
                            });
                        }
                        if let Some(known) = &self.known {
                            let unknown: Vec<&InstalledApp> = self.installed.iter().filter(|a| !self.is_excluded(&a.name) && !known.contains(&norm(&a.name))).collect();
                            if !unknown.is_empty() {
                                ui.add_space(6.0);
                                egui::CollapsingHeader::new(RichText::new(format!("{} {} app(s) can't be checked automatically", icon::INFO, unknown.len())).color(theme::text_dim())).id_salt("unknown_apps").show(ui, |ui| {
                                    ui.label(RichText::new("They aren't in the winget catalog (drivers, games from launchers, company software…). Most update themselves or through their launcher.").small().color(theme::text_dim()));
                                    for a in unknown {
                                        ui.label(RichText::new(format!("• {}  {}", a.name, a.version)).small());
                                    }
                                });
                            }
                        }
                    }
                }
                ui.add_space(4.0);
                if ui.button(format!("{} Microsoft Store app updates", icon::ARROW_SQUARE_OUT)).on_hover_text("Store apps update through the Microsoft Store").clicked() {
                    shell::open("ms-windows-store://downloadsandupdates");
                }
            });
            ui.add_space(14.0);
        }

        // The inventory with per-app switches.
        let total = self.installed.len();
        let off = self.installed.iter().filter(|a| self.is_excluded(&a.name)).count();
        section(ui, icon::LIST_CHECKS, "Installed apps", "Every app on this PC. Switch off the ones Wield shouldn't check (for example apps you keep on a specific version). Your choice is remembered.");
        ui.horizontal(|ui| {
            let label = if self.scanned.is_some() { "Scan again" } else { "Scan for updates" };
            if ui.add_enabled(has_winget && !busy, egui::Button::new(RichText::new(format!("{} {label}", icon::MAGNIFYING_GLASS)).color(theme::on_accent())).fill(theme::accent()).min_size(vec2(0.0, 32.0))).on_disabled_hover_text("Needs winget").clicked() {
                self.start_scan(&ctx);
            }
            ui.label(RichText::new(format!("{} apps · {} checked · {off} excluded", total, total - off)).color(theme::text_dim()));
            ui.add_space(12.0);
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("{} Search apps", icon::MAGNIFYING_GLASS)).desired_width(200.0));
            if ui.small_button("Check all").clicked() {
                let names: Vec<String> = self.installed.iter().map(|a| a.name.clone()).collect();
                let _ = appupdates::set_excluded(&names, false);
                self.excluded = appupdates::excluded();
            }
        });
        ui.add_space(6.0);
        let q = self.search.trim().to_lowercase();
        let rows: Vec<InstalledApp> = self.installed.iter().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.publisher.to_lowercase().contains(&q)).cloned().collect();
        let mut toggle: Option<(String, bool)> = None;
        // The page already scrolls; a second scroll area inside it would fight it.
        TableBuilder::new(ui)
            .vscroll(false)
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(52.0))
            .column(Column::remainder().at_least(260.0).clip(true))
            .column(Column::exact(160.0).clip(true))
            .column(Column::exact(220.0).clip(true))
            .header(22.0, |mut h| {
                for t in ["Check", "App", "Version", "Publisher"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().color(theme::text_dim()));
                    });
                }
            })
            .body(|body| {
                body.rows(28.0, rows.len(), |mut row| {
                    let a = &rows[row.index()];
                    let on = !self.is_excluded(&a.name);
                    row.col(|ui| {
                        if theme::switch(ui, on, theme::green(), !busy).on_hover_text(if on { "Checked for updates. Click to exclude." } else { "Excluded. Click to check it again." }).clicked() {
                            toggle = Some((a.name.clone(), on));
                        }
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&a.name).color(if on { theme::text() } else { theme::text_dim() })).truncate());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&a.version).small().monospace().color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&a.publisher).small().color(theme::text_dim())).truncate());
                    });
                });
            });
        if let Some((name, exclude)) = toggle {
            let _ = appupdates::set_excluded(&[name], exclude);
            self.excluded = appupdates::excluded();
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        self.spawn(ctx, "Checking for app updates…", || {
            let r = appupdates::scan();
            Msg::AppScan(r, appupdates::known().ok())
        });
    }

    fn start_batch(&mut self, ctx: &egui::Context, list: Vec<Upgrade>, download_only: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (c, s, l) = (ctx.clone(), stop.clone(), list.clone());
        std::thread::spawn(move || appupdates::update(l, download_only, tx, s, c));
        let n = list.len();
        self.batch = Some(Batch { list, download_only, rx, stop, current: None, results: vec![None; n], line: String::new(), pct: None, log: Vec::new(), started: Instant::now(), done: false, log_file: None });
    }

    fn batch_card(&mut self, ui: &mut Ui) {
        let Some(b) = &self.batch else { return };
        let mut close = false;
        theme::card_frame().stroke(Stroke::new(1.5, if b.done { theme::border() } else { theme::accent() })).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if !b.done {
                    ui.spinner();
                }
                let what = if b.download_only { "Downloading" } else { "Updating" };
                let title = if b.done { format!("{} finished", if b.download_only { "Download" } else { "Update" }) } else { format!("{what} {} of {}", b.current.map(|i| i + 1).unwrap_or(0), b.list.len()) };
                ui.label(RichText::new(title).font(FontId::new(16.0, theme::heading_family())));
                let s = b.started.elapsed().as_secs();
                ui.label(RichText::new(format!("· {}:{:02}", s / 60, s % 60)).color(theme::text_dim()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if b.done {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                        if b.download_only && ui.button(format!("{} Open folder", icon::FOLDER_OPEN)).clicked() {
                            shell::reveal_in_explorer(&appupdates::download_dir());
                        }
                        if let Some(l) = &b.log_file {
                            if ui.button(format!("{} Log", icon::FILE_TEXT)).clicked() {
                                shell::open(&l.display().to_string());
                            }
                        }
                    } else if b.list.len() > 1 {
                        let stopping = b.stop.load(Ordering::Relaxed);
                        if ui.add_enabled(!stopping, egui::Button::new(format!("{} {}", icon::STOP, if stopping { "Stopping after this one…" } else { "Stop after this one" }))).clicked() {
                            b.stop.store(true, Ordering::Relaxed);
                        }
                    }
                });
            });
            if !b.done {
                ui.add(Label::new(RichText::new(if b.line.is_empty() { "Starting…" } else { &b.line }).color(theme::text_dim())).truncate());
                let bar = match b.pct {
                    Some(p) => egui::ProgressBar::new(p).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar.fill(theme::accent()));
            }
            for (i, u) in b.list.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (ic, c, text) = match (&b.results[i], b.current == Some(i)) {
                        (_, true) => (icon::CIRCLE_NOTCH, theme::accent(), if b.download_only { "Downloading…".to_string() } else { "Updating…".to_string() }),
                        (None, _) => (icon::CIRCLE, theme::text_dim(), "Waiting".to_string()),
                        (Some(Outcome::Installed), _) => (icon::CHECK_CIRCLE, theme::green(), if b.download_only { "Downloaded".to_string() } else { format!("Updated to {}", u.available) }),
                        (Some(Outcome::AlreadyInstalled), _) => (icon::CHECK_CIRCLE, theme::green(), "Already up to date".to_string()),
                        (Some(Outcome::RestartNeeded), _) => (icon::ARROW_CLOCKWISE, theme::yellow(), "Updated; restart Windows to finish".to_string()),
                        (Some(Outcome::Skipped), _) => (icon::MINUS_CIRCLE, theme::text_dim(), "Skipped".to_string()),
                        (Some(Outcome::OpenedWebsite), _) => (icon::GLOBE, theme::aqua(), "Opened in the browser".to_string()),
                        (Some(Outcome::Failed(e)), _) => (icon::X_CIRCLE, theme::red(), e.clone()),
                    };
                    ui.label(RichText::new(ic).color(c));
                    ui.label(RichText::new(&u.name).strong());
                    ui.add(Label::new(RichText::new(text).color(c)).truncate());
                });
            }
            if !b.log.is_empty() {
                egui::CollapsingHeader::new("Details").id_salt("upd_log").show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                        for l in &b.log {
                            ui.label(RichText::new(l).small().monospace().color(theme::text_dim()));
                        }
                    });
                });
            }
        });
        ui.add_space(12.0);
        if close {
            self.batch = None;
        }
    }
}

fn or_never(s: &str) -> &str {
    if s.is_empty() { "never" } else { s }
}

fn section(ui: &mut Ui, ic: &str, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(20.0).color(theme::accent()));
        ui.label(RichText::new(title).font(FontId::new(17.0, theme::heading_family())));
    });
    if !blurb.is_empty() {
        ui.add(Label::new(RichText::new(blurb).color(theme::text_dim())).wrap());
    }
    ui.add_space(6.0);
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
                let reserve = if button.is_some() { 220.0 } else { 0.0 };
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - reserve).max(100.0));
                    ui.add(Label::new(text).wrap());
                });
                if let Some((label, clicked)) = button {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        *clicked = ui.button(label).clicked();
                    });
                }
            });
        });
    ui.add_space(6.0);
}

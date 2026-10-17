//! Driver Manager screen.

use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Label, Layout, Margin, RichText, Stroke, Ui, vec2};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::devices::{self, Device, problem_text};
use super::gpu::{self, Gpu, NvidiaDriver, Vendor};
use super::oem::{self, Maker, Pc};
use super::wu::{self, DriverUpdate};
use crate::theme;
use crate::toast::Toasts;
use crate::win::fileinfo::format_bytes;
use crate::win::{elevation, shell};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Devices,
    Updates,
    Maker,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Problems,
    ThirdParty,
    Old,
}

enum Done {
    Scan(Result<Vec<Device>, String>, Pc),
    Check(Result<Vec<DriverUpdate>, String>, Vec<(String, Result<NvidiaDriver, String>)>),
    Op(String, Result<String, String>, bool),
    Line(String),
}

pub struct DriverManager {
    tab: Tab,
    devices: Option<Result<Vec<Device>, String>>,
    pc: Option<Pc>,
    rx: Option<Receiver<Done>>,
    busy_label: String,
    check_rx: Option<Receiver<Done>>,
    updates: Option<Result<Vec<DriverUpdate>, String>>,
    nvidia: Vec<(String, Result<NvidiaDriver, String>)>,
    studio: bool,
    selected: BTreeSet<String>,
    filter: Filter,
    search: String,
    log: Vec<String>,
    check_on_load: bool,
    show_serial: bool,
}

fn third_party(d: &Device) -> bool {
    d.inf.to_lowercase().starts_with("oem")
}

impl DriverManager {
    pub fn new() -> Self {
        Self {
            tab: Tab::Overview,
            devices: None,
            pc: None,
            rx: None,
            busy_label: String::new(),
            check_rx: None,
            updates: None,
            nvidia: Vec::new(),
            studio: false,
            selected: BTreeSet::new(),
            filter: Filter::All,
            search: String::new(),
            log: Vec::new(),
            check_on_load: false,
            show_serial: false,
        }
    }

    pub fn set_tab(&mut self, name: &str) {
        self.tab = match name.to_lowercase().as_str() {
            "devices" => Tab::Devices,
            "problems" => {
                self.filter = Filter::Problems;
                Tab::Devices
            }
            "thirdparty" => {
                self.filter = Filter::ThirdParty;
                Tab::Devices
            }
            "updates" => {
                // Opening the updates tab from the command line checks right away.
                self.check_on_load = true;
                Tab::Updates
            }
            "maker" | "oem" => Tab::Maker,
            _ => Tab::Overview,
        };
    }

    pub fn summary_line(&self) -> Option<String> {
        let devs = self.devices.as_ref()?.as_ref().ok()?;
        let problems = devs.iter().filter(|d| d.problem != 0).count();
        Some(match (problems, self.update_count()) {
            (0, None) => "All devices working".into(),
            (0, Some(0)) => "All devices working, drivers up to date".into(),
            (p, u) => {
                let mut s = if p > 0 { format!("{p} device(s) need attention") } else { "All devices working".into() };
                if let Some(u) = u.filter(|u| *u > 0) {
                    s.push_str(&format!(" · {u} update(s)"));
                }
                s
            }
        })
    }

    fn gpus(&self) -> Vec<Gpu> {
        self.devices.as_ref().and_then(|r| r.as_ref().ok()).map(|d| gpu::gpus(d)).unwrap_or_default()
    }

    /// Windows Update drivers plus newer NVIDIA drivers, once checked.
    fn update_count(&self) -> Option<usize> {
        let wu = self.updates.as_ref()?.as_ref().map(|v| v.len()).unwrap_or(0);
        let gpus = self.gpus();
        let nv = self
            .nvidia
            .iter()
            .filter(|(dev, r)| r.as_ref().is_ok_and(|d| gpus.iter().any(|g| &g.dev_id == dev && g.vendor_version().is_some_and(|v| gpu::newer(&d.version, &v)))))
            .count();
        Some(wu + nv)
    }

    fn busy(&self) -> bool {
        self.rx.is_some()
    }

    fn scan(&mut self, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let d = devices::scan();
            let _ = tx.send(Done::Scan(d, oem::pc()));
            c.request_repaint();
        });
        self.busy_label = "Reading your devices…".into();
        self.rx = Some(rx);
    }

    /// Windows Update search and NVIDIA lookups, in the background.
    fn check(&mut self, ctx: &egui::Context) {
        if self.check_rx.is_some() {
            return;
        }
        let gpus: Vec<Gpu> = self.gpus().into_iter().filter(|g| g.vendor == Vendor::Nvidia).collect();
        let studio = self.studio;
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let laptop = gpu::is_laptop();
            let nv: Vec<(String, Result<NvidiaDriver, String>)> = gpus.iter().map(|g| (g.dev_id.clone(), gpu::nvidia_latest(&g.dev_id, laptop, studio))).collect();
            let w = wu::search();
            let _ = tx.send(Done::Check(w, nv));
            c.request_repaint();
        });
        self.check_rx = Some(rx);
    }

    fn run(&mut self, ctx: &egui::Context, label: &str, rescan: bool, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (c, t) = (ctx.clone(), label.to_string());
        std::thread::spawn(move || {
            let r = f();
            let _ = tx.send(Done::Op(t, r, rescan));
            c.request_repaint();
        });
        self.busy_label = format!("{label}…");
        self.rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if let Some(rx) = &self.check_rx {
            if let Ok(Done::Check(w, nv)) = rx.try_recv() {
                self.check_rx = None;
                if let Ok(v) = &w {
                    self.selected = v.iter().map(|u| u.id.clone()).collect();
                }
                self.updates = Some(w);
                self.nvidia = nv;
                match self.update_count() {
                    Some(0) => toasts.success(ctx, "Your drivers are up to date."),
                    Some(n) => toasts.info(ctx, format!("{n} driver update(s) available.")),
                    None => {}
                }
            }
        }
        let Some(rx) = &self.rx else { return };
        let msgs: Vec<Done> = rx.try_iter().collect();
        for m in msgs {
            match m {
                Done::Scan(d, pc) => {
                    self.rx = None;
                    self.devices = Some(d);
                    self.pc = Some(pc);
                    if std::mem::take(&mut self.check_on_load) {
                        self.check(ctx);
                    }
                }
                Done::Line(l) => {
                    self.busy_label = l.clone();
                    self.log.push(l);
                }
                Done::Op(t, r, rescan) => {
                    self.rx = None;
                    match r {
                        Ok(m) => toasts.success(ctx, m),
                        Err(e) => toasts.error(ctx, format!("{t}: {e}")),
                    }
                    if rescan {
                        self.scan(ctx);
                        self.updates = None;
                    }
                }
                Done::Check(..) => {}
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if self.devices.is_none() && self.rx.is_none() {
            self.scan(&ctx);
        }
        self.poll(&ctx, toasts);
        if self.busy() || self.check_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
        egui::Panel::top("drv_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.devices.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(theme::accent()));
                    ui.label(RichText::new("Reading your devices and drivers…").color(theme::text_dim()));
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 20 }).show(ui, |ui| match self.tab {
                    Tab::Overview => self.overview(ui, elevated),
                    Tab::Devices => self.devices_tab(ui, elevated),
                    Tab::Updates => self.updates_tab(ui, elevated),
                    Tab::Maker => self.maker_tab(ui),
                });
            });
        });
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CIRCUITRY).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Driver Manager");
                ui.label(RichText::new("Find missing and outdated drivers and get them from Windows Update, NVIDIA and your PC's maker.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let checking = self.check_rx.is_some();
                if ui.add_enabled(!checking && self.devices.is_some(), egui::Button::new(RichText::new(format!("{} Check for driver updates", icon::MAGNIFYING_GLASS)).color(theme::on_accent())).fill(theme::accent()).min_size(vec2(0.0, 32.0))).on_hover_text("Asks Windows Update and NVIDIA. Takes up to a minute.").clicked() {
                    self.check(&ctx);
                }
                if ui.add_enabled(!self.busy(), egui::Button::new(format!("{} Rescan devices", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.scan(&ctx);
                }
                if checking || self.busy() {
                    ui.spinner();
                    ui.label(RichText::new(if checking { "Checking Windows Update and NVIDIA…" } else { &self.busy_label }).color(theme::text_dim()));
                }
            });
        });
        ui.add_space(6.0);
        if !elevated {
            let mut clicked = false;
            banner(ui, theme::orange(), icon::LOCK, "Checking works now. Installing drivers and rescanning hardware need administrator rights.", Some(("Restart as administrator", &mut clicked)));
            if clicked {
                match elevation::relaunch_as_admin() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => toasts.error(&ctx, e),
                }
            }
        }
        ui.horizontal(|ui| {
            let problems = self.devices.as_ref().and_then(|r| r.as_ref().ok()).map(|d| d.iter().filter(|x| x.problem != 0).count()).unwrap_or(0);
            let ups = self.update_count().filter(|n| *n > 0);
            for (t, ic, label) in [
                (Tab::Overview, icon::SQUARES_FOUR, "Overview".to_string()),
                (Tab::Devices, icon::LIST_BULLETS, if problems > 0 { format!("Devices ({problems} need attention)") } else { "Devices".into() }),
                (Tab::Updates, icon::DOWNLOAD_SIMPLE, match ups { Some(n) => format!("Driver updates ({n})"), None => "Driver updates".into() }),
                (Tab::Maker, icon::DESKTOP, "PC maker".to_string()),
            ] {
                let sel = self.tab == t;
                let text = RichText::new(format!("{ic} {label}")).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    // ------------------------------------------------------------------ Overview

    fn overview(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let devs = match self.devices.as_ref().unwrap() {
            Ok(d) => d.clone(),
            Err(e) => {
                ui.label(RichText::new(e).color(theme::red()));
                return;
            }
        };
        let problems: Vec<&Device> = devs.iter().filter(|d| d.problem != 0).collect();
        let missing = problems.iter().filter(|d| d.problem == 28).count();
        let gpus = self.gpus();
        let w = ui.available_width();
        let cols = if w > 1000.0 { 4.0 } else { 2.0 };
        let card_w = ((w - 10.0 * (cols - 1.0)) / cols).floor() - 1.0;
        ui.with_layout(Layout::left_to_right(Align::Min).with_main_wrap(true), |ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            // Devices
            let (c, ic, head, sub) = if problems.is_empty() {
                (theme::green(), icon::CHECK_CIRCLE, "All devices working".to_string(), format!("{} devices have drivers.", devs.iter().filter(|d| d.has_driver()).count()))
            } else {
                (theme::red(), icon::WARNING_CIRCLE, format!("{} device(s) need attention", problems.len()), if missing > 0 { format!("{missing} without a driver.") } else { "See Devices for details.".into() })
            };
            if stat(ui, card_w, c, ic, &head, &sub).clicked() {
                self.tab = Tab::Devices;
                self.filter = if problems.is_empty() { Filter::All } else { Filter::Problems };
            }
            // Updates
            let (c, ic, head, sub) = match (&self.updates, self.check_rx.is_some()) {
                (_, true) => (theme::accent(), icon::CIRCLE_NOTCH, "Checking…".to_string(), "Windows Update and NVIDIA".to_string()),
                (None, _) => (theme::text_dim(), icon::MAGNIFYING_GLASS, "Not checked yet".into(), "Click Check for driver updates.".into()),
                (Some(Err(e)), _) => (theme::red(), icon::WARNING, "Couldn't check".into(), e.clone()),
                (Some(Ok(_)), _) => match self.update_count().unwrap_or(0) {
                    0 => (theme::green(), icon::CHECK_CIRCLE, "Drivers up to date".into(), "Nothing newer on Windows Update or NVIDIA.".into()),
                    n => (theme::yellow(), icon::DOWNLOAD_SIMPLE, format!("{n} update(s) available"), "Open Driver updates to install.".into()),
                },
            };
            if stat(ui, card_w, c, ic, &head, &sub).clicked() {
                if self.updates.is_none() && self.check_rx.is_none() {
                    self.check(&ctx);
                }
                self.tab = Tab::Updates;
            }
            // Graphics
            let gtext = gpus.iter().map(|g| g.name.replace("(TM)", "").replace("(R)", "")).collect::<Vec<_>>().join(" + ");
            if stat(ui, card_w, theme::accent(), icon::GRAPHICS_CARD, if gpus.is_empty() { "No graphics card found" } else { "Graphics" }, &gtext).clicked() {
                self.tab = Tab::Updates;
            }
            // PC
            let pc = self.pc.clone().unwrap_or_default();
            let mk = oem::maker(&pc);
            let sub = format!("BIOS {} ({})", pc.bios, if pc.bios_date.is_empty() { "date unknown" } else { &pc.bios_date });
            if stat(ui, card_w, theme::purple(), icon::DESKTOP, &format!("{} {}", mk.as_ref().map(|m| m.name).unwrap_or(&pc.manufacturer), pc.model), &sub).clicked() {
                self.tab = Tab::Maker;
            }
        });
        ui.add_space(14.0);

        if !problems.is_empty() {
            section(ui, icon::WARNING_CIRCLE, "Devices that need attention", "Click Check for driver updates: Windows Update has drivers for most devices. Otherwise use the device's ⋯ menu.");
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                for d in &problems {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::WARNING_CIRCLE).color(theme::red()));
                        ui.label(RichText::new(&d.name).strong());
                        ui.label(RichText::new(format!("{} (code {})", problem_text(d.problem), d.problem)).color(theme::text_dim()));
                    });
                }
            });
            ui.add_space(14.0);
        }

        section(ui, icon::LIGHTBULB, "Good to know", "");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for t in [
                "Windows Update carries drivers from the makers of most hardware. It's the safest source, since every driver there is tested and signed.",
                "Graphics cards get newer drivers from NVIDIA, AMD and Intel directly. Wield checks NVIDIA's driver service for your exact card.",
                "Chipset, BIOS and laptop-specific drivers (hotkeys, fans, audio enhancements) come from your PC's maker. See PC maker.",
                "Before big driver changes, back up your drivers (Backup & Restore → Reinstall kit) or create a restore point.",
            ] {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("•").color(theme::accent()));
                    ui.add(Label::new(t).wrap());
                });
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("{} Open Device Manager", icon::ARROW_SQUARE_OUT)).clicked() {
                    shell::open("devmgmt.msc");
                }
                if ui.add_enabled(elevated && !self.busy(), egui::Button::new(format!("{} Scan for hardware changes", icon::ARROWS_CLOCKWISE))).on_disabled_hover_text("Needs administrator rights").on_hover_text("Asks Windows to detect new hardware and install drivers it already has").clicked() {
                    self.run(&ctx, "Hardware scan", true, devices::rescan);
                }
            });
        });
    }

    // ------------------------------------------------------------------ Devices

    fn devices_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let Some(Ok(devs)) = self.devices.clone() else { return };
        ui.horizontal(|ui| {
            for (f, label) in [(Filter::All, "All"), (Filter::Problems, "Need attention"), (Filter::ThirdParty, "Non-Windows drivers"), (Filter::Old, "Older than 3 years")] {
                ui.selectable_value(&mut self.filter, f, label);
            }
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("{} Search devices", icon::MAGNIFYING_GLASS)).desired_width(220.0));
        });
        let year_now = 1970 + (crate::tools::backup::sys::now_unix() / 31_557_600) as u32;
        let q = self.search.trim().to_lowercase();
        let rows: Vec<&Device> = devs
            .iter()
            .filter(|d| match self.filter {
                Filter::All => true,
                Filter::Problems => d.problem != 0,
                Filter::ThirdParty => third_party(d),
                Filter::Old => third_party(d) && d.year().is_some_and(|y| y + 3 < year_now),
            })
            .filter(|d| q.is_empty() || d.name.to_lowercase().contains(&q) || d.class.to_lowercase().contains(&q) || d.provider.to_lowercase().contains(&q) || d.hardware_id.to_lowercase().contains(&q))
            .collect();
        ui.label(RichText::new(format!("{} device(s)", rows.len())).color(theme::text_dim()));
        ui.add_space(4.0);
        let mut action: Option<(&str, Device)> = None;
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(28.0))
            .column(Column::remainder().at_least(240.0).clip(true))
            .column(Column::exact(130.0).clip(true))
            .column(Column::exact(170.0).clip(true))
            .column(Column::exact(150.0).clip(true))
            .column(Column::exact(96.0))
            .column(Column::exact(40.0))
            .header(22.0, |mut h| {
                for t in ["", "Device", "Type", "Driver from", "Version", "Date", ""] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().color(theme::text_dim()));
                    });
                }
            })
            .body(|body| {
                body.rows(28.0, rows.len(), |mut row| {
                    let d = rows[row.index()];
                    row.col(|ui| {
                        let (ic, c) = if d.problem != 0 { (icon::WARNING_CIRCLE, theme::red()) } else { (icon::CHECK_CIRCLE, theme::green()) };
                        ui.label(RichText::new(ic).color(c)).on_hover_text(problem_text(d.problem));
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&d.name).color(if d.problem != 0 { theme::red() } else { theme::text() })).truncate()).on_hover_text(format!("{}\n{}\n{}", d.hardware_id, d.device_id, if d.problem != 0 { problem_text(d.problem) } else { "" }));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&d.class).small().color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        let p = if d.provider.is_empty() { "No driver".to_string() } else { d.provider.clone() };
                        ui.add(Label::new(RichText::new(p).color(if third_party(d) { theme::text() } else { theme::text_dim() })).truncate());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&d.version).small().monospace());
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(&d.date).small());
                    });
                    row.col(|ui| {
                        ui.menu_button(icon::DOTS_THREE_VERTICAL, |ui| {
                            if ui.button(format!("{} Check Windows Update", icon::MAGNIFYING_GLASS)).clicked() {
                                action = Some(("wu", d.clone()));
                                ui.close();
                            }
                            if ui.button(format!("{} Search the web for this hardware ID", icon::GLOBE)).clicked() {
                                shell::search_web(&format!("{} driver", d.search_id()));
                                ui.close();
                            }
                            if ui.add_enabled(elevated, egui::Button::new(format!("{} Install a driver from a folder…", icon::FOLDER_OPEN))).on_disabled_hover_text("Needs administrator rights").clicked() {
                                action = Some(("folder", d.clone()));
                                ui.close();
                            }
                            if ui.button(format!("{} Copy hardware ID", icon::COPY)).clicked() {
                                ui.ctx().copy_text(d.hardware_id.clone());
                                ui.close();
                            }
                            if ui.button(format!("{} Open Device Manager", icon::ARROW_SQUARE_OUT)).clicked() {
                                shell::open("devmgmt.msc");
                                ui.close();
                            }
                        });
                    });
                });
            });
        if let Some((what, _d)) = action {
            match what {
                "wu" => {
                    self.tab = Tab::Updates;
                    self.check(&ctx);
                }
                _ => {
                    // Pick a folder, then install every driver in it (pnputil).
                    let (tx, rx) = std::sync::mpsc::channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || {
                        let r = match crate::win::with_com(|| shell::pick_folder("Choose the folder with the driver (.inf files)")) {
                            Some(dir) => crate::tools::backup::kit::restore_drivers(&dir),
                            None => Err("No folder chosen.".into()),
                        };
                        let _ = tx.send(Done::Op("Install driver".into(), r, true));
                        c.request_repaint();
                    });
                    self.busy_label = "Installing driver…".into();
                    self.rx = Some(rx);
                }
            }
        }
    }

    // ------------------------------------------------------------------ Updates

    fn updates_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        let checking = self.check_rx.is_some();
        section(ui, icon::GRAPHICS_CARD, "Graphics drivers", "Straight from the graphics card makers. Games need current graphics drivers more than anything else.");
        let gpus = self.gpus();
        let laptop_note = gpus.len() > 1;
        for g in &gpus {
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::GRAPHICS_CARD).size(20.0).color(theme::accent()));
                    ui.label(RichText::new(&g.name).font(FontId::new(16.0, theme::heading_family())));
                    theme::badge(ui, g.vendor.name(), theme::text_dim());
                });
                let installed = g.vendor_version().unwrap_or_else(|| g.windows_version.clone());
                ui.label(format!("Installed driver: {installed}{}", if g.date.is_empty() { String::new() } else { format!(" ({})", g.date) }));
                match g.vendor {
                    Vendor::Nvidia => {
                        let r = self.nvidia.iter().find(|(d, _)| d == &g.dev_id).map(|(_, r)| r.clone());
                        ui.horizontal(|ui| {
                            ui.label("Driver type:");
                            let before = self.studio;
                            ui.radio_value(&mut self.studio, false, "Game Ready");
                            ui.radio_value(&mut self.studio, true, "Studio (for creative apps)");
                            if before != self.studio {
                                self.nvidia.clear();
                                if !checking {
                                    self.check(&ctx);
                                }
                            }
                        });
                        match r {
                            None if checking => {
                                ui.label(RichText::new("Asking NVIDIA…").color(theme::text_dim()));
                            }
                            None => {
                                ui.label(RichText::new("Click Check for driver updates to ask NVIDIA for the newest driver for this card.").color(theme::text_dim()));
                            }
                            Some(Err(e)) => {
                                ui.label(RichText::new(e).color(theme::red()));
                            }
                            Some(Ok(d)) => {
                                let newer = gpu::newer(&d.version, &installed);
                                ui.horizontal(|ui| {
                                    if newer {
                                        ui.label(RichText::new(format!("{} {} {} available (released {})", icon::DOWNLOAD_SIMPLE, d.name, d.version, d.date)).color(theme::yellow()).strong());
                                    } else {
                                        ui.label(RichText::new(format!("{} Up to date: {} {} is the newest", icon::CHECK_CIRCLE, d.name, d.version)).color(theme::green()));
                                    }
                                });
                                ui.horizontal(|ui| {
                                    let label = if newer { format!("{} Download and install {}", icon::DOWNLOAD_SIMPLE, d.version) } else { format!("{} Reinstall {}", icon::ARROW_CLOCKWISE, d.version) };
                                    let btn = if newer { egui::Button::new(RichText::new(label).color(theme::on_accent())).fill(theme::accent()) } else { egui::Button::new(label) };
                                    if ui.add_enabled(!busy, btn).on_hover_text("Downloads NVIDIA's installer (about 700 MB), checks it's signed by NVIDIA, then opens it. Choose Express or Custom → Clean installation there.").clicked() {
                                        let d2 = d.clone();
                                        self.run(&ctx, "NVIDIA driver", false, move || {
                                            let f = gpu::download_nvidia(&d2)?;
                                            crate::win::shell::open(&f.display().to_string());
                                            Ok(format!("NVIDIA's installer for {} is open. Follow its steps; the screen may flicker while it installs.", d2.version))
                                        });
                                        self.busy_label = format!("Downloading NVIDIA driver {} (about 700 MB)", d.version);
                                    }
                                    if !d.details.is_empty() && ui.button(format!("{} Release notes", icon::ARROW_SQUARE_OUT)).clicked() {
                                        shell::open(&d.details);
                                    }
                                });
                            }
                        }
                    }
                    Vendor::Amd => {
                        ui.label(RichText::new("AMD doesn't offer a public driver lookup, so Wield can't compare versions. AMD's own installer detects your card and installs the right driver.").color(theme::text_dim()));
                        ui.horizontal(|ui| {
                            if ui.button(format!("{} AMD drivers & Auto-Detect tool", icon::ARROW_SQUARE_OUT)).clicked() {
                                shell::open("https://www.amd.com/en/support/download/drivers.html");
                            }
                        });
                    }
                    Vendor::Intel => {
                        ui.label(RichText::new("Intel Driver & Support Assistant detects your Intel graphics, Wi-Fi and Bluetooth and keeps them current.").color(theme::text_dim()));
                        if ui.button(format!("{} Intel Driver & Support Assistant", icon::ARROW_SQUARE_OUT)).clicked() {
                            shell::open("https://www.intel.com/content/www/us/en/support/detect.html");
                        }
                    }
                    Vendor::Other => {
                        ui.label(RichText::new("Check Windows Update below or the card maker's website.").color(theme::text_dim()));
                    }
                }
            });
            ui.add_space(8.0);
        }
        if laptop_note {
            ui.label(RichText::new(format!("{} This PC switches between two graphics chips. If the newest drivers cause problems, use the versions from your PC's maker (PC maker tab), which are tuned for it.", icon::INFO)).small().color(theme::text_dim()));
        }
        ui.add_space(14.0);

        section(ui, icon::WINDOWS_LOGO, "Windows Update drivers", "Drivers that hardware makers publish through Windows Update for the devices in this PC, tested and signed by Microsoft.");
        if wu::drivers_blocked_by_policy() {
            banner(ui, theme::orange(), icon::WARNING, "A policy on this PC keeps drivers out of Windows Update (common on work PCs), so the list may be empty.", None);
        }
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &self.updates {
                None if checking => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Searching Windows Update. This can take up to a minute…");
                    });
                }
                None => {
                    ui.label(RichText::new("Click Check for driver updates.").color(theme::text_dim()));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                Some(Ok(v)) if v.is_empty() => {
                    ui.label(RichText::new(format!("{} Windows Update has no newer drivers for this PC.", icon::CHECK_CIRCLE)).color(theme::green()));
                }
                Some(Ok(v)) => {
                    let v = v.clone();
                    for u in &v {
                        ui.horizontal(|ui| {
                            let mut on = self.selected.contains(&u.id);
                            if ui.checkbox(&mut on, RichText::new(&u.title).strong()).changed() {
                                if on {
                                    self.selected.insert(u.id.clone());
                                } else {
                                    self.selected.remove(&u.id);
                                }
                            }
                            ui.label(RichText::new(format!("{} · {} · {} · {}", u.manufacturer, u.class, u.date, format_bytes(u.size))).small().color(theme::text_dim()));
                        });
                    }
                    ui.add_space(6.0);
                    let n = self.selected.len();
                    ui.horizontal(|ui| {
                        if ui.add_enabled(n > 0 && elevated && !busy, egui::Button::new(RichText::new(format!("{} Install {n} selected", icon::DOWNLOAD_SIMPLE)).color(theme::on_accent())).fill(theme::accent())).on_disabled_hover_text(if elevated { "Select drivers first" } else { "Needs administrator rights" }).clicked() {
                            let ids: Vec<String> = self.selected.iter().cloned().collect();
                            let (tx, rx) = std::sync::mpsc::channel();
                            let (otx, orx) = std::sync::mpsc::channel();
                            let c = ctx.clone();
                            std::thread::spawn(move || wu::install(ids, tx, c));
                            // Relay Windows Update progress into this screen's channel.
                            let c2 = ctx.clone();
                            std::thread::spawn(move || {
                                for p in rx {
                                    let m = match p {
                                        wu::Progress::Line(l) => Done::Line(l),
                                        wu::Progress::Done(r) => Done::Op("Windows Update".into(), r, true),
                                    };
                                    let _ = otx.send(m);
                                    c2.request_repaint();
                                }
                            });
                            self.log.clear();
                            self.busy_label = "Installing drivers from Windows Update…".into();
                            self.rx = Some(orx);
                        }
                        ui.label(RichText::new("A restore point is a good idea first (Backup & Restore → System Restore).").small().color(theme::text_dim()));
                    });
                }
            }
            if !self.log.is_empty() {
                egui::CollapsingHeader::new("Details").id_salt("wu_log").show(ui, |ui| {
                    for l in &self.log {
                        ui.label(RichText::new(l).small().monospace().color(theme::text_dim()));
                    }
                });
            }
            ui.add_space(4.0);
            if ui.button(format!("{} Open Windows Update optional updates", icon::ARROW_SQUARE_OUT)).clicked() {
                shell::open("ms-settings:windowsupdate-optionalupdates");
            }
        });
    }

    // ------------------------------------------------------------------ PC maker

    fn maker_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let pc = self.pc.clone().unwrap_or_default();
        let mk: Option<Maker> = oem::maker(&pc);
        section(ui, icon::DESKTOP, "This PC", "Your PC's maker publishes drivers made for this exact model: chipset, audio, touchpad, hotkeys, fan control and BIOS updates.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("pc_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                for (k, v) in [("Maker", pc.manufacturer.clone()), ("Model", pc.model.clone()), ("Motherboard", format!("{} {}", pc.board_maker, pc.board)), ("BIOS", format!("{}  ({})", pc.bios, pc.bios_date))] {
                    ui.label(RichText::new(k).color(theme::text_dim()));
                    ui.label(v);
                    ui.end_row();
                }
                ui.label(RichText::new("Serial number").color(theme::text_dim()));
                ui.horizontal(|ui| {
                    // Hidden by default: it identifies this PC and is easy to leak in a screenshot.
                    let shown = if self.show_serial { pc.serial.clone() } else { "•".repeat(pc.serial.chars().count().min(12)) };
                    ui.label(RichText::new(shown).monospace());
                    if !pc.serial.is_empty() {
                        if ui.small_button(if self.show_serial { "Hide" } else { "Show" }).clicked() {
                            self.show_serial = !self.show_serial;
                        }
                        if ui.small_button(icon::COPY).on_hover_text("Copy").clicked() {
                            ui.ctx().copy_text(pc.serial.clone());
                        }
                    }
                });
                ui.end_row();
            });
        });
        ui.add_space(10.0);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &mk {
                None => {
                    ui.label(format!("Wield doesn't know {}'s support site. Search for your model's driver page:", if pc.manufacturer.is_empty() { "this maker" } else { &pc.manufacturer }));
                    if ui.button(format!("{} Search for {} {} drivers", icon::GLOBE, pc.manufacturer, pc.model)).clicked() {
                        shell::search_web(&format!("{} {} drivers support", pc.manufacturer, pc.model));
                    }
                }
                Some(m) => {
                    ui.label(RichText::new(format!("{} drivers", m.name)).font(FontId::new(16.0, theme::heading_family())));
                    ui.label(RichText::new(m.note).color(theme::text_dim()));
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        if !m.support_url.is_empty() && ui.button(format!("{} Open {}'s driver page", icon::ARROW_SQUARE_OUT, m.name)).clicked() {
                            shell::open(&m.support_url);
                        }
                        if let Some((tool, id)) = m.tool {
                            let has_winget = crate::tools::apps::winget::winget_path().is_some();
                            let r = ui.add_enabled(has_winget && !self.busy(), egui::Button::new(format!("{} Install {tool}", icon::DOWNLOAD_SIMPLE)));
                            let r = r.on_disabled_hover_text("Needs winget: install it from App Installer first").on_hover_text(format!("{tool} is {}'s official app for drivers, firmware and BIOS updates.", m.name));
                            if r.clicked() {
                                let id = id.to_string();
                                let tool = tool.to_string();
                                self.run(&ctx, &format!("Installing {tool}"), false, move || {
                                    let wg = crate::tools::apps::winget::winget_path().ok_or("winget isn't installed.")?;
                                    let (code, out) = crate::tools::backup::sys::run(
                                        &wg.display().to_string(),
                                        &["install", "--id", &id, "--exact", "--source", "winget", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"],
                                    )?;
                                    match crate::tools::apps::winget::outcome(code, &out) {
                                        crate::tools::apps::winget::Outcome::Failed(e) => Err(e),
                                        _ => Ok(format!("{tool} is installed. Open it to check for {}'s driver and BIOS updates.", id.split('.').next().unwrap_or(""))),
                                    }
                                });
                            }
                        }
                    });
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new(format!("{} BIOS updates: only install them with the maker's tool or page, keep a laptop plugged in, and never turn the PC off while it updates.", icon::WARNING)).small().color(theme::yellow()));
        });
    }
}

fn stat(ui: &mut Ui, w: f32, color: Color32, ic: &str, head: &str, sub: &str) -> egui::Response {
    let r = egui::Frame::new()
        .fill(theme::card())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            // Frame width = content + 2×14 margin + 2×1 stroke.
            ui.set_width(w - 30.0);
            ui.set_min_height(72.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic).size(24.0).color(color));
                ui.vertical(|ui| {
                    ui.add(Label::new(RichText::new(head).strong().color(color)).truncate());
                    ui.add(Label::new(RichText::new(sub).small().color(theme::text_dim())).wrap());
                });
            });
        });
    r.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
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

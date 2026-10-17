//! Backup & Restore screen: System Restore, system images, folder backups and the reinstall kit.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::image::{self, Check, Frequency, History, Level, Target, Version};
use super::restore::{self, RestorePoint, Storage};
use super::sys::{self, BitLocker, DriveKind, Volume};
use super::{files, kit};
use crate::theme;
use crate::toast::Toasts;
use crate::win::fileinfo::format_bytes;
use crate::win::{elevation, shell, with_com};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Restore,
    Image,
    Files,
    Kit,
}

/// Everything the screen shows that takes a moment to read.
struct Info {
    volumes: Vec<Volume>,
    points: Option<Result<Vec<RestorePoint>, String>>,
    storage: Vec<Storage>,
    protected: Vec<String>,
    daily: bool,
    winre: Option<bool>,
    bitlocker: BitLocker,
    image_task: Option<(String, String, String)>,
    files_task: bool,
}

fn load_info(elevated: bool, ctx: egui::Context) -> Receiver<Info> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let storage = if elevated { restore::storage().unwrap_or_default() } else { Vec::new() };
        let info = Info {
            volumes: sys::volumes(),
            points: elevated.then(restore::list),
            storage: storage.clone(),
            protected: restore::protected_drives(&storage),
            daily: restore::daily_scheduled(),
            winre: if elevated { image::winre_enabled().ok() } else { None },
            bitlocker: sys::bitlocker(&sys::system_drive()),
            image_task: image::scheduled(),
            files_task: files::scheduled(),
        };
        let _ = tx.send(info);
        ctx.request_repaint();
    });
    rx
}

/// A long-running job with live output.
enum Long {
    Image(Receiver<image::Progress>),
    Files(Receiver<files::Progress>),
    Kit(Receiver<kit::Progress>),
}

struct Running {
    title: String,
    job: Long,
    status: String,
    pct: Option<f32>,
    log: Vec<String>,
    started: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pick {
    FilesDest,
    FilesAdd,
    FilesRestoreFrom,
    KitDest,
    KitDrivers,
    KitWifi,
    KitWinget,
}

/// Dialogs that need an explicit confirmation.
enum Confirm {
    Restore(RestorePoint),
    Delete(RestorePoint),
    DeleteAll,
    Recovery,
    FilesRestore(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ShareMode {
    Drive,
    Share,
}

pub struct BackupManager {
    tab: Tab,
    info: Option<Info>,
    info_rx: Option<Receiver<Info>>,
    op_rx: Option<Receiver<(String, Result<String, String>, bool)>>,
    pick_rx: Option<Receiver<(Pick, Option<PathBuf>)>>,
    running: Option<Running>,
    history: Vec<History>,
    confirm: Option<Confirm>,
    confirm_text: String,
    restart_prompt: bool,
    last_result: Option<Result<String, String>>,
    // System Restore
    new_point: String,
    max_pct: u32,
    // System image
    mode: ShareMode,
    drive: Option<String>,
    share: String,
    share_user: String,
    share_pass: String,
    extra: Vec<String>,
    versions: Option<Result<Vec<Version>, String>>,
    versions_rx: Option<Receiver<Result<Vec<Version>, String>>>,
    freq: Option<Frequency>,
    // Files
    folders: Vec<(String, PathBuf, bool)>,
    files_dest: Option<PathBuf>,
    mirror: bool,
    // Kit
    kit_dest: Option<PathBuf>,
    kit_parts: [bool; 5],
}

impl BackupManager {
    pub fn new() -> Self {
        Self {
            tab: Tab::Image,
            info: None,
            info_rx: None,
            op_rx: None,
            pick_rx: None,
            running: None,
            history: image::history(),
            confirm: None,
            confirm_text: String::new(),
            restart_prompt: false,
            last_result: None,
            new_point: String::new(),
            max_pct: 10,
            mode: ShareMode::Drive,
            drive: None,
            share: String::new(),
            share_user: String::new(),
            share_pass: String::new(),
            extra: Vec::new(),
            versions: None,
            versions_rx: None,
            freq: None,
            folders: files::default_folders().into_iter().map(|(n, p)| (n, p, true)).collect(),
            files_dest: None,
            mirror: false,
            kit_dest: None,
            kit_parts: [true; 5],
        }
    }

    /// Selects a tab by name (used by `--tab` on the command line).
    pub fn set_tab(&mut self, name: &str) {
        self.tab = match name.to_lowercase().as_str() {
            "restore" | "restorepoints" => Tab::Restore,
            "files" | "folders" => Tab::Files,
            "kit" => Tab::Kit,
            _ => Tab::Image,
        };
    }

    pub fn summary_line(&self) -> Option<String> {
        let last = self.history.iter().filter(|h| h.ok).max_by_key(|h| h.when);
        Some(match last {
            Some(h) => format!("Last backup ({}) {}", h.kind.to_lowercase(), ago(h.when)),
            None => "No backups made with Wield yet".into(),
        })
    }

    fn busy(&self) -> bool {
        self.running.is_some() || self.op_rx.is_some()
    }

    fn reload(&mut self, ctx: &egui::Context, elevated: bool) {
        if self.info_rx.is_none() {
            self.info_rx = Some(load_info(elevated, ctx.clone()));
        }
    }

    /// Runs a quick operation in the background; `restart` offers a restart when it succeeds.
    fn op(&mut self, ctx: &egui::Context, title: &str, restart: bool, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let title = title.to_string();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((title, f(), restart));
            ctx2.request_repaint();
        });
        self.op_rx = Some(rx);
    }

    fn pick(&mut self, ctx: &egui::Context, what: Pick, title: &'static str) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((what, with_com(|| shell::pick_folder(title))));
            ctx.request_repaint();
        });
        self.pick_rx = Some(rx);
    }

    fn start(&mut self, title: &str, job: Long) {
        self.last_result = None;
        self.running = Some(Running { title: title.into(), job, status: "Starting…".into(), pct: None, log: Vec::new(), started: Instant::now() });
    }

    fn poll(&mut self, ctx: &egui::Context, elevated: bool, toasts: &mut Toasts) {
        if let Some(rx) = &self.info_rx {
            if let Ok(info) = rx.try_recv() {
                self.info_rx = None;
                if self.drive.is_none() {
                    // Default to the largest suitable drive on another disk.
                    let sysd = info.volumes.iter().find(|v| v.letter == sys::system_drive()).and_then(|v| v.disk);
                    self.drive = info
                        .volumes
                        .iter()
                        .filter(|v| v.kind == DriveKind::Fixed && v.letter != sys::system_drive())
                        .max_by_key(|v| (v.disk != sysd, v.free))
                        .map(|v| v.letter.clone());
                }
                self.info = Some(info);
            }
        }
        if let Some(rx) = &self.versions_rx {
            if let Ok(v) = rx.try_recv() {
                self.versions_rx = None;
                self.versions = Some(v);
            }
        }
        if let Some(rx) = &self.op_rx {
            if let Ok((title, r, restart)) = rx.try_recv() {
                self.op_rx = None;
                match r {
                    Ok(m) => {
                        toasts.success(ctx, if m.is_empty() { format!("{title}: done.") } else { m });
                        self.restart_prompt |= restart;
                    }
                    Err(e) => toasts.error(ctx, format!("{title}: {e}")),
                }
                self.info_rx = None;
                self.reload(ctx, elevated);
            }
        }
        if let Some(rx) = &self.pick_rx {
            if let Ok((what, path)) = rx.try_recv() {
                self.pick_rx = None;
                if let Some(p) = path {
                    self.picked(ctx, what, p);
                }
            }
        }
        let mut finished = None;
        if let Some(run) = &mut self.running {
            match &run.job {
                Long::Image(rx) => {
                    for m in rx.try_iter() {
                        match m {
                            image::Progress::Line(l) => {
                                run.status = l.clone();
                                run.log.push(l);
                            }
                            image::Progress::Percent(what, p) => {
                                run.status = format!("Backing up {what}");
                                run.pct = Some(p as f32 / 100.0);
                            }
                            image::Progress::Done(r) => finished = Some(r),
                        }
                    }
                }
                Long::Files(rx) => {
                    for m in rx.try_iter() {
                        match m {
                            files::Progress::Folder(name, i, n) => {
                                run.status = format!("{name} ({} of {n})", i + 1);
                                run.pct = Some(i as f32 / n.max(1) as f32);
                            }
                            files::Progress::Line(l) => run.log.push(l),
                            files::Progress::Done(r) => finished = Some(r),
                        }
                    }
                }
                Long::Kit(rx) => {
                    for m in rx.try_iter() {
                        match m {
                            kit::Progress::Step(s) => {
                                run.status = s.clone();
                                run.log.push(s);
                            }
                            kit::Progress::Done(r) => finished = Some(r),
                        }
                    }
                }
            }
            if run.log.len() > 400 {
                let cut = run.log.len() - 400;
                run.log.drain(..cut);
            }
        }
        if let Some(r) = finished {
            let title = self.running.take().map(|r| r.title).unwrap_or_default();
            match &r {
                Ok(m) => toasts.success(ctx, m.clone()),
                Err(e) => toasts.error(ctx, format!("{title}: {e}")),
            }
            self.last_result = Some(r);
            self.history = image::history();
            self.versions = None;
            self.reload(ctx, elevated);
        }
    }

    fn picked(&mut self, ctx: &egui::Context, what: Pick, p: PathBuf) {
        match what {
            Pick::FilesDest => self.files_dest = Some(p),
            Pick::FilesAdd => {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Folder".into());
                let mut unique = name.clone();
                let mut i = 2;
                while self.folders.iter().any(|f| f.0.eq_ignore_ascii_case(&unique)) {
                    unique = format!("{name} {i}");
                    i += 1;
                }
                if !self.folders.iter().any(|f| f.1 == p) {
                    self.folders.push((unique, p, true));
                }
            }
            Pick::FilesRestoreFrom => {
                self.confirm_text.clear();
                self.confirm = Some(Confirm::FilesRestore(p));
            }
            Pick::KitDest => self.kit_dest = Some(p),
            Pick::KitDrivers => {
                let dir = if p.join("Drivers").is_dir() { p.join("Drivers") } else { p };
                self.op(ctx, "Restore drivers", false, move || kit::restore_drivers(&dir));
            }
            Pick::KitWifi => {
                let dir = if p.join("WiFi").is_dir() { p.join("WiFi") } else { p };
                self.op(ctx, "Restore Wi-Fi", false, move || kit::restore_wifi(&dir));
            }
            Pick::KitWinget => {
                let file = p.join("winget-apps.json");
                if file.is_file() {
                    // Visible console so the user can follow winget and answer its prompts.
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("cmd.exe")
                        .raw_arg(format!("/K winget import -i \"{}\" --accept-package-agreements --accept-source-agreements", file.display()))
                        .creation_flags(0x0000_0010)
                        .spawn();
                } else {
                    self.last_result = Some(Err(format!("No winget-apps.json in {}.", p.display())));
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if self.info.is_none() && self.info_rx.is_none() {
            self.reload(&ctx, elevated);
        }
        self.poll(&ctx, elevated, toasts);
        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::Panel::top("backup_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.info.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(theme::accent()));
                    ui.label(RichText::new("Checking drives, restore points and recovery settings…").color(theme::text_dim()));
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 20 }).show(ui, |ui| {
                    self.running_card(ui);
                    match self.tab {
                        Tab::Restore => self.restore_tab(ui, elevated),
                        Tab::Image => self.image_tab(ui, elevated),
                        Tab::Files => self.files_tab(ui),
                        Tab::Kit => self.kit_tab(ui, elevated),
                    }
                });
            });
        });

        self.dialogs(&ctx);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::FLOPPY_DISK).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Backup & Restore");
                ui.label(RichText::new("Full system images you can restore from the boot menu, restore points, folder backups and a reinstall kit.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(self.info_rx.is_none(), egui::Button::new(format!("{} Refresh", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.versions = None;
                    self.reload(&ctx, elevated);
                }
                if self.info_rx.is_some() || self.op_rx.is_some() {
                    ui.spinner();
                }
            });
        });
        ui.add_space(6.0);
        if !elevated && self.tab != Tab::Files {
            let mut clicked = false;
            banner(
                ui,
                theme::orange(),
                icon::LOCK,
                "System images, restore points, recovery settings and driver backups need administrator rights. Folder backups work without them.",
                Some(("Restart as administrator", &mut clicked)),
            );
            if clicked {
                match elevation::relaunch_as_admin() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => toasts.error(&ctx, e),
                }
            }
        }
        ui.horizontal(|ui| {
            for (t, ic, label) in [
                (Tab::Image, icon::HARD_DRIVES, "System image"),
                (Tab::Restore, icon::CLOCK_COUNTER_CLOCKWISE, "System Restore"),
                (Tab::Files, icon::FOLDERS, "Files & folders"),
                (Tab::Kit, icon::TOOLBOX, "Reinstall kit"),
            ] {
                let sel = self.tab == t;
                let text = RichText::new(format!("{ic} {label}")).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    fn running_card(&mut self, ui: &mut Ui) {
        if let Some(run) = &self.running {
            let mut stop = false;
            theme::card_frame().stroke(Stroke::new(1.5, theme::accent())).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(&run.title).font(FontId::new(16.0, theme::heading_family())));
                    ui.label(RichText::new(format!("· {}", elapsed(run.started))).color(theme::text_dim()));
                    if matches!(run.job, Long::Image(_)) {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            stop = ui.button(format!("{} Stop", icon::STOP)).on_hover_text("Cancel the backup. A cancelled image can't be restored.").clicked();
                        });
                    }
                });
                ui.add(Label::new(RichText::new(&run.status).color(theme::text_dim())).truncate());
                let bar = match run.pct {
                    Some(p) => egui::ProgressBar::new(p).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar.fill(theme::accent()));
                if !run.log.is_empty() {
                    egui::CollapsingHeader::new("Details").id_salt("run_log").show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
                            for l in &run.log {
                                ui.label(RichText::new(l).small().monospace().color(theme::text_dim()));
                            }
                        });
                    });
                }
                ui.label(RichText::new("You can keep using your PC. Don't shut it down or unplug the backup drive until this finishes.").small().color(theme::orange()));
            });
            ui.add_space(12.0);
            if stop {
                let ctx = ui.ctx().clone();
                self.op(&ctx, "Stop backup", false, || image::stop().map(|_| "Backup cancelled.".into()));
            }
        } else if let Some(r) = &self.last_result {
            let (c, ic, text) = match r {
                Ok(m) => (theme::green(), icon::CHECK_CIRCLE, m.clone()),
                Err(e) => (theme::red(), icon::WARNING_CIRCLE, e.clone()),
            };
            let mut close = false;
            banner(ui, c, ic, &text, Some(("Dismiss", &mut close)));
            if close {
                self.last_result = None;
            }
            ui.add_space(6.0);
        }
    }

    // ------------------------------------------------------------------ System image

    fn target(&self) -> Option<Target> {
        match self.mode {
            ShareMode::Drive => self.drive.clone().map(Target::Drive),
            ShareMode::Share => {
                (!self.share.trim().is_empty()).then(|| Target::Share { path: self.share.trim().into(), user: self.share_user.trim().into(), password: self.share_pass.clone() })
            }
        }
    }

    fn image_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let Some(info) = &self.info else { return };
        let volumes = info.volumes.clone();
        let (winre, bitlocker, task) = (info.winre, info.bitlocker, info.image_task.clone());
        let sysd = sys::system_drive();

        section(ui, icon::HARD_DRIVES, "Create a system image", "A complete copy of Windows, your programs, settings and files. If Windows breaks or the disk dies, restore everything from the boot menu.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("Save to").strong());
                ui.radio_value(&mut self.mode, ShareMode::Drive, "A drive");
                ui.radio_value(&mut self.mode, ShareMode::Share, "A network folder");
            });
            ui.add_space(4.0);
            match self.mode {
                ShareMode::Drive => {
                    let candidates: Vec<&Volume> = volumes.iter().filter(|v| v.letter != sysd && matches!(v.kind, DriveKind::Fixed | DriveKind::Removable | DriveKind::Network)).collect();
                    if candidates.is_empty() {
                        ui.label(RichText::new(format!("{} Connect an external drive (USB hard disk or SSD) and press Refresh.", icon::PLUGS)).color(theme::orange()));
                    }
                    let current = self.drive.clone();
                    egui::ComboBox::from_id_salt("img_drive")
                        .width(420.0)
                        .selected_text(current.as_ref().and_then(|d| volumes.iter().find(|v| &v.letter == d)).map(vol_text).unwrap_or_else(|| "Choose a drive".into()))
                        .show_ui(ui, |ui| {
                            for v in &candidates {
                                if ui.selectable_label(current.as_deref() == Some(&v.letter), vol_text(v)).clicked() {
                                    self.drive = Some(v.letter.clone());
                                    self.extra.retain(|e| e != &v.letter);
                                    self.versions = None;
                                }
                            }
                        });
                }
                ShareMode::Share => {
                    egui::Grid::new("share_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Folder");
                        if ui.add(egui::TextEdit::singleline(&mut self.share).hint_text(r"\\server\backups").desired_width(320.0)).changed() {
                            self.versions = None;
                        }
                        ui.end_row();
                        ui.label("User name");
                        ui.add(egui::TextEdit::singleline(&mut self.share_user).hint_text("optional, e.g. NAS\\me").desired_width(320.0));
                        ui.end_row();
                        ui.label("Password");
                        ui.add(egui::TextEdit::singleline(&mut self.share_pass).password(true).desired_width(320.0));
                        ui.end_row();
                    });
                    ui.label(RichText::new("The password is only passed to Windows Backup for this run; Wield doesn't store it.").small().color(theme::text_dim()));
                }
            }

            let chosen = if self.mode == ShareMode::Drive { self.drive.clone() } else { None };
            let others: Vec<&Volume> = volumes.iter().filter(|v| v.kind == DriveKind::Fixed && v.letter != sysd && Some(&v.letter) != chosen.as_ref()).collect();
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Included: {sysd} (Windows) and the hidden boot and recovery partitions")).color(theme::text_dim()));
            if !others.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Also include:");
                    for v in others {
                        let mut on = self.extra.contains(&v.letter);
                        if ui.checkbox(&mut on, format!("{} {} ({} used)", v.letter, v.label, format_bytes(v.used()))).changed() {
                            if on {
                                self.extra.push(v.letter.clone());
                            } else {
                                self.extra.retain(|e| e != &v.letter);
                            }
                        }
                    }
                });
            }
        });
        ui.add_space(8.0);

        let target = self.target();
        let checks: Vec<Check> = match &target {
            Some(t) => image::preflight(t, &self.extra, &volumes, elevated, winre, bitlocker),
            None => vec![Check { level: Level::Block, text: "Choose where to save the image.".into() }],
        };
        let blocked = checks.iter().any(|c| c.level == Level::Block);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Before you start").strong());
            for c in &checks {
                let (ic, col) = match c.level {
                    Level::Ok => (icon::CHECK_CIRCLE, theme::green()),
                    Level::Warn => (icon::WARNING, theme::yellow()),
                    Level::Block => (icon::X_CIRCLE, theme::red()),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ic).color(col));
                    ui.add(Label::new(&c.text).wrap());
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = format!("{} Create system image now", icon::PLAY);
                let r = ui.add_enabled(!blocked && !self.busy(), egui::Button::new(RichText::new(label).color(theme::on_accent())).fill(theme::accent()).min_size(egui::vec2(0.0, 32.0)));
                if r.clicked() {
                    if let Some(t) = target.clone() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let extra = self.extra.clone();
                        let c = ctx.clone();
                        std::thread::spawn(move || image::run_backup(t, extra, tx, c));
                        self.start("Creating system image", Long::Image(rx));
                    }
                }
                ui.label(RichText::new(format!("Needs about {}. Usually 15–60 minutes.", format_bytes(image::estimated_size(&volumes, &self.extra)))).color(theme::text_dim()));
            });
        });
        ui.add_space(8.0);

        // Versions on the chosen target.
        if let Some(t) = &target {
            theme::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Images on {}", t.label())).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(elevated && self.versions_rx.is_none(), egui::Button::new(format!("{} Check", icon::MAGNIFYING_GLASS))).clicked() {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let t = t.clone();
                            let c = ctx.clone();
                            std::thread::spawn(move || {
                                let _ = tx.send(image::versions(&t));
                                c.request_repaint();
                            });
                            self.versions_rx = Some(rx);
                        }
                        if self.versions_rx.is_some() {
                            ui.spinner();
                        }
                    });
                });
                match &self.versions {
                    None => {
                        ui.label(RichText::new("Press Check to list the images already saved there.").color(theme::text_dim()));
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(theme::red()));
                    }
                    Some(Ok(v)) if v.is_empty() => {
                        ui.label(RichText::new("No system images there yet.").color(theme::text_dim()));
                    }
                    Some(Ok(v)) => {
                        for ver in v {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(icon::DISC).color(theme::accent()));
                                ui.label(&ver.time);
                                if ver.bootable() {
                                    theme::badge(ui, "Restorable from boot", theme::green());
                                } else {
                                    theme::badge(ui, "Files only", theme::yellow());
                                }
                            });
                        }
                    }
                }
            });
            ui.add_space(8.0);
        }

        // Schedule.
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Automatic system images").strong());
            match &task {
                Some((next, last, _)) => {
                    ui.label(format!("{} On. Next run: {next}. Last result: {}", icon::CALENDAR_CHECK, if last == "267011" { "not run yet".into() } else if last == "0" { "success".to_string() } else { format!("code {last}") }));
                }
                None => {
                    ui.label(RichText::new("Off. Images are only made when you start one.").color(theme::text_dim()));
                }
            }
            ui.horizontal(|ui| {
                let freq = self.freq.get_or_insert(Frequency::Weekly);
                ui.radio_value(freq, Frequency::Weekly, "Every Sunday at 2:00");
                ui.radio_value(freq, Frequency::Monthly, "1st of every month at 2:00");
                let f = *freq;
                let can = elevated && !self.busy() && matches!(target, Some(Target::Drive(_))) && !blocked;
                if ui.add_enabled(can, egui::Button::new(format!("{} {}", icon::CALENDAR_PLUS, if task.is_some() { "Update" } else { "Turn on" }))).on_disabled_hover_text("Choose a local drive that passes the checks above (run as administrator)").clicked() {
                    let t = target.clone().unwrap();
                    self.op(&ctx, "Schedule system image", false, move || image::schedule(&t, f).map(|_| "Automatic system images are on.".into()));
                }
                if task.is_some() && ui.add_enabled(elevated && !self.busy(), egui::Button::new("Turn off")).clicked() {
                    self.op(&ctx, "Schedule system image", false, || image::unschedule().map(|_| "Automatic system images are off.".into()));
                }
            });
            ui.label(RichText::new("The PC must be on (or asleep with wake timers allowed) and the backup drive connected. Windows keeps older images until the drive fills up.").small().color(theme::text_dim()));
        });
        ui.add_space(14.0);

        section(ui, icon::LIFEBUOY, "Restore a system image", "Replaces everything on the PC with the image. Your files made since the image was taken are lost, so copy them somewhere first.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let steps = [
                "Connect the drive that holds the image.",
                "Restart to the recovery options (button below). If Windows doesn't start at all, it opens them by itself after two failed starts, or boot from a recovery USB or Windows install USB and choose \"Repair your computer\".",
                "Choose Troubleshoot → Advanced options → See more recovery options → System Image Recovery.",
                "Pick the image (newest is preselected) and follow the steps. Leave \"Format and repartition disks\" off unless you're restoring to a new disk.",
            ];
            for (i, s) in steps.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}.", i + 1)).strong().color(theme::accent()));
                    ui.add(Label::new(*s).wrap());
                });
            }
            if bitlocker == BitLocker::On {
                ui.label(RichText::new(format!("{} BitLocker is on: you'll need the recovery key in the recovery environment. Save it in the Reinstall kit tab now.", icon::KEY)).color(theme::orange()));
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                match winre {
                    Some(true) => {
                        theme::badge(ui, "Recovery environment: on", theme::green());
                    }
                    Some(false) => {
                        theme::badge(ui, "Recovery environment: off", theme::red());
                        if ui.add_enabled(!self.busy(), egui::Button::new("Turn it on")).clicked() {
                            self.op(&ctx, "Windows RE", false, || image::enable_winre().map(|_| "Windows Recovery Environment enabled.".into()));
                        }
                    }
                    None => {
                        theme::badge(ui, "Recovery environment: unknown (needs admin)", theme::text_dim());
                    }
                }
                if ui.add_enabled(elevated && !self.busy(), egui::Button::new(format!("{} Restart to recovery options…", icon::ARROW_CLOCKWISE))).clicked() {
                    self.confirm_text.clear();
                    self.confirm = Some(Confirm::Recovery);
                }
                if ui.button(format!("{} Create a recovery USB drive", icon::USB)).on_hover_text("Opens Windows' Recovery Drive creator. Use it to restore when the PC can't start at all.").clicked() {
                    shell::open("RecoveryDrive.exe");
                }
            });
        });
        ui.add_space(14.0);
        self.history_card(ui, "System image");
    }

    fn history_card(&mut self, ui: &mut Ui, kind: &str) {
        let items: Vec<&History> = self.history.iter().rev().filter(|h| h.kind == kind).take(8).collect();
        if items.is_empty() {
            return;
        }
        section(ui, icon::LIST_CHECKS, "History", "Backups made with Wield on this PC.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for h in items {
                ui.horizontal(|ui| {
                    if h.ok {
                        ui.label(RichText::new(icon::CHECK_CIRCLE).color(theme::green()));
                    } else {
                        ui.label(RichText::new(icon::X_CIRCLE).color(theme::red()));
                    }
                    ui.label(crate::tools::startup::journal::format_time(h.when));
                    ui.label(RichText::new(format!("→ {}", h.target)).color(theme::text_dim()));
                    if let Some(l) = &h.log {
                        if ui.small_button("Log").clicked() {
                            shell::open(l);
                        }
                    }
                });
            }
        });
    }

    // ------------------------------------------------------------------ System Restore

    fn restore_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let Some(info) = &self.info else { return };
        let volumes: Vec<Volume> = info.volumes.iter().filter(|v| v.kind == DriveKind::Fixed && v.fs.eq_ignore_ascii_case("NTFS")).cloned().collect();
        let protected = info.protected.clone();
        let storage = info.storage.clone();
        let daily = info.daily;
        let points = info.points.clone();
        let sysd = sys::system_drive();
        let busy = self.busy();

        section(ui, icon::SHIELD_CHECK, "Protection", "Restore points undo changes to Windows, drivers, programs and settings. Your documents and photos aren't touched.");
        if elevated && !protected.contains(&sysd) {
            banner(ui, theme::red(), icon::WARNING, &format!("System Protection is off for {sysd}. Without it there are no restore points. Turn it on below."), None);
        }
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for v in &volumes {
                let on = protected.contains(&v.letter);
                ui.horizontal(|ui| {
                    if theme::switch(ui, on, theme::green(), elevated && !busy).clicked() {
                        let d = v.letter.clone();
                        self.op(&ctx, "System Protection", false, move || {
                            restore::set_protection(&d, !on)?;
                            if !on {
                                // A sensible default size so points aren't purged immediately.
                                let _ = restore::set_max_size(&d, 8);
                            }
                            Ok(format!("System Protection {} for {d}.", if on { "turned off" } else { "turned on" }))
                        });
                    }
                    ui.label(RichText::new(format!("{} {}", v.letter, if v.letter == sysd { "(Windows)" } else { &v.label })).strong());
                    if let Some(s) = storage.iter().find(|s| s.volume == v.letter) {
                        ui.label(RichText::new(format!("using {} of up to {}", s.used, s.max)).color(theme::text_dim()));
                    } else if on {
                        ui.label(RichText::new("on").color(theme::text_dim()));
                    }
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(format!("Space for restore points on {sysd}"));
                ui.add(egui::Slider::new(&mut self.max_pct, 2..=30).suffix("%"));
                if ui.add_enabled(elevated && !busy, egui::Button::new("Apply")).clicked() {
                    let (d, p) = (sysd.clone(), self.max_pct);
                    self.op(&ctx, "Restore point space", false, move || restore::set_max_size(&d, p).map(|_| format!("Restore points on {d} may now use up to {p}% of the drive.")));
                }
            });
            ui.label(RichText::new("More space keeps more points. When it fills up, Windows deletes the oldest ones.").small().color(theme::text_dim()));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if theme::switch(ui, daily, theme::green(), elevated && !busy).clicked() {
                    self.op(&ctx, "Daily restore point", false, move || restore::set_daily(!daily).map(|_| if daily { "Daily restore points are off.".into() } else { "A restore point will be made every day.".into() }));
                }
                ui.label("Make a restore point every day");
            });
        });
        ui.add_space(8.0);

        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Create a restore point").strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_point).hint_text("e.g. Before installing new graphics driver").desired_width(360.0));
                let can = elevated && !busy && protected.contains(&sysd);
                if ui.add_enabled(can, egui::Button::new(RichText::new(format!("{} Create", icon::PLUS)).color(theme::on_accent())).fill(theme::accent())).clicked() {
                    let desc = if self.new_point.trim().is_empty() { "Wield - manual restore point".to_string() } else { format!("Wield - {}", self.new_point.trim()) };
                    self.new_point.clear();
                    self.op(&ctx, "Restore point", false, move || restore::create(&desc).map(|p| format!("Restore point created: {}", p.description)));
                }
            });
            ui.label(RichText::new("Wield skips Windows' once-a-day limit, so you always get a fresh point. Takes up to a minute.").small().color(theme::text_dim()));
        });
        ui.add_space(8.0);

        section(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Restore points", "Going back removes programs and drivers installed after that point and undoes setting changes. Windows makes an undo point first.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &points {
                None => {
                    ui.label(RichText::new("Run Wield as administrator to see restore points.").color(theme::text_dim()));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::red()));
                }
                Some(Ok(p)) if p.is_empty() => {
                    ui.label(RichText::new("No restore points yet. Create one above.").color(theme::text_dim()));
                }
                Some(Ok(p)) => {
                    let p = p.clone();
                    TableBuilder::new(ui)
                        .striped(true)
                        .column(Column::exact(130.0))
                        .column(Column::remainder().at_least(200.0))
                        .column(Column::exact(130.0))
                        .column(Column::exact(170.0))
                        .header(22.0, |mut h| {
                            for t in ["Created", "Description", "Type", ""] {
                                h.col(|ui| {
                                    ui.label(RichText::new(t).small().color(theme::text_dim()));
                                });
                            }
                        })
                        .body(|mut body| {
                            for (i, pt) in p.iter().enumerate() {
                                body.row(28.0, |mut row| {
                                    row.col(|ui| {
                                        ui.label(&pt.time);
                                    });
                                    row.col(|ui| {
                                        ui.add(Label::new(&pt.description).truncate());
                                        if i == 0 {
                                            theme::badge(ui, "Newest", theme::accent());
                                        }
                                    });
                                    row.col(|ui| {
                                        ui.label(RichText::new(pt.kind_label()).color(theme::text_dim()));
                                    });
                                    row.col(|ui| {
                                        if ui.add_enabled(!busy, egui::Button::new(format!("{} Restore", icon::ARROW_COUNTER_CLOCKWISE))).clicked() {
                                            self.confirm_text.clear();
                                            self.confirm = Some(Confirm::Restore(pt.clone()));
                                        }
                                        if ui.add_enabled(!busy, egui::Button::new(icon::TRASH)).on_hover_text("Delete this restore point").clicked() {
                                            self.confirm_text.clear();
                                            self.confirm = Some(Confirm::Delete(pt.clone()));
                                        }
                                    });
                                });
                            }
                        });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("{} Delete all restore points", icon::TRASH)).color(theme::red()))).clicked() {
                            self.confirm_text.clear();
                            self.confirm = Some(Confirm::DeleteAll);
                        }
                        if ui.button(format!("{} Open Windows System Restore", icon::ARROW_SQUARE_OUT)).on_hover_text("Windows' own wizard, including \"Scan for affected programs\"").clicked() {
                            shell::open("rstrui.exe");
                        }
                    });
                }
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("If Windows won't start: open the recovery options (they appear after two failed starts) → Troubleshoot → Advanced options → System Restore.")
                    .small()
                    .color(theme::text_dim()),
            );
        });
    }

    // ------------------------------------------------------------------ Files

    fn files_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(info) = &self.info else { return };
        let volumes = info.volumes.clone();
        let task = info.files_task;
        let busy = self.busy();

        section(ui, icon::FOLDERS, "Back up your folders", "Copies your files to another drive or a network folder. Only new and changed files are copied after the first run, and an interrupted backup just picks up where it stopped.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Folders").strong());
            let mut remove = None;
            for (i, (name, path, on)) in self.folders.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(on, RichText::new(name.as_str()).strong());
                    ui.add(Label::new(RichText::new(path.display().to_string()).color(theme::text_dim())).truncate());
                    if i >= 6 && ui.small_button(icon::X).on_hover_text("Remove from the list").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.folders.remove(i);
            }
            if ui.add_enabled(self.pick_rx.is_none(), egui::Button::new(format!("{} Add a folder…", icon::FOLDER_PLUS))).clicked() {
                self.pick(&ctx, Pick::FilesAdd, "Choose a folder to back up");
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Save to").strong());
                ui.label(self.files_dest.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not chosen".into()));
                if ui.add_enabled(self.pick_rx.is_none(), egui::Button::new(format!("{} Choose…", icon::FOLDER_OPEN))).clicked() {
                    self.pick(&ctx, Pick::FilesDest, "Choose where to save the backup (a drive or folder)");
                }
            });
            ui.checkbox(&mut self.mirror, "Mirror: also remove files from the backup that you deleted from the PC")
                .on_hover_text("Keeps the backup an exact copy. Leave off to keep deleted files in the backup, which is safer.");
        });
        ui.add_space(8.0);

        let selected: Vec<(String, PathBuf)> = self.folders.iter().filter(|f| f.2).map(|f| (f.0.clone(), f.1.clone())).collect();
        let mut checks = Vec::new();
        match &self.files_dest {
            None => checks.push(Check { level: Level::Block, text: "Choose where to save the backup.".into() }),
            Some(d) => {
                if selected.iter().any(|(_, s)| d.starts_with(s)) {
                    checks.push(Check { level: Level::Block, text: "The backup can't be saved inside a folder that's being backed up.".into() });
                }
                let letter = d.to_string_lossy().get(..2).map(|s| s.to_uppercase()).unwrap_or_default();
                if let Some(v) = volumes.iter().find(|v| v.letter == letter) {
                    let sys_disk = volumes.iter().find(|x| x.letter == sys::system_drive()).and_then(|x| x.disk);
                    if v.letter == sys::system_drive() || (v.disk.is_some() && v.disk == sys_disk) {
                        checks.push(Check { level: Level::Warn, text: "This is on the same disk as your files. It protects against mistakes, not against the disk failing.".into() });
                    }
                    checks.push(Check { level: Level::Ok, text: format!("{} free on {}.", format_bytes(v.free), v.letter) });
                }
                if files::read_manifest(d).is_some() {
                    checks.push(Check { level: Level::Ok, text: "A previous backup is there; only changes will be copied.".into() });
                }
            }
        }
        if selected.is_empty() {
            checks.push(Check { level: Level::Block, text: "Select at least one folder.".into() });
        }
        let blocked = checks.iter().any(|c| c.level == Level::Block);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for c in &checks {
                let (ic, col) = match c.level {
                    Level::Ok => (icon::CHECK_CIRCLE, theme::green()),
                    Level::Warn => (icon::WARNING, theme::yellow()),
                    Level::Block => (icon::X_CIRCLE, theme::red()),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ic).color(col));
                    ui.add(Label::new(&c.text).wrap());
                });
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(!blocked && !busy, egui::Button::new(RichText::new(format!("{} Back up now", icon::PLAY)).color(theme::on_accent())).fill(theme::accent()).min_size(egui::vec2(0.0, 32.0))).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (dest, mirror, c, f) = (self.files_dest.clone().unwrap(), self.mirror, ctx.clone(), selected.clone());
                    std::thread::spawn(move || files::backup(f, dest, mirror, tx, c));
                    self.start("Backing up folders", Long::Files(rx));
                }
                if let Some(d) = &self.files_dest {
                    let root = files::backup_root(d);
                    if root.is_dir() && ui.button(format!("{} Open backup folder", icon::FOLDER_OPEN)).clicked() {
                        shell::reveal_in_explorer(&root);
                    }
                }
                if !blocked && ui.add_enabled(!busy, egui::Button::new(format!("{} {}", icon::CALENDAR_PLUS, if task { "Update daily backup" } else { "Back up daily at 13:00" }))).clicked() {
                    let (dest, mirror) = (self.files_dest.clone().unwrap(), self.mirror);
                    self.op(&ctx, "Daily folder backup", false, move || files::schedule(&selected, &dest, mirror).map(|_| "Your folders will be backed up every day at 13:00 (while you're signed in).".into()));
                }
                if task && ui.add_enabled(!busy, egui::Button::new("Stop daily backup")).clicked() {
                    self.op(&ctx, "Daily folder backup", false, || files::unschedule().map(|_| "Daily folder backup is off.".into()));
                }
            });
            if task {
                ui.label(RichText::new(format!("{} Daily backup is on.", icon::CALENDAR_CHECK)).color(theme::green()));
            }
        });
        ui.add_space(14.0);

        section(ui, icon::ARROW_U_UP_LEFT, "Restore folders", "Copies the backup back to where the folders were. Files that are newer on the PC are kept, so nothing you've changed since is overwritten.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if let Some(d) = self.files_dest.clone().filter(|d| files::read_manifest(d).is_some()) {
                    if ui.add_enabled(!busy, egui::Button::new(format!("{} Restore from {}", icon::ARROW_COUNTER_CLOCKWISE, d.display()))).clicked() {
                        self.confirm_text.clear();
                        self.confirm = Some(Confirm::FilesRestore(d));
                    }
                }
                if ui.add_enabled(!busy && self.pick_rx.is_none(), egui::Button::new(format!("{} Restore from another location…", icon::FOLDER_OPEN))).clicked() {
                    self.pick(&ctx, Pick::FilesRestoreFrom, "Choose the drive or folder that contains WieldBackup");
                }
            });
            ui.label(RichText::new("To get back a single file, just open the backup folder and copy it.").small().color(theme::text_dim()));
        });
        ui.add_space(14.0);
        self.history_card(ui, "Folders");
    }

    // ------------------------------------------------------------------ Reinstall kit

    fn kit_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        let bitlocker = self.info.as_ref().map(|i| i.bitlocker).unwrap_or(BitLocker::Unknown);

        section(ui, icon::TOOLBOX, "Reinstall kit", "Everything you'll want after a clean Windows install or a new PC, saved to one folder. Put it on a USB stick or another drive, not on the PC itself.");
        if bitlocker == BitLocker::On {
            banner(ui, theme::orange(), icon::KEY, "BitLocker is on. Save the recovery key: without it you can't unlock the drive after a hardware change or to restore an image.", None);
        }
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, p) in kit::Part::ALL.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.kit_parts[i], p.label());
                    if p.needs_admin() && !elevated {
                        ui.label(RichText::new(icon::LOCK).color(theme::orange())).on_hover_text("Needs administrator rights");
                    }
                    if p.sensitive() {
                        ui.label(RichText::new(icon::WARNING).color(theme::yellow())).on_hover_text("Saved in plain text. Keep the kit somewhere private.");
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Save to").strong());
                ui.label(self.kit_dest.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not chosen".into()));
                if ui.add_enabled(self.pick_rx.is_none(), egui::Button::new(format!("{} Choose…", icon::FOLDER_OPEN))).clicked() {
                    self.pick(&ctx, Pick::KitDest, "Choose where to save the reinstall kit");
                }
            });
            if self.kit_parts.iter().enumerate().any(|(i, &on)| on && kit::Part::ALL[i].sensitive()) {
                ui.label(RichText::new(format!("{} Wi-Fi passwords and keys are saved readable so they work on any PC. Keep the kit private.", icon::WARNING)).small().color(theme::yellow()));
            }
            ui.add_space(6.0);
            let parts: Vec<kit::Part> = kit::Part::ALL.iter().enumerate().filter(|(i, _)| self.kit_parts[*i]).map(|(_, p)| *p).collect();
            ui.horizontal(|ui| {
                let can = !busy && self.kit_dest.is_some() && !parts.is_empty();
                if ui.add_enabled(can, egui::Button::new(RichText::new(format!("{} Save reinstall kit", icon::DOWNLOAD_SIMPLE)).color(theme::on_accent())).fill(theme::accent()).min_size(egui::vec2(0.0, 32.0))).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (dest, c) = (self.kit_dest.clone().unwrap(), ctx.clone());
                    std::thread::spawn(move || kit::export(parts, dest, tx, c));
                    self.start("Saving reinstall kit", Long::Kit(rx));
                }
                if let Some(d) = &self.kit_dest {
                    let dir = kit::kit_dir(d);
                    if dir.is_dir() && ui.button(format!("{} Open kit folder", icon::FOLDER_OPEN)).clicked() {
                        shell::reveal_in_explorer(&dir);
                    }
                }
            });
        });
        ui.add_space(14.0);

        section(ui, icon::ARROW_U_UP_LEFT, "Use a kit", "On the new or reinstalled PC, point Wield at the kit's folder.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let free = !busy && self.pick_rx.is_none();
                if ui.add_enabled(free && elevated, egui::Button::new(format!("{} Install drivers…", icon::CPU))).clicked() {
                    self.pick(&ctx, Pick::KitDrivers, "Choose the Reinstall kit folder");
                }
                if ui.add_enabled(free && elevated, egui::Button::new(format!("{} Add Wi-Fi networks…", icon::WIFI_HIGH))).clicked() {
                    self.pick(&ctx, Pick::KitWifi, "Choose the Reinstall kit folder");
                }
                if ui.add_enabled(free, egui::Button::new(format!("{} Reinstall apps with winget…", icon::PACKAGE))).clicked() {
                    self.pick(&ctx, Pick::KitWinget, "Choose the Reinstall kit folder");
                }
            });
            ui.label(RichText::new("The kit's README.txt explains how to do each step by hand too.").small().color(theme::text_dim()));
        });
    }

    // ------------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.restart_prompt {
            let m = egui::Modal::new(Id::new("backup_restart")).show(ctx, |ui| {
                ui.set_width(440.0);
                title(ui, icon::ARROW_CLOCKWISE, theme::accent(), "Restart to finish System Restore");
                ui.add(Label::new("Windows will restore the selected point while it restarts. Save your work and close your apps first. The restore takes 10–30 minutes; don't turn the PC off.").wrap());
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, "Restart now").clicked() {
                        self.restart_prompt = false;
                        let c = ctx.clone();
                        self.op(&c, "Restart", false, || restore::restart_now().map(|_| "Restarting in 10 seconds…".into()));
                    }
                    if ui.button("Later").on_hover_text("The restore runs the next time you restart.").clicked() {
                        self.restart_prompt = false;
                    }
                });
            });
            if m.should_close() {
                self.restart_prompt = false;
            }
        }
        let Some(c) = &self.confirm else { return };
        // (title, icon, color, body, word to type, button)
        let (head, ic, col, body, word, button): (&str, &str, Color32, String, &str, &str) = match c {
            Confirm::Restore(p) => (
                "Restore Windows to this point?",
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::orange(),
                format!(
                    "{}\n{}\n\nPrograms, updates and drivers installed after this point will be removed and settings changed since will be undone. Your personal files stay as they are. \
                     Windows makes an undo point first, so you can reverse this. The PC must restart.",
                    p.time, p.description
                ),
                "RESTORE",
                "Restore",
            ),
            Confirm::Delete(p) => ("Delete this restore point?", icon::TRASH, theme::red(), format!("{}\n{}\n\nThis can't be undone.", p.time, p.description), "DELETE", "Delete"),
            Confirm::DeleteAll => (
                "Delete every restore point?",
                icon::TRASH,
                theme::red(),
                format!("All restore points on {} will be removed, leaving nothing to go back to. Consider creating a fresh one afterwards.", sys::system_drive()),
                "DELETE",
                "Delete all",
            ),
            Confirm::Recovery => (
                "Restart to the recovery options?",
                icon::LIFEBUOY,
                theme::orange(),
                "The PC restarts in 5 seconds into the blue recovery screen. Save your work first.\n\nThen: Troubleshoot → Advanced options → See more recovery options → System Image Recovery. \
                 Choose \"Continue\" there to go back to Windows without restoring."
                    .into(),
                "RESTART",
                "Restart now",
            ),
            Confirm::FilesRestore(d) => (
                "Restore your folders?",
                icon::ARROW_U_UP_LEFT,
                theme::accent(),
                match files::read_manifest(d) {
                    Some(m) => format!(
                        "From {} (backed up {}):\n{}\n\nMissing and older files are copied back. Files that are newer on the PC are kept.",
                        files::backup_root(d).display(),
                        ago(m.when),
                        m.folders.iter().map(|(n, p)| format!("• {n} → {}", p.display())).collect::<Vec<_>>().join("\n")
                    ),
                    None => format!("There's no Wield folder backup for {} in {}.", files::computer_name(), d.display()),
                },
                "",
                "Restore",
            ),
        };
        let valid = !matches!(c, Confirm::FilesRestore(d) if files::read_manifest(d).is_none());
        let mut go = false;
        let mut cancel = false;
        let m = egui::Modal::new(Id::new("backup_confirm")).show(ctx, |ui| {
            ui.set_width(480.0);
            title(ui, ic, col, head);
            ui.add(Label::new(body.as_str()).wrap());
            ui.add_space(10.0);
            if !word.is_empty() {
                ui.label(format!("Type {word} to confirm:"));
                ui.add(egui::TextEdit::singleline(&mut self.confirm_text).desired_width(200.0));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok = valid && (word.is_empty() || self.confirm_text.trim().eq_ignore_ascii_case(word));
                if theme::danger_button(ui, button, col, ok).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if m.should_close() || cancel {
            self.confirm = None;
            return;
        }
        if go {
            let c = self.confirm.take().unwrap();
            match c {
                Confirm::Restore(p) => self.op(ctx, "System Restore", true, move || restore::restore(p.seq).map(|_| "System Restore is ready and will run when you restart.".into())),
                Confirm::Delete(p) => self.op(ctx, "Delete restore point", false, move || restore::delete(&p).map(|_| "Restore point deleted.".into())),
                Confirm::DeleteAll => self.op(ctx, "Delete restore points", false, || restore::delete_all().map(|_| "All restore points deleted.".into())),
                Confirm::Recovery => self.op(ctx, "Restart", false, || image::restart_to_recovery().map(|_| "Restarting to the recovery options…".into())),
                Confirm::FilesRestore(d) => {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let c = ctx.clone();
                    std::thread::spawn(move || files::restore(d, tx, c));
                    self.start("Restoring folders", Long::Files(rx));
                }
            }
        }
    }
}

fn vol_text(v: &Volume) -> String {
    format!("{}  ·  {} free of {}  ·  {}", v.display(), format_bytes(v.free), format_bytes(v.total), if v.fs.is_empty() { "?" } else { &v.fs })
}

fn ago(when: u64) -> String {
    let s = sys::now_unix().saturating_sub(when);
    match s {
        0..=89 => "just now".into(),
        90..=5399 => format!("{} minutes ago", (s + 30) / 60),
        5400..=129_599 => format!("{} hours ago", (s + 1800) / 3600),
        _ => format!("{} days ago", (s + 43_200) / 86_400),
    }
}

fn elapsed(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn title(ui: &mut Ui, ic: &str, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(28.0).color(color));
        ui.label(RichText::new(text).font(FontId::new(18.0, theme::heading_family())));
    });
    ui.add_space(8.0);
}

fn section(ui: &mut Ui, ic: &str, title: &str, blurb: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(20.0).color(theme::accent()));
        ui.label(RichText::new(title).font(FontId::new(17.0, theme::heading_family())));
    });
    ui.add(Label::new(RichText::new(blurb).color(theme::text_dim())).wrap());
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

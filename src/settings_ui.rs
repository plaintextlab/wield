//! Settings screen and the background update checker.

use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, FontId, Label, Layout, Margin, RichText, Ui};
use egui_phosphor::regular as icon;

use crate::settings::{self, Settings};
use crate::theme;
use crate::toast::Toasts;
use crate::update::{self, Mode, Release, Staged};
use crate::win::shell;

enum Msg {
    Checked(Result<Release, String>),
    Staged(Result<Staged, String>, bool),
}

#[derive(Clone)]
enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release),
    Ready(Staged, Release),
    Applied,
    Error(String),
}

pub struct Updater {
    status: Status,
    rx: Option<Receiver<Msg>>,
    mode: Mode,
    /// Install automatically when Wield closes (set by auto-install or "Install when I close").
    install_on_exit: bool,
    last_check: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Updater {
    pub fn new() -> Self {
        update::cleanup();
        Self { status: Status::Idle, rx: None, mode: update::mode(), install_on_exit: false, last_check: Settings::load().last_check_unix }
    }

    /// Starts a background check if automatic checks are on and the last one is old enough.
    pub fn maybe_auto_check(&mut self, s: &Settings, ctx: &egui::Context) {
        if s.auto_check && now().saturating_sub(s.last_check_unix) > 12 * 3600 {
            self.check(ctx);
        }
    }

    pub fn check(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        self.status = Status::Checking;
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Checked(update::fetch_latest()));
            c.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn download(&mut self, r: Release, install_now: bool, ctx: &egui::Context) {
        self.status = Status::Downloading(r.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Staged(update::stage(&r), install_now));
            c.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn install_now(&mut self, staged: Staged, ctx: &egui::Context, toasts: &mut Toasts) {
        match update::apply(&staged, true) {
            Ok(()) => {
                self.status = Status::Applied;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.status = Status::Error(e.clone());
                toasts.error(ctx, e);
            }
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(rx) = &self.rx else { return };
        let Ok(msg) = rx.try_recv() else { return };
        self.rx = None;
        match msg {
            Msg::Checked(Ok(r)) => {
                let s = Settings::update(|s| s.last_check_unix = now());
                self.last_check = s.last_check_unix;
                let skipped = s.skipped_version.as_deref() == Some(update::version_string(r.version).as_str());
                if !r.is_newer() {
                    self.status = Status::UpToDate;
                } else if s.auto_install && !skipped {
                    self.install_on_exit = true;
                    self.download(r, false, ctx);
                } else {
                    if !skipped {
                        toasts.info(ctx, format!("Wield {} is available. Open Settings to update.", update::version_string(r.version)));
                    }
                    self.status = Status::Available(r);
                }
            }
            Msg::Checked(Err(e)) => self.status = Status::Error(e),
            Msg::Staged(Ok(staged), install_now) => {
                let r = match &self.status {
                    Status::Downloading(r) => r.clone(),
                    _ => Release::default(),
                };
                if install_now {
                    self.install_now(staged, ctx, toasts);
                } else {
                    toasts.success(ctx, format!("Wield {} is downloaded and will install when you close Wield.", update::version_string(staged.version)));
                    self.status = Status::Ready(staged, r);
                }
            }
            Msg::Staged(Err(e), _) => {
                toasts.error(ctx, format!("Update failed: {e}"));
                self.status = Status::Error(e);
            }
        }
    }

    /// Called when the app closes: applies a downloaded update without relaunching.
    pub fn on_exit(&mut self) {
        if let Status::Ready(staged, _) = &self.status {
            if self.install_on_exit {
                let _ = update::apply(staged, false);
            }
        }
    }

    pub fn pending_version(&self) -> Option<String> {
        match &self.status {
            Status::Available(r) | Status::Downloading(r) => Some(update::version_string(r.version)),
            Status::Ready(s, _) => Some(update::version_string(s.version)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ Settings screen

    pub fn show_settings(&mut self, ui: &mut Ui, settings: &mut Settings, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::same(24)).show(ui, |ui| {
                ui.set_max_width(860.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::GEAR).size(26.0).color(theme::accent()));
                    ui.heading("Settings");
                });
                ui.add_space(12.0);

                section(ui, icon::CLOUD_ARROW_DOWN, "Updates");
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let current = update::version_string(update::current_version());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Wield {current}")).font(FontId::new(17.0, theme::heading_family())));
                        match &self.mode {
                            Mode::Installed { dir } => theme::badge(ui, "Installed", theme::aqua()).on_hover_text(dir.display().to_string()),
                            Mode::Portable { exe } => theme::badge(ui, "Portable", theme::yellow()).on_hover_text(exe.display().to_string()),
                        };
                    });
                    if self.last_check > 0 {
                        ui.label(RichText::new(format!("Last checked {}", crate::tools::startup::journal::format_time(self.last_check))).small().color(theme::text_dim()));
                    }
                    ui.add_space(6.0);
                    let status = self.status.clone();
                    match &status {
                        Status::Idle => {}
                        Status::Checking => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Checking GitHub for a new version…");
                            });
                        }
                        Status::UpToDate => {
                            ui.label(RichText::new(format!("{} You have the latest version.", icon::CHECK_CIRCLE)).color(theme::green()));
                        }
                        Status::Error(e) => {
                            ui.label(RichText::new(format!("{} {e}", icon::WARNING)).color(theme::orange()));
                        }
                        Status::Applied => {
                            ui.label("Installing the update…");
                        }
                        Status::Downloading(r) => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("Downloading and verifying Wield {}…", update::version_string(r.version)));
                            });
                        }
                        Status::Available(r) | Status::Ready(_, r) => {
                            let v = update::version_string(r.version);
                            ui.label(RichText::new(format!("{} Wield {v} is available{}", icon::SPARKLE, if r.published.is_empty() { String::new() } else { format!(" (released {})", r.published) })).strong().color(theme::accent()));
                            if !r.notes.is_empty() {
                                egui::Frame::new().fill(theme::bg()).corner_radius(egui::CornerRadius::same(6)).inner_margin(Margin::same(10)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                        ui.add(Label::new(RichText::new(&r.notes).small()).wrap());
                                    });
                                });
                            }
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                if theme::primary_button(ui, format!("{} Update now", icon::DOWNLOAD_SIMPLE)).on_hover_text("Downloads, verifies and installs, then reopens Wield.").clicked() {
                                    match &status {
                                        Status::Ready(s, _) => self.install_now(s.clone(), &ctx, toasts),
                                        _ => self.download(r.clone(), true, &ctx),
                                    }
                                }
                                if matches!(status, Status::Available(_)) && ui.button("Install when I close Wield").clicked() {
                                    self.install_on_exit = true;
                                    self.download(r.clone(), false, &ctx);
                                }
                                if !r.page.is_empty() && ui.button(format!("{} Release page", icon::ARROW_SQUARE_OUT)).clicked() {
                                    shell::open(&r.page);
                                }
                                if matches!(status, Status::Available(_)) && ui.button("Skip this version").clicked() {
                                    let v2 = v.clone();
                                    *settings = Settings::update(|s| s.skipped_version = Some(v2));
                                    self.status = Status::Idle;
                                }
                            });
                        }
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.rx.is_none(), egui::Button::new(format!("{} Check for updates", icon::ARROWS_CLOCKWISE))).clicked() {
                            self.check(&ctx);
                        }
                    });
                    ui.separator();
                    if ui.checkbox(&mut settings.auto_check, "Check for updates when Wield starts").changed() {
                        let v = settings.auto_check;
                        Settings::update(|s| s.auto_check = v);
                    }
                    if ui.checkbox(&mut settings.auto_install, "Download updates automatically and install them when I close Wield").changed() {
                        let v = settings.auto_install;
                        Settings::update(|s| s.auto_install = v);
                    }
                    ui.label(
                        RichText::new("Updates come from github.com/plaintextlab/wield and are checked against their published SHA-256 checksum before anything runs.")
                            .small()
                            .color(theme::text_dim()),
                    );
                });

                ui.add_space(14.0);
                section(ui, icon::PALETTE, "Appearance");
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let mut dark = theme::is_dark();
                        let before = dark;
                        ui.radio_value(&mut dark, true, format!("{} Gruvbox dark", icon::MOON));
                        ui.radio_value(&mut dark, false, format!("{} Gruvbox light", icon::SUN));
                        if dark != before {
                            theme::set_dark(&ctx, dark);
                            *settings = Settings::update(|s| s.dark = dark);
                        }
                    });
                });

                ui.add_space(14.0);
                section(ui, icon::MOUSE_RIGHT_CLICK, "Windows integration");
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut menu = shell::context_menu_enabled();
                    if ui.checkbox(&mut menu, "Add \"Scan with Wield\" to Explorer's right-click menu").changed() {
                        if let Err(e) = shell::set_context_menu(menu) {
                            toasts.error(&ctx, e);
                        }
                    }
                    ui.label(RichText::new("On Windows 11 it appears under \"Show more options\".").small().color(theme::text_dim()));
                });

                ui.add_space(14.0);
                section(ui, icon::FOLDER, "Data");
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label("Undo histories and backups (startup changes, privacy backup, removed apps) and settings are stored locally:");
                    if let Some(d) = settings::data_dir() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(d.display().to_string()).monospace().small());
                            if ui.button(format!("{} Open", icon::FOLDER_OPEN)).clicked() {
                                let _ = std::fs::create_dir_all(&d);
                                shell::reveal_in_explorer(&d);
                            }
                        });
                    }
                    ui.label(RichText::new("Wield sends nothing anywhere on its own, apart from the update check you can turn off above.").small().color(theme::text_dim()));
                });

                ui.add_space(14.0);
                section(ui, icon::INFO, "About");
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(format!("Wield {} · MIT License · © 2026 Bitangsha Ray", env!("CARGO_PKG_VERSION")));
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if ui.link("Source code").clicked() {
                            shell::open(&format!("https://github.com/{}", update::REPO));
                        }
                        ui.label("·");
                        if ui.link("Report a problem").clicked() {
                            shell::open(&format!("https://github.com/{}/issues", update::REPO));
                        }
                        ui.label("·");
                        if ui.link("All releases").clicked() {
                            shell::open(&format!("https://github.com/{}/releases", update::REPO));
                        }
                    });
                });
            });
        });
    }
}

fn section(ui: &mut Ui, ic: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).size(20.0).color(theme::accent()));
        ui.label(RichText::new(title).font(FontId::new(17.0, theme::heading_family())));
    });
    ui.add_space(4.0);
}

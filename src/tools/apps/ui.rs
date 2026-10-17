//! App Installer screen: pick apps with checkboxes and install them all at once.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icon;

use super::catalog::{APPS, App, Cat, PRESETS, Source};
use super::winget::{self, Installed, Outcome, Progress};
use crate::theme;
use crate::toast::Toasts;
use crate::win::{elevation, shell};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Install,
    Upgrade,
    Winget,
}

struct Running {
    job: Job,
    apps: Vec<&'static App>,
    current: Option<usize>,
    results: Vec<Option<Outcome>>,
    line: String,
    pct: Option<f32>,
    log: Vec<String>,
    stop: Arc<AtomicBool>,
    started: Instant,
    done: bool,
    log_file: Option<PathBuf>,
}

pub struct AppInstaller {
    selected: BTreeSet<&'static str>,
    installed: Option<Installed>,
    installed_rx: Option<Receiver<Installed>>,
    has_winget: bool,
    rx: Option<Receiver<Progress>>,
    running: Option<Running>,
    search: String,
    hide_installed: bool,
    confirm: bool,
}

fn cat_icon(c: Cat) -> &'static str {
    match c {
        Cat::Browsers => icon::BROWSER,
        Cat::Media => icon::MUSIC_NOTES,
        Cat::Creative => icon::PAINT_BRUSH,
        Cat::Gaming => icon::GAME_CONTROLLER,
        Cat::Runtimes => icon::PUZZLE_PIECE,
        Cat::Hardware => icon::CPU,
        Cat::Utilities => icon::TOOLBOX,
        Cat::Communication => icon::CHAT_CIRCLE,
        Cat::Developer => icon::CODE,
    }
}

impl AppInstaller {
    pub fn new() -> Self {
        Self {
            selected: BTreeSet::new(),
            installed: None,
            installed_rx: None,
            has_winget: winget::winget_path().is_some(),
            rx: None,
            running: None,
            search: String::new(),
            hide_installed: false,
            confirm: false,
        }
    }

    pub fn summary_line(&self) -> Option<String> {
        let inst = self.installed.as_ref()?;
        let n = APPS.iter().filter(|a| inst.has(a)).count();
        Some(format!("{n} of {} apps installed", APPS.len()))
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        if self.installed_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(winget::installed());
            c.request_repaint();
        });
        self.installed_rx = Some(rx);
        self.has_winget = winget::winget_path().is_some();
    }

    fn busy(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.done)
    }

    fn start(&mut self, ctx: &egui::Context, job: Job, apps: Vec<&'static App>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (c, s) = (ctx.clone(), stop.clone());
        let list = apps.clone();
        match job {
            Job::Install => std::thread::spawn(move || winget::install(list, tx, s, c)),
            Job::Upgrade => std::thread::spawn(move || winget::upgrade_all(tx, c)),
            Job::Winget => std::thread::spawn(move || winget::install_winget(tx, c)),
        };
        let n = if job == Job::Install { apps.len() } else { 1 };
        self.running = Some(Running { job, apps, current: None, results: vec![None; n], line: String::new(), pct: None, log: Vec::new(), stop, started: Instant::now(), done: false, log_file: None });
        self.rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if let Some(rx) = &self.installed_rx {
            if let Ok(i) = rx.try_recv() {
                self.installed = Some(i);
                self.installed_rx = None;
            }
        }
        let (Some(rx), Some(run)) = (&self.rx, &mut self.running) else { return };
        let mut finished = false;
        for m in rx.try_iter() {
            match m {
                Progress::Start(i) => {
                    run.current = Some(i);
                    run.pct = None;
                    run.line.clear();
                }
                Progress::Line(l) => {
                    run.line = l.clone();
                    run.log.push(l);
                    if run.log.len() > 500 {
                        run.log.drain(..100);
                    }
                }
                Progress::Percent(p) => run.pct = Some(p.clamp(0.0, 1.0)),
                Progress::Finished(i, o) => {
                    if let Some(r) = run.results.get_mut(i) {
                        *r = Some(o);
                    }
                }
                Progress::Done(log) => {
                    run.done = true;
                    run.current = None;
                    run.log_file = log;
                    finished = true;
                }
            }
        }
        if finished {
            self.rx = None;
            let failed = run.results.iter().filter(|r| matches!(r, Some(Outcome::Failed(_)))).count();
            match run.job {
                Job::Install => {
                    let ok = run.results.iter().filter(|r| matches!(r, Some(o) if o.ok())).count();
                    if failed == 0 {
                        toasts.success(ctx, format!("Done: {ok} app(s) ready."));
                    } else {
                        toasts.error(ctx, format!("{ok} app(s) ready, {failed} failed. See the list for why."));
                    }
                    // Deselect what worked; keep failures selected for a retry.
                    for (a, r) in run.apps.iter().zip(&run.results) {
                        if matches!(r, Some(o) if o.ok()) {
                            self.selected.remove(a.key);
                        }
                    }
                }
                Job::Upgrade => match &run.results[0] {
                    Some(Outcome::Failed(e)) => toasts.error(ctx, format!("Update: {e}")),
                    Some(Outcome::AlreadyInstalled) => toasts.success(ctx, "Everything is already up to date."),
                    _ => toasts.success(ctx, "Apps updated."),
                },
                Job::Winget => match &run.results[0] {
                    Some(Outcome::Failed(e)) => toasts.error(ctx, format!("Installing winget: {e}")),
                    _ => toasts.success(ctx, "Windows Package Manager is installed. You can install apps now."),
                },
            }
            self.refresh(ctx);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if self.installed.is_none() && self.installed_rx.is_none() {
            self.refresh(&ctx);
        }
        self.poll(&ctx, toasts);
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        egui::Panel::top("apps_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 8 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 20 }).show(ui, |ui| {
                    self.progress_card(ui);
                    let q = self.search.trim().to_lowercase();
                    for c in Cat::ALL {
                        let apps: Vec<&'static App> = APPS
                            .iter()
                            .filter(|a| a.cat == c)
                            .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.blurb.to_lowercase().contains(&q))
                            .filter(|a| !(self.hide_installed && self.is_installed(a)))
                            .collect();
                        if apps.is_empty() {
                            continue;
                        }
                        self.category(ui, c, &apps);
                        ui.add_space(14.0);
                    }
                });
            });
        });
        self.dialog(&ctx);
    }

    fn is_installed(&self, a: &App) -> bool {
        self.installed.as_ref().is_some_and(|i| i.has(a))
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::STOREFRONT).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("App Installer");
                ui.label(RichText::new("Tick the apps you want and install them all at once, silently, from their official sources. Free software only.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = self.selected.len();
                let label = if n == 0 { format!("{} Install", icon::DOWNLOAD_SIMPLE) } else { format!("{} Install {n} app{}", icon::DOWNLOAD_SIMPLE, if n == 1 { "" } else { "s" }) };
                let can = n > 0 && !busy && (self.has_winget || self.selected.iter().all(|k| matches!(super::catalog::find(k).map(|a| a.source), Some(Source::Website(_)))));
                if ui.add_enabled(can, egui::Button::new(RichText::new(label).color(theme::on_accent())).fill(theme::accent()).min_size(vec2(0.0, 32.0))).clicked() {
                    self.confirm = true;
                }
                if ui
                    .add_enabled(self.has_winget && !busy, egui::Button::new(format!("{} Update all apps", icon::ARROWS_CLOCKWISE)))
                    .on_hover_text("Updates every installed app winget knows about, including apps you didn't install with Wield")
                    .clicked()
                {
                    self.start(&ctx, Job::Upgrade, Vec::new());
                }
                if self.installed_rx.is_some() {
                    ui.spinner();
                }
            });
        });
        ui.add_space(6.0);
        if !self.has_winget {
            let mut clicked = false;
            banner(
                ui,
                theme::orange(),
                icon::WARNING,
                "This PC doesn't have the Windows Package Manager (winget), which installs the apps. That's normal on LTSC and older Windows 10. Wield can install it from Microsoft's official release on GitHub (about 300 MB, verified).",
                Some(("Install winget", &mut clicked)),
            );
            if clicked && !busy {
                self.start(&ctx, Job::Winget, Vec::new());
            }
        } else if !elevated {
            let mut clicked = false;
            banner(
                ui,
                theme::aqua(),
                icon::INFO,
                "Some apps install for all users and Windows will ask for permission for each one. Run Wield as administrator to install everything without prompts.",
                Some(("Restart as administrator", &mut clicked)),
            );
            if clicked {
                match elevation::relaunch_as_admin() {
                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    Err(e) => toasts.error(&ctx, e),
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("{} Search apps", icon::MAGNIFYING_GLASS)).desired_width(220.0));
            ui.label(RichText::new("Quick picks:").color(theme::text_dim()));
            for (name, desc, keys) in PRESETS {
                if ui.button(format!("{} {name}", icon::STAR)).on_hover_text(format!("{desc}: {}", keys.iter().filter_map(|k| super::catalog::find(k)).map(|a| a.name).collect::<Vec<_>>().join(", "))).clicked() {
                    for k in *keys {
                        if let Some(a) = super::catalog::find(k) {
                            if !self.is_installed(a) {
                                self.selected.insert(a.key);
                            }
                        }
                    }
                }
            }
            if !self.selected.is_empty() && ui.button(format!("{} Clear selection", icon::X)).clicked() {
                self.selected.clear();
            }
            ui.checkbox(&mut self.hide_installed, "Hide installed");
        });
    }

    fn progress_card(&mut self, ui: &mut Ui) {
        let Some(run) = &self.running else { return };
        let mut close = false;
        let border = if run.done { theme::border() } else { theme::accent() };
        theme::card_frame().stroke(Stroke::new(1.5, border)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let title = match (run.job, run.done) {
                (Job::Install, false) => format!("Installing {} of {}", run.current.map(|i| i + 1).unwrap_or(0), run.apps.len()),
                (Job::Install, true) => "Installation finished".into(),
                (Job::Upgrade, false) => "Updating all apps".into(),
                (Job::Upgrade, true) => "Update finished".into(),
                (Job::Winget, false) => "Installing the Windows Package Manager".into(),
                (Job::Winget, true) => "Windows Package Manager".into(),
            };
            ui.horizontal(|ui| {
                if !run.done {
                    ui.spinner();
                }
                ui.label(RichText::new(title).font(FontId::new(16.0, theme::heading_family())));
                let s = run.started.elapsed().as_secs();
                ui.label(RichText::new(format!("· {}:{:02}", s / 60, s % 60)).color(theme::text_dim()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if run.done {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                        if let Some(l) = &run.log_file {
                            if ui.button(format!("{} Log", icon::FILE_TEXT)).clicked() {
                                shell::open(&l.display().to_string());
                            }
                        }
                    } else if run.job == Job::Install && run.apps.len() > 1 {
                        let stopping = run.stop.load(Ordering::Relaxed);
                        if ui.add_enabled(!stopping, egui::Button::new(format!("{} {}", icon::STOP, if stopping { "Stopping after this app…" } else { "Stop after this app" }))).clicked() {
                            run.stop.store(true, Ordering::Relaxed);
                        }
                    }
                });
            });
            if !run.done {
                ui.add(Label::new(RichText::new(if run.line.is_empty() { "Starting…" } else { &run.line }).color(theme::text_dim())).truncate());
                let bar = match run.pct {
                    Some(p) => egui::ProgressBar::new(p).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true),
                };
                ui.add(bar.fill(theme::accent()));
            }
            if run.job == Job::Install {
                ui.add_space(4.0);
                for (i, a) in run.apps.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (ic, col, text) = match (&run.results[i], run.current == Some(i)) {
                            (_, true) => (icon::CIRCLE_NOTCH, theme::accent(), "Installing…".to_string()),
                            (None, _) => (icon::CIRCLE, theme::text_dim(), "Waiting".to_string()),
                            (Some(Outcome::Installed), _) => (icon::CHECK_CIRCLE, theme::green(), "Installed".to_string()),
                            (Some(Outcome::AlreadyInstalled), _) => (icon::CHECK_CIRCLE, theme::green(), "Already installed and up to date".to_string()),
                            (Some(Outcome::RestartNeeded), _) => (icon::ARROW_CLOCKWISE, theme::yellow(), "Installed; restart Windows to finish".to_string()),
                            (Some(Outcome::OpenedWebsite), _) => (icon::GLOBE, theme::aqua(), "Download page opened in your browser".to_string()),
                            (Some(Outcome::Skipped), _) => (icon::MINUS_CIRCLE, theme::text_dim(), "Skipped".to_string()),
                            (Some(Outcome::Failed(e)), _) => (icon::X_CIRCLE, theme::red(), e.clone()),
                        };
                        ui.label(RichText::new(ic).color(col));
                        ui.label(RichText::new(a.name).strong());
                        ui.add(Label::new(RichText::new(text).color(col)).truncate());
                    });
                }
            } else if run.done {
                let (col, text) = match &run.results[0] {
                    Some(Outcome::Failed(e)) => (theme::red(), e.clone()),
                    Some(Outcome::AlreadyInstalled) => (theme::green(), "Everything is up to date.".into()),
                    _ => (theme::green(), "Done.".into()),
                };
                ui.label(RichText::new(text).color(col));
            }
            if !run.log.is_empty() {
                egui::CollapsingHeader::new("Details").id_salt("apps_log").show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                        for l in &run.log {
                            ui.label(RichText::new(l).small().monospace().color(theme::text_dim()));
                        }
                    });
                });
            }
        });
        ui.add_space(12.0);
        if close {
            self.running = None;
        }
    }

    fn category(&mut self, ui: &mut Ui, c: Cat, apps: &[&'static App]) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(cat_icon(c)).size(20.0).color(theme::accent()));
            ui.label(RichText::new(c.label()).font(FontId::new(17.0, theme::heading_family())));
            let not_installed: Vec<&'static str> = apps.iter().filter(|a| !self.is_installed(a)).map(|a| a.key).collect();
            let all_sel = !not_installed.is_empty() && not_installed.iter().all(|k| self.selected.contains(k));
            if !not_installed.is_empty() && ui.small_button(if all_sel { "Unselect all" } else { "Select all" }).clicked() {
                for k in not_installed {
                    if all_sel {
                        self.selected.remove(k);
                    } else {
                        self.selected.insert(k);
                    }
                }
            }
        });
        if !c.blurb().is_empty() {
            ui.label(RichText::new(c.blurb()).color(theme::text_dim()));
        }
        ui.add_space(6.0);
        let gap = 10.0;
        let min_w = 300.0;
        let cols = (((ui.available_width() + gap) / (min_w + gap)).floor() as usize).max(1);
        let w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
        for row in apps.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for a in row {
                    self.tile(ui, a, w);
                }
            });
            ui.add_space(gap);
        }
    }

    fn tile(&mut self, ui: &mut Ui, a: &'static App, w: f32) {
        let installed = self.is_installed(a);
        let sel = self.selected.contains(a.key);
        let busy = self.busy();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 74.0), Sense::click());
        let hovered = resp.hovered() && !busy;
        let fill = if sel { theme::accent().gamma_multiply(0.14) } else if hovered { theme::card_hover() } else { theme::card() };
        let stroke = if sel { Stroke::new(1.5, theme::accent()) } else { Stroke::new(1.0, theme::border()) };
        ui.painter().rect(rect, CornerRadius::same(10), fill, stroke, StrokeKind::Inside);
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 9.0))).layout(Layout::top_down(Align::Min)));
        child.horizontal(|ui| {
            let box_icon = if sel { icon::CHECK_SQUARE } else { icon::SQUARE };
            ui.label(RichText::new(box_icon).size(18.0).color(if sel { theme::accent() } else { theme::text_dim() }));
            ui.label(RichText::new(a.name).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if installed {
                    theme::badge(ui, "Installed", theme::green());
                }
                if matches!(a.source, Source::Website(_)) {
                    theme::badge(ui, "Website", theme::aqua()).on_hover_text("Not available for silent install; opens the official download page");
                }
            });
        });
        child.add(Label::new(RichText::new(a.blurb).small().color(theme::text_dim())).wrap());
        let resp = resp.on_hover_text(match a.source {
            Source::Winget(ids) => format!("winget: {}", ids.join(", ")),
            Source::Website(u) => format!("Opens {u}"),
        });
        if resp.clicked() && !busy {
            if sel {
                self.selected.remove(a.key);
            } else {
                self.selected.insert(a.key);
            }
        }
        if !busy {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }

    fn dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm {
            return;
        }
        let apps: Vec<&'static App> = APPS.iter().filter(|a| self.selected.contains(a.key)).collect();
        let installed: Vec<&str> = apps.iter().filter(|a| self.is_installed(a)).map(|a| a.name).collect();
        let web: Vec<&str> = apps.iter().filter(|a| matches!(a.source, Source::Website(_))).map(|a| a.name).collect();
        let mut go = false;
        let mut cancel = false;
        let m = egui::Modal::new(Id::new("apps_confirm")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::DOWNLOAD_SIMPLE).size(28.0).color(theme::accent()));
                ui.label(RichText::new(format!("Install {} app{}?", apps.len(), if apps.len() == 1 { "" } else { "s" })).font(FontId::new(18.0, theme::heading_family())));
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for a in &apps {
                    ui.label(RichText::new(format!("{} {}", icon::CHECK, a.name)).color(theme::text_dim()));
                }
            });
            ui.add_space(6.0);
            ui.label("Each app is downloaded from its official source and installed silently with its default options, one after another. You can keep using your PC.");
            if !installed.is_empty() {
                ui.label(RichText::new(format!("Already installed (will be updated if a newer version exists): {}", installed.join(", "))).small().color(theme::text_dim()));
            }
            if !web.is_empty() {
                ui.label(RichText::new(format!("{} {} can't be installed silently; its official download page opens in your browser.", icon::GLOBE, web.join(", "))).small().color(theme::aqua()));
            }
            ui.label(RichText::new("By installing you accept each app's license.").small().color(theme::text_dim()));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "Install").clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if m.should_close() || cancel {
            self.confirm = false;
        }
        if go {
            self.confirm = false;
            self.start(ctx, Job::Install, apps);
        }
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
                // Leave room for the button so long text wraps instead of running under it.
                let reserve = if button.is_some() { 200.0 } else { 0.0 };
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

//! Privacy & Telemetry screen.

use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

use super::catalog::{Category, TWEAKS};
use super::engine::{self, Backup, State};
use crate::theme::{self, ACCENT, GREEN, ORANGE, RED, TEXT_DIM, YELLOW};
use crate::toast::Toasts;
use crate::tools::startup::actions::friendly;
use crate::win::sysver::OsInfo;
use crate::win::{elevation, with_com};

enum Job {
    /// Results of a bulk operation: (summary message, errors, fresh states).
    Bulk(String, Vec<String>, Vec<State>),
}

pub struct PrivacyManager {
    os: OsInfo,
    states: Vec<State>,
    scan_rx: Option<Receiver<Vec<State>>>,
    job_rx: Option<Receiver<Job>>,
    filter: Option<Category>,
    confirm_recommended: bool,
    confirm_restore: bool,
    admin_prompt: Option<String>,
    backup_count: usize,
}

fn scan(os: OsInfo, ctx: egui::Context) -> Receiver<Vec<State>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let states = with_com(move || TWEAKS.iter().map(|t| engine::state(t, &os)).collect());
        let _ = tx.send(states);
        ctx.request_repaint();
    });
    rx
}

fn category_icon(c: Category) -> &'static str {
    match c {
        Category::Telemetry => icon::CHART_LINE_UP,
        Category::Ads => icon::MEGAPHONE,
        Category::Search => icon::MAGNIFYING_GLASS,
        Category::Activity => icon::CLOCK_COUNTER_CLOCKWISE,
        Category::Ai => icon::SPARKLE,
        Category::Permissions => icon::LOCK_KEY,
    }
}

impl PrivacyManager {
    pub fn new(os: OsInfo) -> Self {
        Self {
            os,
            states: Vec::new(),
            scan_rx: None,
            job_rx: None,
            filter: None,
            confirm_recommended: false,
            confirm_restore: false,
            admin_prompt: None,
            backup_count: Backup::load().originals.len(),
        }
    }

    fn loaded(&self) -> bool {
        self.states.len() == TWEAKS.len()
    }

    /// (applied, total) over recommended tweaks that are available here.
    fn score(&self) -> (usize, usize) {
        let mut on = 0;
        let mut total = 0;
        for (t, s) in TWEAKS.iter().zip(&self.states) {
            if t.recommended && !matches!(s, State::Unavailable(_)) {
                total += 1;
                if *s == State::On {
                    on += 1;
                }
            }
        }
        (on, total)
    }

    pub fn summary_line(&self) -> Option<String> {
        self.loaded().then(|| {
            let (on, total) = self.score();
            format!("{on} of {total} recommended protections on")
        })
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        if self.scan_rx.is_none() {
            self.scan_rx = Some(scan(self.os.clone(), ctx.clone()));
        }
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if let Some(rx) = &self.scan_rx {
            if let Ok(states) = rx.try_recv() {
                self.states = states;
                self.scan_rx = None;
            }
        }
        if let Some(rx) = &self.job_rx {
            if let Ok(Job::Bulk(msg, errors, states)) = rx.try_recv() {
                self.job_rx = None;
                self.states = states;
                self.backup_count = Backup::load().originals.len();
                if errors.is_empty() {
                    toasts.success(ctx, msg);
                } else {
                    toasts.error(ctx, format!("{msg} {} change(s) failed: {}", errors.len(), friendly(errors[0].clone())));
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if !self.loaded() && self.scan_rx.is_none() {
            self.rescan(&ctx);
        }
        self.poll(&ctx, toasts);

        egui::Panel::top("privacy_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 8 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            if !self.loaded() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                    ui.label(RichText::new("Reading your privacy settings…").color(TEXT_DIM));
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 20 }).show(ui, |ui| {
                    for cat in Category::ALL {
                        if self.filter.is_some_and(|f| f != cat) {
                            continue;
                        }
                        self.section(ui, cat, elevated, toasts);
                        ui.add_space(14.0);
                    }
                });
            });
        });

        self.dialogs(&ctx, elevated, toasts);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        let busy = self.job_rx.is_some() || self.scan_rx.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::EYE_SLASH).size(26.0).color(ACCENT));
            ui.vertical(|ui| {
                ui.heading("Privacy & Telemetry");
                ui.label(RichText::new("Turn off data collection, ads and tracking. Every switch can be turned back.").color(TEXT_DIM));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("{} Apply all recommended", icon::SHIELD_CHECK)).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    self.confirm_recommended = true;
                }
                if ui
                    .add_enabled(!busy && self.backup_count > 0, egui::Button::new(format!("{} Restore original settings", icon::ARROW_COUNTER_CLOCKWISE)))
                    .on_hover_text("Put back every value exactly as it was before Wield changed it")
                    .clicked()
                {
                    self.confirm_restore = true;
                }
                if ui.add_enabled(!busy, egui::Button::new(format!("{} Rescan", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.rescan(&ctx);
                }
                if busy {
                    ui.spinner();
                }
            });
        });
        ui.add_space(8.0);

        if !elevated {
            let mut clicked = false;
            banner(ui, ORANGE, icon::LOCK, "Settings marked with a lock apply to the whole PC and need administrator rights. Your personal settings can be changed now.", Some(("Restart as administrator", &mut clicked)));
            if clicked {
                relaunch(&ctx, toasts);
            }
        }

        if self.loaded() {
            self.score_card(ui);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let all_sel = self.filter.is_none();
                if ui.add(egui::Button::new(RichText::new(format!("All  {}", TWEAKS.len())).color(if all_sel { theme::TEXT } else { TEXT_DIM })).selected(all_sel).frame_when_inactive(false)).clicked() {
                    self.filter = None;
                }
                for cat in Category::ALL {
                    let (on, total) = self.category_counts(cat);
                    let sel = self.filter == Some(cat);
                    let text = RichText::new(format!("{} {}  {on}/{total}", category_icon(cat), cat.label())).color(if sel { theme::TEXT } else { TEXT_DIM });
                    if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                        self.filter = Some(cat);
                    }
                }
            });
        }
    }

    fn category_counts(&self, cat: Category) -> (usize, usize) {
        let items: Vec<&State> = TWEAKS.iter().zip(&self.states).filter(|(t, _)| t.category == cat).map(|(_, s)| s).collect();
        (items.iter().filter(|s| ***s == State::On).count(), items.len())
    }

    fn score_card(&self, ui: &mut Ui) {
        let (on, total) = self.score();
        let frac = if total == 0 { 1.0 } else { on as f32 / total as f32 };
        let color = if frac >= 0.85 {
            GREEN
        } else if frac >= 0.5 {
            YELLOW
        } else {
            RED
        };
        theme::card_frame().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::SHIELD_CHECK).size(34.0).color(color));
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{on} / {total}")).font(FontId::new(24.0, theme::heading_family())).color(color));
                        ui.label(RichText::new("recommended protections on").color(TEXT_DIM));
                    });
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(520.0), 8.0), Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::same(4), theme::BORDER);
                    let mut fill = rect;
                    fill.set_width(rect.width() * frac);
                    ui.painter().rect_filled(fill, CornerRadius::same(4), color);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} · {}\nSome changes apply after you sign out or restart.",
                            self.os.product,
                            if self.os.is_enterprise_class() { "full telemetry control" } else { "telemetry minimum: Required" }
                        ))
                        .small()
                        .color(TEXT_DIM),
                    );
                });
            });
        });
    }

    fn section(&mut self, ui: &mut Ui, cat: Category, elevated: bool, toasts: &mut Toasts) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(category_icon(cat)).size(20.0).color(ACCENT));
            ui.label(RichText::new(cat.label()).font(FontId::new(17.0, theme::heading_family())));
        });
        ui.label(RichText::new(cat.blurb()).color(TEXT_DIM));
        ui.add_space(6.0);
        theme::card_frame().inner_margin(Margin::symmetric(4, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let indices: Vec<usize> = (0..TWEAKS.len()).filter(|&i| TWEAKS[i].category == cat).collect();
            for (n, i) in indices.iter().enumerate() {
                if n > 0 {
                    let r = ui.available_rect_before_wrap();
                    ui.painter().hline(r.x_range(), r.top(), Stroke::new(1.0, theme::BORDER));
                }
                ui.push_id(TWEAKS[*i].id, |ui| self.row(ui, *i, elevated, toasts));
            }
        });
    }

    fn row(&mut self, ui: &mut Ui, i: usize, elevated: bool, toasts: &mut Toasts) {
        let t = &TWEAKS[i];
        let state = self.states[i].clone();
        let available = !matches!(state, State::Unavailable(_));
        let busy = self.job_rx.is_some();
        let mut clicked = false;
        egui::Frame::new().inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    let r = theme::switch(ui, state == State::On, GREEN, available && !busy);
                    let tip = match &state {
                        State::On => "Protection on. Click to restore the Windows default.".to_string(),
                        State::Off => "Click to turn this protection on.".to_string(),
                        State::Partial => "Partly applied. Click to apply fully.".to_string(),
                        State::Unavailable(why) => why.clone(),
                    };
                    clicked = r.on_hover_text(tip).clicked();
                });
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut title = RichText::new(t.title).strong();
                        if !available {
                            title = title.color(TEXT_DIM);
                        }
                        ui.label(title);
                        if t.recommended {
                            theme::badge(ui, "Recommended", GREEN);
                        }
                        if t.needs_admin() {
                            theme::badge(ui, format!("{} All users", icon::LOCK), if elevated { TEXT_DIM } else { ORANGE })
                                .on_hover_text("Machine-wide setting. Needs administrator rights.");
                        }
                        match &state {
                            State::Partial => {
                                theme::badge(ui, "Partly applied", YELLOW);
                            }
                            State::Unavailable(why) => {
                                theme::badge(ui, "Not available", TEXT_DIM).on_hover_text(why);
                            }
                            _ => {}
                        }
                    });
                    ui.add(Label::new(RichText::new(t.summary).color(TEXT_DIM)).wrap());
                    if t.tradeoff != "None." {
                        ui.add(Label::new(RichText::new(format!("{} Trade-off: {}", icon::INFO, t.tradeoff)).small().color(ORANGE.gamma_multiply(0.85))).wrap());
                    }
                    if let Some(n) = t.note {
                        ui.add(Label::new(RichText::new(n).small().color(TEXT_DIM)).wrap());
                    }
                    if let State::Unavailable(why) = &state {
                        ui.label(RichText::new(why).small().color(TEXT_DIM));
                    }
                });
            });
        });
        if clicked {
            if t.needs_admin() && !elevated {
                self.admin_prompt = Some(format!("change \"{}\"", t.title));
                return;
            }
            let on = state != State::On;
            let os = self.os.clone();
            let idx = i;
            let (res, new_state) = with_com(move || {
                let t = &TWEAKS[idx];
                let r = engine::set(t, on, &os);
                (r, engine::state(t, &os))
            });
            self.states[i] = new_state;
            self.backup_count = Backup::load().originals.len();
            let ctx = ui.ctx().clone();
            match res {
                Ok(()) => toasts.success(&ctx, format!("{}: {}", if on { "Turned on" } else { "Restored Windows default" }, t.title)),
                Err(e) => toasts.error(&ctx, format!("{}: {}", t.title, friendly(e))),
            }
        }
    }

    fn recommended_targets(&self, elevated: bool) -> (Vec<usize>, usize) {
        let mut todo = Vec::new();
        let mut skipped_admin = 0;
        for (i, (t, s)) in TWEAKS.iter().zip(&self.states).enumerate() {
            if !t.recommended || matches!(s, State::On | State::Unavailable(_)) {
                continue;
            }
            if t.needs_admin() && !elevated {
                skipped_admin += 1;
            } else {
                todo.push(i);
            }
        }
        (todo, skipped_admin)
    }

    fn dialogs(&mut self, ctx: &egui::Context, elevated: bool, toasts: &mut Toasts) {
        if self.confirm_recommended {
            let (todo, skipped) = self.recommended_targets(elevated);
            let mut close = false;
            let mut go = false;
            let modal = egui::Modal::new(Id::new("privacy_apply")).show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SHIELD_CHECK).size(28.0).color(ACCENT));
                    ui.label(RichText::new("Apply recommended protections").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.add_space(8.0);
                if todo.is_empty() {
                    ui.label("All recommended protections you can change are already on.");
                } else {
                    ui.label(format!("These {} settings will be changed:", todo.len()));
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for &i in &todo {
                            ui.label(RichText::new(format!("{} {}", icon::CHECK, TWEAKS[i].title)).color(TEXT_DIM));
                        }
                    });
                }
                if skipped > 0 {
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("{} {skipped} all-users setting(s) skipped: restart as administrator to include them.", icon::LOCK)).color(ORANGE));
                }
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Original values are backed up to {}", Backup::location())).small().color(TEXT_DIM));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!todo.is_empty(), egui::Button::new(RichText::new("Apply").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if go {
                let os = self.os.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let job = with_com(move || {
                        let mut errors = Vec::new();
                        for &i in &todo {
                            if let Err(e) = engine::set(&TWEAKS[i], true, &os) {
                                errors.push(format!("{}: {e}", TWEAKS[i].title));
                            }
                        }
                        let states = TWEAKS.iter().map(|t| engine::state(t, &os)).collect();
                        Job::Bulk(format!("Applied {} recommended protection(s).", todo.len() - errors.len()), errors, states)
                    });
                    let _ = tx.send(job);
                    c.request_repaint();
                });
                self.job_rx = Some(rx);
                close = true;
            }
            if close || modal.should_close() {
                self.confirm_recommended = false;
            }
        }

        if self.confirm_restore {
            let mut close = false;
            let mut go = false;
            let modal = egui::Modal::new(Id::new("privacy_restore")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::ARROW_COUNTER_CLOCKWISE).size(28.0).color(ORANGE));
                    ui.label(RichText::new("Restore original settings?").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.add_space(8.0);
                ui.add(Label::new(format!(
                    "Puts back all {} values Wield has changed, exactly as they were before. This undoes every privacy protection you applied with Wield.",
                    self.backup_count
                )).wrap());
                if !elevated {
                    ui.label(RichText::new("All-users values need administrator rights and will be kept for a later retry.").small().color(ORANGE));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::danger_button(ui, "Restore", ORANGE, true).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if go {
                let os = self.os.clone();
                let (tx, rx) = std::sync::mpsc::channel();
                let c = ctx.clone();
                std::thread::spawn(move || {
                    let job = with_com(move || {
                        let (n, errors) = engine::restore_all();
                        let states = TWEAKS.iter().map(|t| engine::state(t, &os)).collect();
                        Job::Bulk(format!("Restored {n} original value(s)."), errors, states)
                    });
                    let _ = tx.send(job);
                    c.request_repaint();
                });
                self.job_rx = Some(rx);
                close = true;
            }
            if close || modal.should_close() {
                self.confirm_restore = false;
            }
        }

        if let Some(what) = self.admin_prompt.clone() {
            let mut close = false;
            let modal = egui::Modal::new(Id::new("privacy_admin")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::LOCK).size(28.0).color(ORANGE));
                    ui.label(RichText::new("Administrator rights needed").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.add_space(8.0);
                ui.add(Label::new(format!("This is a machine-wide setting. Wield needs to run as administrator to {what}.")).wrap());
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::primary_button(ui, format!("{} Restart as administrator", icon::SHIELD_CHECK)).clicked() {
                        relaunch(ctx, toasts);
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.admin_prompt = None;
            }
        }
    }
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

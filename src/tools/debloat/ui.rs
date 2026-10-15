//! App Debloater screen.

use std::collections::HashSet;
use std::sync::mpsc::Receiver;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui, vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::catalog::Verdict;
use super::packages::{self, AppPackage, RemovalLog, RemovedApp};
use crate::theme::{self, ACCENT, GREEN, ORANGE, RED, TEXT_DIM, YELLOW};
use crate::toast::Toasts;
use crate::tools::startup::journal;
use crate::win::fileinfo::format_bytes;
use crate::win::{elevation, shell, with_mta};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Bloat,
    Optional,
    Caution,
    Keep,
    All,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Bloat, Tab::Optional, Tab::Caution, Tab::Keep, Tab::All];
    fn label(self) -> &'static str {
        match self {
            Tab::Bloat => "Recommended to remove",
            Tab::Optional => "Optional",
            Tab::Caution => "Use caution",
            Tab::Keep => "Keep",
            Tab::All => "All apps",
        }
    }
    fn matches(self, a: &AppPackage) -> bool {
        match self {
            Tab::Bloat => a.verdict == Verdict::Bloat,
            Tab::Optional => a.verdict == Verdict::Optional,
            Tab::Caution => a.verdict == Verdict::Caution,
            Tab::Keep => a.verdict == Verdict::Keep,
            Tab::All => true,
        }
    }
}

pub fn verdict_color(v: Verdict) -> Color32 {
    match v {
        Verdict::Bloat => GREEN,
        Verdict::Optional => ACCENT,
        Verdict::Caution => ORANGE,
        Verdict::Keep => RED,
    }
}

fn verdict_icon(v: Verdict) -> &'static str {
    match v {
        Verdict::Bloat => icon::TRASH,
        Verdict::Optional => icon::CHECK_CIRCLE,
        Verdict::Caution => icon::WARNING,
        Verdict::Keep => icon::WARNING_OCTAGON,
    }
}

struct RemoveDialog {
    all_users: bool,
    deprovision: bool,
    acknowledged: bool,
    typed: String,
}

enum JobMsg {
    Step(usize, String),
    Done(Vec<(String, Result<(), String>)>),
}

pub struct DebloatManager {
    apps: Vec<AppPackage>,
    scan_rx: Option<Receiver<Result<Vec<AppPackage>, String>>>,
    scanned: bool,
    scanned_ok: bool,
    error: Option<String>,
    tab: Tab,
    search: String,
    show_system: bool,
    selected: HashSet<String>,
    detail: Option<String>,
    dialog: Option<RemoveDialog>,
    job_rx: Option<Receiver<JobMsg>>,
    job_total: usize,
    job_step: (usize, String),
    history: Option<RemovalLog>,
}

impl DebloatManager {
    pub fn new() -> Self {
        Self {
            apps: Vec::new(),
            scan_rx: None,
            scanned: false,
            scanned_ok: false,
            error: None,
            tab: Tab::Bloat,
            search: String::new(),
            show_system: false,
            selected: HashSet::new(),
            detail: None,
            dialog: None,
            job_rx: None,
            job_total: 0,
            job_step: (0, String::new()),
            history: None,
        }
    }

    pub fn summary_line(&self) -> Option<String> {
        self.scanned.then(|| {
            let n = self.apps.iter().filter(|a| a.verdict == Verdict::Bloat).count();
            format!("{n} app(s) recommended to remove")
        })
    }

    fn rescan(&mut self, ctx: &egui::Context, elevated: bool) {
        if self.scan_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(with_mta(move || packages::scan(elevated)));
            c.request_repaint();
        });
        self.scan_rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context, elevated: bool, toasts: &mut Toasts) {
        if let Some(rx) = &self.scan_rx {
            if let Ok(res) = rx.try_recv() {
                self.scan_rx = None;
                self.scanned = true;
                match res {
                    Ok(apps) => {
                        // Land on "All apps" when there's nothing to recommend (e.g. LTSC).
                        if !self.scanned_ok && !apps.iter().any(|a| a.verdict == Verdict::Bloat) {
                            self.tab = Tab::All;
                        }
                        self.scanned_ok = true;
                        self.apps = apps;
                        self.error = None;
                        let present: HashSet<String> = self.apps.iter().map(|a| a.full_name.clone()).collect();
                        self.selected.retain(|f| present.contains(f));
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        let mut finished = None;
        if let Some(rx) = &self.job_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    JobMsg::Step(i, name) => self.job_step = (i, name),
                    JobMsg::Done(results) => finished = Some(results),
                }
            }
        }
        if let Some(results) = finished {
            self.job_rx = None;
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let failed: Vec<String> = results.iter().filter_map(|(n, r)| r.as_ref().err().map(|e| format!("{n}: {e}"))).collect();
            if failed.is_empty() {
                toasts.success(ctx, format!("Removed {ok} app(s)."));
            } else {
                toasts.error(ctx, format!("Removed {ok} app(s), {} failed. {}", failed.len(), failed[0]));
            }
            self.selected.clear();
            self.rescan(ctx, elevated);
        }
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        self.apps
            .iter()
            .enumerate()
            .filter(|(_, a)| self.tab.matches(a))
            .filter(|(_, a)| self.show_system || !a.system)
            .filter(|(_, a)| q.is_empty() || a.haystack().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    fn selected_apps(&self) -> Vec<&AppPackage> {
        self.apps.iter().filter(|a| self.selected.contains(&a.full_name)).collect()
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if !self.scanned && self.scan_rx.is_none() {
            self.rescan(&ctx, elevated);
        }
        self.poll(&ctx, elevated, toasts);

        egui::Panel::top("debloat_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 8 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        if !self.scanned {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                    ui.label(RichText::new("Listing installed apps…").color(TEXT_DIM));
                });
            });
            return;
        }

        egui::Panel::right("debloat_details")
            .resizable(true)
            .default_size(320.0)
            .size_range(280.0..=560.0)
            .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.details(ui));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 20, right: 12, top: 4, bottom: 12 }).show(ui, |ui| self.table(ui));
        });

        self.dialogs(&ctx, elevated, toasts);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        let busy = self.job_rx.is_some() || self.scan_rx.is_some();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::PACKAGE).size(26.0).color(ACCENT));
            ui.vertical(|ui| {
                ui.heading("App Debloater");
                ui.label(RichText::new("Remove preinstalled and unused Store apps. Removed apps can be reinstalled.").color(TEXT_DIM));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let sel = self.selected_apps();
                let size: u64 = sel.iter().filter_map(|a| a.size).sum();
                let label = if sel.is_empty() {
                    format!("{} Remove selected", icon::TRASH)
                } else {
                    format!("{} Remove {} app(s) · {}", icon::TRASH, sel.len(), format_bytes(size))
                };
                if theme::danger_button(ui, label, RED, !busy && !sel.is_empty()).clicked() {
                    self.dialog = Some(RemoveDialog { all_users: elevated, deprovision: elevated, acknowledged: false, typed: String::new() });
                }
                let rec: Vec<String> = self.apps.iter().filter(|a| a.verdict == Verdict::Bloat && a.removable()).map(|a| a.full_name.clone()).collect();
                if ui.add_enabled(!busy && !rec.is_empty(), egui::Button::new(format!("{} Select recommended", icon::LIST_CHECKS))).clicked() {
                    self.selected.extend(rec);
                    self.tab = Tab::Bloat;
                }
                let n = self.history.as_ref().map(|h| h.removed.len()).unwrap_or_else(|| RemovalLog::load().removed.len());
                if ui.button(format!("{} History ({n})", icon::CLOCK_COUNTER_CLOCKWISE)).clicked() {
                    self.history = Some(RemovalLog::load());
                }
                if ui.add_enabled(!busy, egui::Button::new(format!("{} Rescan", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.rescan(&ctx, elevated);
                }
                if busy {
                    ui.spinner();
                }
            });
        });
        ui.add_space(8.0);

        if !elevated {
            let mut clicked = false;
            banner(
                ui,
                ORANGE,
                icon::LOCK,
                "Removing apps for all users, and stopping them from coming back, needs administrator rights.",
                Some(("Restart as administrator", &mut clicked)),
            );
            if clicked {
                relaunch(&ctx, toasts);
            }
        }
        if let Some(e) = self.error.clone() {
            banner(ui, RED, icon::WARNING, &e, None);
        }
        if !self.scanned {
            return;
        }

        self.tiles(ui);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for tab in Tab::ALL {
                let count = self.apps.iter().filter(|a| tab.matches(a) && (self.show_system || !a.system)).count();
                let sel = self.tab == tab;
                let color = if sel { theme::TEXT } else { TEXT_DIM };
                if ui.add(egui::Button::new(RichText::new(format!("{}  {count}", tab.label())).color(color)).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.search).hint_text(format!("{} Search apps…", icon::MAGNIFYING_GLASS)).desired_width(260.0));
            ui.checkbox(&mut self.show_system, "Show protected Windows components");
            if !self.selected.is_empty() && ui.button(format!("{} Clear selection", icon::X_CIRCLE)).clicked() {
                self.selected.clear();
            }
        });
    }

    fn tiles(&mut self, ui: &mut Ui) {
        let removable: Vec<&AppPackage> = self.apps.iter().filter(|a| a.removable()).collect();
        let bloat: Vec<&&AppPackage> = removable.iter().filter(|a| a.verdict == Verdict::Bloat).collect();
        let bloat_size: u64 = bloat.iter().filter_map(|a| a.size).sum();
        let sel = self.selected_apps();
        let sel_size: u64 = sel.iter().filter_map(|a| a.size).sum();
        let system = self.apps.iter().filter(|a| a.system).count();
        let tiles = [
            (icon::PACKAGE, "Removable apps".to_string(), removable.len().to_string(), ACCENT),
            (icon::TRASH, format!("Recommended to remove · {}", format_bytes(bloat_size)), bloat.len().to_string(), GREEN),
            (icon::LIST_CHECKS, format!("Selected · {}", format_bytes(sel_size)), sel.len().to_string(), YELLOW),
            (icon::LOCK, "Protected Windows components".to_string(), system.to_string(), TEXT_DIM),
        ];
        ui.columns(4, |cols| {
            for (i, (ic, label, value, color)) in tiles.into_iter().enumerate() {
                theme::card_frame().inner_margin(Margin::symmetric(14, 10)).show(&mut cols[i], |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ic).size(22.0).color(color));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(value).font(FontId::new(22.0, theme::heading_family())).color(color));
                            ui.label(RichText::new(label).small().color(TEXT_DIM));
                        });
                    });
                });
            }
        });
    }

    fn table(&mut self, ui: &mut Ui) {
        let rows = self.visible();
        if rows.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                let msg = if self.tab == Tab::Bloat { "Nothing to remove here. This PC has no known bloatware installed." } else { "No apps match." };
                ui.label(RichText::new(msg).color(TEXT_DIM));
            });
            return;
        }
        let mut toggle: Option<String> = None;
        let mut clicked: Option<String> = None;
        let mut toggle_all: Option<bool> = None;
        let apps = &self.apps;
        let selected = &self.selected;
        let detail = self.detail.clone();
        let all_selected = rows.iter().filter(|&&i| apps[i].removable()).all(|&i| selected.contains(&apps[i].full_name));

        TableBuilder::new(ui)
            .id_salt("debloat_table")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(34.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::initial(150.0).at_least(80.0).clip(true))
            .column(Column::exact(170.0))
            .column(Column::initial(80.0).at_least(60.0))
            .column(Column::initial(110.0).at_least(70.0).clip(true))
            .header(26.0, |mut h| {
                h.col(|ui| {
                    let mut v = all_selected;
                    if ui.checkbox(&mut v, "").on_hover_text("Select all removable apps in this view").changed() {
                        toggle_all = Some(v);
                    }
                });
                for t in ["APP", "PUBLISHER", "VERDICT", "SIZE", "SOURCE"] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().strong().color(TEXT_DIM));
                    });
                }
            })
            .body(|body| {
                body.rows(46.0, rows.len(), |mut row| {
                    let a = &apps[rows[row.index()]];
                    row.set_selected(detail.as_deref() == Some(a.full_name.as_str()));
                    row.col(|ui| {
                        let mut v = selected.contains(&a.full_name);
                        let r = ui.add_enabled(a.removable(), egui::Checkbox::without_text(&mut v));
                        let r = if a.removable() { r } else { r.on_disabled_hover_text("Protected by Windows; can't be removed.") };
                        if r.changed() {
                            toggle = Some(a.full_name.clone());
                        }
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if a.system {
                                    ui.label(RichText::new(icon::LOCK).small().color(TEXT_DIM));
                                }
                                ui.add(Label::new(RichText::new(&a.display_name).strong()).truncate().selectable(false));
                            });
                            ui.add(Label::new(RichText::new(&a.name).small().color(TEXT_DIM)).truncate().selectable(false));
                        });
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(&a.publisher).color(TEXT_DIM)).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        theme::badge(ui, format!("{} {}", verdict_icon(a.verdict), a.verdict.label()), verdict_color(a.verdict));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(a.size.map(format_bytes).unwrap_or_else(|| "—".into())).color(TEXT_DIM));
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(a.source).color(TEXT_DIM)).truncate().selectable(false));
                    });
                    if row.response().clicked() {
                        clicked = Some(a.full_name.clone());
                    }
                });
            });

        if let Some(all) = toggle_all {
            for &i in &rows {
                let a = &self.apps[i];
                if a.removable() {
                    if all {
                        self.selected.insert(a.full_name.clone());
                    } else {
                        self.selected.remove(&a.full_name);
                    }
                }
            }
        }
        if let Some(f) = toggle {
            if !self.selected.remove(&f) {
                self.selected.insert(f.clone());
            }
            self.detail = Some(f);
        }
        if let Some(f) = clicked {
            self.detail = Some(f);
        }
    }

    fn details(&mut self, ui: &mut Ui) {
        let Some(a) = self.detail.as_ref().and_then(|f| self.apps.iter().find(|a| &a.full_name == f)).cloned() else {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::CURSOR_CLICK).size(40.0).color(TEXT_DIM));
                ui.add_space(8.0);
                ui.label(RichText::new("Select an app to see what it is\nand what removing it affects.").color(TEXT_DIM));
                ui.add_space(20.0);
                theme::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for v in [Verdict::Bloat, Verdict::Optional, Verdict::Caution, Verdict::Keep] {
                        theme::badge(ui, format!("{} {}", verdict_icon(v), v.label()), verdict_color(v));
                        let t = match v {
                            Verdict::Bloat => "Preinstalled promotion or rarely used. Safe to remove.",
                            Verdict::Optional => "Fine to remove if you don't use it.",
                            Verdict::Caution => "Breaks a feature, codec, game sign-in or hardware panel.",
                            Verdict::Keep => "Windows or app installs depend on it. Type REMOVE to confirm.",
                        };
                        ui.label(RichText::new(t).small().color(TEXT_DIM));
                        ui.add_space(4.0);
                    }
                });
            });
            return;
        };
        let color = verdict_color(a.verdict);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new(&a.display_name).font(FontId::new(19.0, theme::heading_family())));
            ui.horizontal_wrapped(|ui| {
                theme::badge(ui, format!("{} {}", verdict_icon(a.verdict), a.verdict.label()), color);
                if a.system {
                    theme::badge(ui, format!("{} Protected", icon::LOCK), TEXT_DIM);
                }
                if a.provisioned {
                    theme::badge(ui, "Installs for new users", YELLOW).on_hover_text("Provisioned: Windows installs it for every new account.");
                }
            });
            ui.add_space(10.0);
            callout(ui, color, verdict_icon(a.verdict), a.verdict.label(), &a.reason);
            if !a.description.is_empty() {
                ui.add_space(8.0);
                ui.add(Label::new(RichText::new(&a.description).color(TEXT_DIM)).wrap());
            }
            ui.add_space(8.0);
            egui::Grid::new("app_grid").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
                let mut row = |k: &str, v: String, mono: bool| {
                    ui.label(RichText::new(k).small().color(TEXT_DIM));
                    let t = if mono { RichText::new(v).monospace() } else { RichText::new(v) };
                    ui.add(Label::new(t).wrap());
                    ui.end_row();
                };
                row("Publisher", a.publisher.clone(), false);
                row("Version", a.version.clone(), false);
                row("Source", a.source.to_string(), false);
                row("Size", a.size.map(format_bytes).unwrap_or_else(|| "Unknown".into()), false);
                row("Package", a.name.clone(), true);
                row("Family name", a.family_name.clone(), true);
                if let Some(p) = &a.installed_path {
                    row("Location", p.display().to_string(), true);
                }
            });
            ui.add_space(10.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(format!("{} Store page", icon::GLOBE)).clicked() {
                    shell::open(&format!("ms-windows-store://pdp/?PFN={}", a.family_name));
                }
                if let Some(p) = a.installed_path.as_ref().filter(|p| p.exists()) {
                    if ui.button(format!("{} Open folder", icon::FOLDER_OPEN)).clicked() {
                        shell::reveal_in_explorer(p);
                    }
                }
                if ui.button(format!("{} Search online", icon::MAGNIFYING_GLASS)).clicked() {
                    shell::search_web(&format!("{} {} safe to remove", a.display_name, a.name));
                }
            });
        });
    }

    fn start_removal(&mut self, ctx: &egui::Context, all_users: bool, deprovision: bool) {
        let targets: Vec<AppPackage> = self.selected_apps().into_iter().filter(|a| a.removable()).cloned().collect();
        self.job_total = targets.len();
        self.job_step = (0, String::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let mut results = Vec::new();
            for (i, a) in targets.iter().enumerate() {
                let _ = tx.send(JobMsg::Step(i, a.display_name.clone()));
                c.request_repaint();
                let (full, fam) = (a.full_name.clone(), a.family_name.clone());
                let res = with_mta(move || packages::remove(&full, &fam, all_users, deprovision));
                if res.is_ok() {
                    let mut log = RemovalLog::load();
                    log.removed.push(RemovedApp {
                        when: journal::now(),
                        display_name: a.display_name.clone(),
                        name: a.name.clone(),
                        family_name: a.family_name.clone(),
                        full_name: a.full_name.clone(),
                        installed_path: a.installed_path.clone(),
                        all_users,
                        deprovisioned: deprovision,
                        reinstalled: false,
                    });
                    let _ = log.save();
                }
                results.push((a.display_name.clone(), res));
            }
            let _ = tx.send(JobMsg::Done(results));
            c.request_repaint();
        });
        self.job_rx = Some(rx);
    }

    fn dialogs(&mut self, ctx: &egui::Context, elevated: bool, toasts: &mut Toasts) {
        if self.dialog.is_some() {
            let sel: Vec<AppPackage> = self.selected_apps().into_iter().cloned().collect();
            let worst = sel.iter().map(|a| a.verdict).max().unwrap_or(Verdict::Bloat);
            let color = verdict_color(worst);
            let mut go = false;
            let mut close = false;
            let d = self.dialog.as_mut().unwrap();
            let modal = egui::Modal::new(Id::new("debloat_confirm"))
                .frame(egui::Frame::new().fill(theme::CARD).stroke(Stroke::new(2.0, color)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20)))
                .show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::TRASH).size(28.0).color(color));
                        ui.label(RichText::new(format!("Remove {} app(s)?", sel.len())).font(FontId::new(18.0, theme::heading_family())));
                    });
                    ui.add_space(8.0);
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for a in &sel {
                            ui.horizontal(|ui| {
                                theme::badge(ui, a.verdict.label(), verdict_color(a.verdict));
                                ui.label(&a.display_name);
                            });
                        }
                    });
                    ui.add_space(8.0);
                    match worst {
                        Verdict::Keep => callout(ui, RED, icon::WARNING_OCTAGON, "Includes apps you should keep", "Removing these can break Windows features or stop you installing apps, and some can't be reinstalled easily."),
                        Verdict::Caution => callout(ui, ORANGE, icon::WARNING, "Includes apps that affect features", "You may lose codecs, game sign-in, hardware settings or a default app. Check the list above."),
                        _ => {}
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Remove for").strong());
                    ui.radio_value(&mut d.all_users, false, "Only my account");
                    ui.add_enabled_ui(elevated, |ui| {
                        ui.radio_value(&mut d.all_users, true, "All user accounts on this PC");
                        ui.checkbox(&mut d.deprovision, "Don't reinstall for new accounts or after feature updates");
                    });
                    if !elevated {
                        ui.label(RichText::new(format!("{} All-users options need administrator rights.", icon::LOCK)).small().color(ORANGE));
                    }
                    ui.add_space(8.0);
                    let ready = match worst {
                        Verdict::Keep => {
                            ui.label("Type REMOVE to confirm:");
                            ui.add(TextEdit::singleline(&mut d.typed).hint_text("REMOVE").desired_width(160.0));
                            d.typed.trim() == "REMOVE"
                        }
                        Verdict::Caution => {
                            ui.checkbox(&mut d.acknowledged, "I understand what these apps do");
                            d.acknowledged
                        }
                        _ => true,
                    };
                    ui.label(RichText::new(format!("{} Reinstall any of them later from History or the Microsoft Store.", icon::ARROW_COUNTER_CLOCKWISE)).small().color(TEXT_DIM));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if theme::danger_button(ui, format!("{} Remove", icon::TRASH), RED, ready).clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if go {
                let (all, dep) = (d.all_users && elevated, d.deprovision && elevated);
                self.dialog = None;
                self.start_removal(ctx, all, dep);
            } else if close || modal.should_close() {
                self.dialog = None;
            }
        }

        if self.job_rx.is_some() {
            egui::Modal::new(Id::new("debloat_progress")).show(ctx, |ui| {
                ui.set_width(420.0);
                let (i, name) = &self.job_step;
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("Removing {} of {}: {name}", i + 1, self.job_total)).strong());
                });
                ui.add_space(8.0);
                let frac = if self.job_total == 0 { 0.0 } else { *i as f32 / self.job_total as f32 };
                ui.add(egui::ProgressBar::new(frac).fill(ACCENT));
            });
        }

        if self.history.is_some() {
            let mut close = false;
            let mut reinstall: Option<usize> = None;
            let log = self.history.as_ref().unwrap();
            let modal = egui::Modal::new(Id::new("debloat_history")).show(ctx, |ui| {
                ui.set_width(640.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::CLOCK_COUNTER_CLOCKWISE).size(24.0).color(ACCENT));
                    ui.label(RichText::new("Removed apps").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.label(RichText::new(format!("Saved in {}", RemovalLog::location())).small().color(TEXT_DIM));
                ui.add_space(8.0);
                if log.removed.is_empty() {
                    ui.label(RichText::new("No apps removed yet.").color(TEXT_DIM));
                }
                egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                    egui::Grid::new("debloat_hist").num_columns(4).striped(true).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                        for (idx, e) in log.removed.iter().enumerate().rev() {
                            ui.label(RichText::new(journal::format_time(e.when)).small().color(TEXT_DIM));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&e.display_name).strong());
                                ui.label(RichText::new(&e.name).small().color(TEXT_DIM));
                            });
                            ui.label(RichText::new(if e.all_users { "All users" } else { "My account" }).color(TEXT_DIM));
                            if e.reinstalled {
                                ui.label(RichText::new("Reinstalled").color(TEXT_DIM));
                            } else if ui.button(format!("{} Reinstall", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                                reinstall = Some(idx);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.add_space(10.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            if let Some(idx) = reinstall {
                let entry = log.removed[idx].clone();
                match with_mta(move || packages::reinstall(&entry)) {
                    Ok(msg) => {
                        let mut l = RemovalLog::load();
                        if let Some(e) = l.removed.get_mut(idx) {
                            e.reinstalled = msg.starts_with("Reinstalled");
                        }
                        let _ = l.save();
                        toasts.success(ctx, msg);
                        self.rescan(ctx, elevated);
                    }
                    Err(e) => toasts.error(ctx, format!("Reinstall failed: {e}")),
                }
                self.history = Some(RemovalLog::load());
            }
            if close || modal.should_close() {
                self.history = None;
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

fn callout(ui: &mut Ui, color: Color32, ic: &str, title: &str, body: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic).color(color));
                ui.label(RichText::new(title).strong().color(color));
            });
            ui.add(Label::new(body).wrap());
        });
}

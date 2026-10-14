//! Startup Manager screen.

use std::sync::mpsc::Receiver;

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui, vec2,
};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::actions::{self, ServiceMode};
use super::journal::{self, Journal};
use super::model::{Criticality, Impact, SourceKind, StartupItem};
use super::scan::{self, ScanMsg};
use crate::theme::{self, ACCENT, GREEN, ORANGE, RED, TEXT_DIM};
use crate::toast::Toasts;
use crate::win::{elevation, fileinfo::format_bytes, shell};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    All,
    Apps,
    Services,
    Tasks,
    System,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::All, Tab::Apps, Tab::Services, Tab::Tasks, Tab::System];
    fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Apps => "Apps",
            Tab::Services => "Services",
            Tab::Tasks => "Scheduled tasks",
            Tab::System => "Sign-in (Winlogon)",
        }
    }
    fn matches(self, s: SourceKind) -> bool {
        match self {
            Tab::All => true,
            Tab::Apps => matches!(s, SourceKind::RegistryRun | SourceKind::StartupFolder),
            Tab::Services => s == SourceKind::Service,
            Tab::Tasks => s == SourceKind::ScheduledTask,
            Tab::System => s == SourceKind::Winlogon,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    Any,
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Name,
    Publisher,
    Source,
    Impact,
    Criticality,
    Status,
}

/// A disable request waiting for the user to confirm in a dialog.
struct Pending {
    id: String,
    acknowledged: bool,
    typed: String,
    mode: ServiceMode,
}

pub struct StartupManager {
    items: Vec<StartupItem>,
    scan_rx: Option<Receiver<ScanMsg>>,
    progress: String,
    warnings: Vec<String>,
    scanned: bool,
    tab: Tab,
    search: String,
    impact_filter: Option<Impact>,
    crit_filter: Option<Criticality>,
    status_filter: StatusFilter,
    hide_microsoft: bool,
    sort: Option<(SortCol, bool)>,
    selected: Option<String>,
    pending: Option<Pending>,
    admin_prompt: Option<String>,
    history: Option<Journal>,
    restore_rx: Option<Receiver<Result<(), String>>>,
}

fn source_icon(s: SourceKind) -> &'static str {
    match s {
        SourceKind::RegistryRun => icon::APP_WINDOW,
        SourceKind::StartupFolder => icon::FOLDER_OPEN,
        SourceKind::Service => icon::GEAR,
        SourceKind::ScheduledTask => icon::CALENDAR_CHECK,
        SourceKind::Winlogon => icon::KEY,
    }
}

impl StartupManager {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            scan_rx: None,
            progress: String::new(),
            warnings: Vec::new(),
            scanned: false,
            tab: Tab::All,
            search: String::new(),
            impact_filter: None,
            crit_filter: None,
            status_filter: StatusFilter::Any,
            hide_microsoft: false,
            sort: None,
            selected: None,
            pending: None,
            admin_prompt: None,
            history: None,
            restore_rx: None,
        }
    }

    pub fn summary_line(&self) -> Option<String> {
        if !self.scanned {
            return None;
        }
        let s = scan::summarize(&self.items);
        Some(format!("{} enabled · {} high impact", s.enabled, s.high_enabled))
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        if self.scan_rx.is_none() {
            self.progress = "Starting scan…".into();
            self.scan_rx = Some(scan::spawn(ctx.clone()));
        }
    }

    fn poll(&mut self, toasts: &mut Toasts, ctx: &egui::Context) {
        if let Some(rx) = &self.scan_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    ScanMsg::Progress(p) => self.progress = p,
                    ScanMsg::Done(items, warnings) => {
                        self.items = items;
                        self.warnings = warnings;
                        self.scanned = true;
                        self.scan_rx = None;
                        if let Some(sel) = &self.selected {
                            if !self.items.iter().any(|i| &i.id == sel) {
                                self.selected = None;
                            }
                        }
                        break;
                    }
                }
            }
        }
        if let Some(rx) = &self.restore_rx {
            if let Ok(res) = rx.try_recv() {
                self.restore_rx = None;
                match res {
                    Ok(()) => toasts.success(ctx, "Restore point created (or one from the last 24 h already exists)."),
                    Err(e) => toasts.error(ctx, format!("Restore point failed: {e}")),
                }
            }
        }
    }

    fn visible(&self) -> Vec<usize> {
        let q = self.search.trim().to_lowercase();
        let mut rows: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| self.tab.matches(it.source))
            .filter(|(_, it)| q.is_empty() || it.haystack().contains(&q))
            .filter(|(_, it)| self.impact_filter.is_none_or(|f| it.impact == f))
            .filter(|(_, it)| self.crit_filter.is_none_or(|f| it.criticality == f))
            .filter(|(_, it)| match self.status_filter {
                StatusFilter::Any => true,
                StatusFilter::Enabled => it.enabled,
                StatusFilter::Disabled => !it.enabled,
            })
            .filter(|(_, it)| !self.hide_microsoft || !it.is_microsoft)
            .map(|(i, _)| i)
            .collect();
        if let Some((col, asc)) = self.sort {
            rows.sort_by(|&a, &b| {
                let (a, b) = (&self.items[a], &self.items[b]);
                let ord = match col {
                    SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    SortCol::Publisher => a.publisher.cmp(&b.publisher),
                    SortCol::Source => a.source.cmp(&b.source),
                    SortCol::Impact => a.impact.cmp(&b.impact),
                    SortCol::Criticality => a.criticality.cmp(&b.criticality),
                    SortCol::Status => a.enabled.cmp(&b.enabled).then(a.status_text.cmp(&b.status_text)),
                };
                if asc { ord } else { ord.reverse() }
            });
        }
        rows
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if !self.scanned && self.scan_rx.is_none() {
            self.rescan(&ctx);
        }
        self.poll(toasts, &ctx);

        egui::Panel::top("startup_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 8 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        if self.scan_rx.is_some() && !self.scanned {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.35);
                    ui.add(egui::Spinner::new().size(36.0).color(ACCENT));
                    ui.add_space(10.0);
                    ui.label(RichText::new(&self.progress).color(TEXT_DIM));
                });
            });
            return;
        }

        egui::Panel::right("startup_details")
            .resizable(true)
            .default_size(320.0)
            .size_range(280.0..=620.0)
            .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::same(16)))
            .show(ui, |ui| self.details(ui, elevated, toasts));

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 20, right: 12, top: 4, bottom: 12 }).show(ui, |ui| {
                self.table(ui, elevated, toasts);
            });
        });

        self.dialogs(&ctx, elevated, toasts);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::ROCKET_LAUNCH).size(26.0).color(ACCENT));
            ui.vertical(|ui| {
                ui.heading("Startup Manager");
                ui.label(
                    RichText::new("Apps, services and scheduled tasks that start with Windows. Every change is reversible.")
                        .color(TEXT_DIM),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button(format!("{} Windows tools", icon::WRENCH), |ui| {
                    if ui.button("Services (services.msc)").clicked() {
                        shell::open("services.msc");
                    }
                    if ui.button("Task Scheduler (taskschd.msc)").clicked() {
                        shell::open("taskschd.msc");
                    }
                    if ui.button("Task Manager").clicked() {
                        shell::open("taskmgr.exe");
                    }
                    if ui.button("My Startup folder").clicked() {
                        if let Ok(a) = std::env::var("APPDATA") {
                            shell::reveal_in_explorer(&std::path::Path::new(&a).join(r"Microsoft\Windows\Start Menu\Programs\Startup"));
                        }
                    }
                });
                let hist = self.history_len();
                if ui.button(format!("{} History ({hist})", icon::CLOCK_COUNTER_CLOCKWISE)).on_hover_text("Undo previous changes").clicked() {
                    self.history = Some(Journal::load());
                }
                let busy = self.restore_rx.is_some();
                let label = if busy { format!("{} Creating…", icon::SHIELD_CHECK) } else { format!("{} Restore point", icon::SHIELD_CHECK) };
                if ui
                    .add_enabled(!busy, egui::Button::new(label))
                    .on_hover_text("Create a System Restore point before making risky changes")
                    .clicked()
                {
                    if elevated {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let c = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(actions::create_restore_point());
                            c.request_repaint();
                        });
                        self.restore_rx = Some(rx);
                        toasts.info(&ctx, "Creating a restore point. This can take a minute…");
                    } else {
                        self.admin_prompt = Some("create a restore point".into());
                    }
                }
                let scanning = self.scan_rx.is_some();
                if ui.add_enabled(!scanning, egui::Button::new(format!("{} Rescan", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.rescan(&ctx);
                }
                if scanning {
                    ui.spinner();
                }
            });
        });
        ui.add_space(8.0);

        if !elevated
            && self.banner(
                ui,
                ORANGE,
                icon::LOCK,
                "Running without administrator rights. Services, all-users entries and Windows tasks are read-only.",
                Some("Restart as administrator"),
            )
        {
            relaunch(&ctx, toasts);
        }
        let warnings = self.warnings.clone();
        for w in warnings {
            self.banner(ui, ORANGE, icon::WARNING, &w, None);
        }

        if self.scanned {
            self.summary_tiles(ui);
            ui.add_space(8.0);
            self.filters(ui);
        }
    }

    fn history_len(&self) -> usize {
        self.history.as_ref().map(|h| h.entries.len()).unwrap_or_else(|| Journal::load().entries.len())
    }

    /// Colored notice strip. Returns true when its button was clicked.
    fn banner(&self, ui: &mut Ui, color: Color32, ic: &str, text: &str, button: Option<&str>) -> bool {
        let mut clicked = false;
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
                    if let Some(b) = button {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            clicked = ui.button(b).clicked();
                        });
                    }
                });
            });
        ui.add_space(6.0);
        clicked
    }

    fn summary_tiles(&mut self, ui: &mut Ui) {
        let s = scan::summarize(&self.items);
        ui.columns(4, |cols| {
            let tiles = [
                (icon::LIST_CHECKS, "Startup entries", s.total, ACCENT, "Everything Wield found"),
                (icon::LIGHTNING, "Enabled", s.enabled, theme::TEXT, "Show enabled entries"),
                (icon::GAUGE, "High impact, enabled", s.high_enabled, RED, "Show enabled high-impact entries"),
                (icon::CHECK_CIRCLE, "Safe to disable, enabled", s.safe_enabled, GREEN, "Show enabled entries that are safe to turn off"),
            ];
            for (i, (ic, label, value, color, hint)) in tiles.into_iter().enumerate() {
                let ui = &mut cols[i];
                let resp = theme::card_frame()
                    .inner_margin(Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(ic).size(22.0).color(color));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(value.to_string()).font(FontId::new(22.0, theme::heading_family())).color(color));
                                ui.label(RichText::new(label).small().color(TEXT_DIM));
                            });
                        });
                    })
                    .response
                    .interact(Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(hint);
                if resp.clicked() {
                    self.tab = Tab::All;
                    self.impact_filter = None;
                    self.crit_filter = None;
                    self.status_filter = StatusFilter::Any;
                    match i {
                        1 => self.status_filter = StatusFilter::Enabled,
                        2 => {
                            self.status_filter = StatusFilter::Enabled;
                            self.impact_filter = Some(Impact::High);
                        }
                        3 => {
                            self.status_filter = StatusFilter::Enabled;
                            self.crit_filter = Some(Criticality::Optional);
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    fn filters(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                let count = self.items.iter().filter(|i| tab.matches(i.source)).count();
                let selected = self.tab == tab;
                let text = RichText::new(format!("{}  {count}", tab.label())).color(if selected { theme::TEXT } else { TEXT_DIM });
                if ui.add(egui::Button::new(text).selected(selected).frame_when_inactive(false)).clicked() {
                    self.tab = tab;
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.search)
                    .hint_text(format!("{} Search name, command, publisher…", icon::MAGNIFYING_GLASS))
                    .desired_width(280.0),
            );
            egui::ComboBox::from_id_salt("impact_f")
                .selected_text(self.impact_filter.map_or("Any impact".into(), |i| format!("{} impact", i.label())))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.impact_filter, None, "Any impact");
                    for i in [Impact::High, Impact::Medium, Impact::Low] {
                        ui.selectable_value(&mut self.impact_filter, Some(i), format!("{} impact", i.label()));
                    }
                });
            egui::ComboBox::from_id_salt("crit_f")
                .selected_text(self.crit_filter.map_or("Any criticality", |c| c.label()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.crit_filter, None, "Any criticality");
                    for c in [Criticality::Optional, Criticality::Important, Criticality::Critical] {
                        ui.selectable_value(&mut self.crit_filter, Some(c), RichText::new(c.label()).color(theme::crit_color(c)));
                    }
                });
            egui::ComboBox::from_id_salt("status_f")
                .selected_text(match self.status_filter {
                    StatusFilter::Any => "Any status",
                    StatusFilter::Enabled => "Enabled",
                    StatusFilter::Disabled => "Disabled",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.status_filter, StatusFilter::Any, "Any status");
                    ui.selectable_value(&mut self.status_filter, StatusFilter::Enabled, "Enabled");
                    ui.selectable_value(&mut self.status_filter, StatusFilter::Disabled, "Disabled");
                });
            ui.checkbox(&mut self.hide_microsoft, "Hide Microsoft entries");
            let filtered = self.impact_filter.is_some()
                || self.crit_filter.is_some()
                || self.status_filter != StatusFilter::Any
                || !self.search.is_empty()
                || self.hide_microsoft;
            if filtered && ui.button(format!("{} Clear", icon::X_CIRCLE)).clicked() {
                self.impact_filter = None;
                self.crit_filter = None;
                self.status_filter = StatusFilter::Any;
                self.search.clear();
                self.hide_microsoft = false;
            }
        });
    }

    fn table(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let rows = self.visible();
        if rows.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(RichText::new("No entries match the current filters.").color(TEXT_DIM)));
            return;
        }
        let mut clicked: Option<String> = None;
        let mut toggle: Option<String> = None;
        let mut sort_click: Option<SortCol> = None;
        let sort = self.sort;
        let selected = self.selected.clone();
        let items = &self.items;

        let header_cell = |ui: &mut Ui, text: &str, col: Option<SortCol>, sort_click: &mut Option<SortCol>| {
            let arrow = match (col, sort) {
                (Some(c), Some((s, asc))) if c == s => if asc { " ▲" } else { " ▼" },
                _ => "",
            };
            let resp = ui.add(Label::new(RichText::new(format!("{text}{arrow}")).small().strong().color(TEXT_DIM)).selectable(false).sense(Sense::click()));
            if let Some(c) = col {
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    *sort_click = Some(c);
                }
            }
        };

        TableBuilder::new(ui)
            .id_salt("startup_table")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(48.0))
            .column(Column::remainder().at_least(150.0).clip(true))
            .column(Column::initial(120.0).at_least(70.0).clip(true))
            .column(Column::initial(118.0).at_least(80.0).clip(true))
            .column(Column::exact(76.0))
            .column(Column::exact(132.0))
            .column(Column::initial(110.0).at_least(80.0).clip(true))
            .header(26.0, |mut h| {
                h.col(|ui| header_cell(ui, "ON", None, &mut sort_click));
                h.col(|ui| header_cell(ui, "NAME", Some(SortCol::Name), &mut sort_click));
                h.col(|ui| header_cell(ui, "PUBLISHER", Some(SortCol::Publisher), &mut sort_click));
                h.col(|ui| header_cell(ui, "TYPE", Some(SortCol::Source), &mut sort_click));
                h.col(|ui| header_cell(ui, "IMPACT", Some(SortCol::Impact), &mut sort_click));
                h.col(|ui| header_cell(ui, "CRITICALITY", Some(SortCol::Criticality), &mut sort_click));
                h.col(|ui| header_cell(ui, "STATUS", Some(SortCol::Status), &mut sort_click));
            })
            .body(|body| {
                body.rows(50.0, rows.len(), |mut row| {
                    let item = &items[rows[row.index()]];
                    row.set_selected(selected.as_deref() == Some(item.id.as_str()));
                    let crit = theme::crit_color(item.criticality);

                    row.col(|ui| {
                        let r = theme::switch(ui, item.enabled, crit, item.can_toggle);
                        let r = if item.can_toggle {
                            let mut tip = format!("{} — {}", if item.enabled { "Click to disable" } else { "Click to enable" }, item.criticality.label());
                            if item.needs_admin && !elevated {
                                tip.push_str(" (needs administrator)");
                            }
                            r.on_hover_text(tip)
                        } else {
                            r.on_hover_text(item.readonly_reason.as_deref().unwrap_or("Read-only"))
                        };
                        if r.clicked() {
                            toggle = Some(item.id.clone());
                        }
                    });
                    row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            let mut name = RichText::new(&item.name).strong();
                            if !item.enabled {
                                name = name.color(TEXT_DIM);
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if item.needs_admin && !elevated && item.can_toggle {
                                    ui.label(RichText::new(icon::LOCK).small().color(TEXT_DIM));
                                }
                                ui.add(Label::new(name).truncate().selectable(false));
                            });
                            let sub = item.description.clone().unwrap_or_else(|| item.command.clone());
                            ui.add(Label::new(RichText::new(sub).small().color(TEXT_DIM)).truncate().selectable(false));
                        });
                    });
                    row.col(|ui| {
                        ui.add(Label::new(RichText::new(item.publisher.as_deref().unwrap_or("—")).color(TEXT_DIM)).truncate().selectable(false));
                    });
                    row.col(|ui| {
                        ui.add(
                            Label::new(RichText::new(format!("{} {}", source_icon(item.source), item.source.label())).color(TEXT_DIM))
                                .truncate()
                                .selectable(false),
                        );
                    });
                    row.col(|ui| {
                        theme::badge(ui, item.impact.label(), theme::impact_color(item.impact));
                    });
                    row.col(|ui| {
                        theme::badge(ui, format!("{} {}", theme::crit_icon(item.criticality), item.criticality.label()), crit);
                    });
                    row.col(|ui| {
                        let color = if item.status_text == "MODIFIED" {
                            RED
                        } else if item.enabled {
                            theme::TEXT
                        } else {
                            TEXT_DIM
                        };
                        ui.add(Label::new(RichText::new(&item.status_text).color(color)).truncate().selectable(false));
                    });
                    if row.response().clicked() {
                        clicked = Some(item.id.clone());
                    }
                });
            });

        if let Some(c) = sort_click {
            self.sort = match self.sort {
                Some((s, asc)) if s == c => if asc { Some((c, false)) } else { None },
                // Most useful first: high impact / critical / enabled on top.
                _ => Some((c, matches!(c, SortCol::Name | SortCol::Publisher | SortCol::Source))),
            };
        }
        if let Some(id) = clicked {
            self.selected = Some(id);
        }
        if let Some(id) = toggle {
            self.selected = Some(id.clone());
            self.request_toggle(&id, ui.ctx(), elevated, toasts);
        }
    }

    fn request_toggle(&mut self, id: &str, ctx: &egui::Context, elevated: bool, toasts: &mut Toasts) {
        let Some(item) = self.items.iter().find(|i| i.id == id) else { return };
        if !item.can_toggle {
            toasts.info(ctx, item.readonly_reason.clone().unwrap_or_else(|| "This entry is read-only.".into()));
            return;
        }
        if item.needs_admin && !elevated {
            self.admin_prompt = Some(format!("change \"{}\"", item.name));
            return;
        }
        let enable = !item.enabled;
        // Turning something back on, or switching off a safe app, needs no confirmation.
        if enable || (item.criticality == Criticality::Optional && item.source != SourceKind::Service) {
            self.apply(id, enable, ServiceMode::Manual, ctx, toasts);
        } else {
            self.pending = Some(Pending { id: id.to_string(), acknowledged: false, typed: String::new(), mode: ServiceMode::Manual });
        }
    }

    fn apply(&mut self, id: &str, enable: bool, mode: ServiceMode, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(item) = self.items.iter_mut().find(|i| i.id == id) else { return };
        match actions::set_enabled(item, enable, mode) {
            Ok(a) => {
                item.enabled = a.enabled;
                item.status_text = a.status_text;
                if let (Some((start, delayed)), Some(svc)) = (a.service_start, item.service.as_mut()) {
                    svc.start_type = start;
                    svc.delayed = delayed;
                }
                let when = if item.source == SourceKind::Service { " Takes effect at next restart." } else { "" };
                toasts.success(ctx, format!("{} \"{}\".{when}", if enable { "Enabled" } else { "Disabled" }, item.name));
                if self.history.is_some() {
                    self.history = Some(Journal::load());
                }
            }
            Err(e) => toasts.error(ctx, format!("\"{}\": {e}", item.name)),
        }
    }

    fn details(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let Some(item) = self.selected.as_ref().and_then(|id| self.items.iter().find(|i| &i.id == id)).cloned() else {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::CURSOR_CLICK).size(40.0).color(TEXT_DIM));
                ui.add_space(8.0);
                ui.label(RichText::new("Select an entry to see what it is,\nwhy it's rated that way, and what\ndisabling it would affect.").color(TEXT_DIM));
                ui.add_space(24.0);
                legend(ui);
            });
            return;
        };
        let ctx = ui.ctx().clone();
        let crit = theme::crit_color(item.criticality);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.label(RichText::new(&item.name).font(FontId::new(19.0, theme::heading_family())));
            ui.horizontal_wrapped(|ui| {
                theme::badge(ui, format!("{} {}", theme::crit_icon(item.criticality), item.criticality.label()), crit);
                theme::badge(ui, format!("{} {} impact", icon::GAUGE, item.impact.label()), theme::impact_color(item.impact));
                theme::badge(ui, format!("{} {}", source_icon(item.source), item.source.label()), ACCENT);
            });
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                let r = theme::switch(ui, item.enabled, crit, item.can_toggle);
                if r.clicked() {
                    self.request_toggle(&item.id, &ctx, elevated, toasts);
                }
                ui.label(RichText::new(&item.status_text).strong());
                if item.needs_admin && !elevated && item.can_toggle {
                    ui.label(RichText::new(format!("{} needs administrator", icon::LOCK)).small().color(ORANGE));
                }
            });
            if let Some(r) = &item.readonly_reason {
                ui.label(RichText::new(format!("{} {r}", icon::INFO)).small().color(TEXT_DIM));
            }
            ui.add_space(10.0);

            callout(ui, crit, theme::crit_icon(item.criticality), &format!("Criticality: {}", item.criticality.label()), &item.criticality_reason);
            ui.add_space(6.0);
            callout(ui, theme::impact_color(item.impact), icon::GAUGE, &format!("Startup impact: {}", item.impact.label()), &item.impact_reason);
            ui.add_space(10.0);

            if let Some(d) = &item.description {
                ui.add(Label::new(RichText::new(d).color(TEXT_DIM)).wrap());
                ui.add_space(8.0);
            }

            egui::Grid::new("details_grid").num_columns(2).spacing(vec2(10.0, 6.0)).striped(false).show(ui, |ui| {
                let row = |ui: &mut Ui, k: &str, v: RichText| {
                    ui.label(RichText::new(k).small().color(TEXT_DIM));
                    ui.add(Label::new(v).wrap());
                    ui.end_row();
                };
                row(ui, "Publisher", RichText::new(item.publisher.as_deref().unwrap_or("Unknown")));
                if let Some(t) = &item.target {
                    let mut v = RichText::new(t.display().to_string()).monospace();
                    if !item.target_exists {
                        v = RichText::new(format!("{} (file missing)", t.display())).monospace().color(RED);
                    }
                    row(ui, "Program", v);
                }
                if let Some(s) = item.file_size {
                    row(ui, "File size", RichText::new(format_bytes(s)));
                }
                if let Some(m) = item.memory.filter(|_| item.service.is_some()) {
                    row(ui, "Memory now", RichText::new(format_bytes(m)));
                }
                row(ui, "Location", RichText::new(&item.location).monospace());
                for (k, v) in &item.facts {
                    let mut t = RichText::new(v);
                    if k == "Warning" {
                        t = t.color(RED);
                    }
                    row(ui, k, t);
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new("Command").small().color(TEXT_DIM));
            let mut cmd = item.command.clone();
            ui.add(TextEdit::multiline(&mut cmd).font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY).interactive(true));
            ui.add_space(8.0);

            ui.horizontal_wrapped(|ui| {
                if let Some(t) = item.target.as_ref().filter(|_| item.target_exists) {
                    if ui.button(format!("{} Open file location", icon::FOLDER_OPEN)).clicked() {
                        shell::reveal_in_explorer(t);
                    }
                }
                if ui.button(format!("{} Search online", icon::GLOBE)).clicked() {
                    let file = item.target.as_ref().and_then(|t| t.file_name()).map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                    shell::search_web(&format!("{} {file} startup safe to disable", item.name));
                }
                if ui.button(format!("{} Copy command", icon::COPY)).clicked() {
                    ctx.copy_text(item.command.clone());
                    toasts.info(&ctx, "Command copied to clipboard.");
                }
            });
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context, elevated: bool, toasts: &mut Toasts) {
        // Confirmation for risky disables.
        if let Some(p) = &mut self.pending {
            let Some(item) = self.items.iter().find(|i| i.id == p.id).cloned() else {
                self.pending = None;
                return;
            };
            let color = theme::crit_color(item.criticality);
            let is_service = item.source == SourceKind::Service;
            let mut confirm = false;
            let mut cancel = false;
            let modal = egui::Modal::new(Id::new("confirm_disable"))
                .frame(egui::Frame::new().fill(theme::CARD).stroke(Stroke::new(2.0, color)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20)))
                .show(ctx, |ui| {
                    ui.set_width(480.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(theme::crit_icon(item.criticality)).size(30.0).color(color));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(format!("Disable \"{}\"?", item.name)).font(FontId::new(18.0, theme::heading_family())));
                            ui.label(RichText::new(item.criticality.label()).color(color).strong());
                        });
                    });
                    ui.add_space(10.0);
                    match item.criticality {
                        Criticality::Critical => {
                            callout(
                                ui,
                                RED,
                                icon::WARNING_OCTAGON,
                                "This is a critical Windows component",
                                &format!(
                                    "{}\n\nDisabling it can leave Windows unstable, unable to boot, sign in, connect to the network or stay protected. Only continue if you know exactly why you need this.",
                                    item.criticality_reason
                                ),
                            );
                        }
                        Criticality::Important => {
                            callout(ui, ORANGE, icon::WARNING, "This affects a Windows feature or your hardware", &item.criticality_reason);
                        }
                        Criticality::Optional => {
                            callout(ui, GREEN, icon::CHECK_CIRCLE, "Safe to disable", &item.criticality_reason);
                        }
                    }
                    ui.add_space(10.0);

                    if is_service {
                        ui.label(RichText::new("How should it start from now on?").strong());
                        ui.radio_value(&mut p.mode, ServiceMode::Manual, "Manual — only starts when another program needs it (recommended)");
                        ui.radio_value(&mut p.mode, ServiceMode::Disabled, "Disabled — never starts, even when needed");
                        if p.mode == ServiceMode::Disabled && item.criticality != Criticality::Optional {
                            ui.label(RichText::new("Disabled services can break things that depend on them. Manual is much safer.").small().color(ORANGE));
                        }
                        ui.add_space(8.0);
                    }

                    let ready = match item.criticality {
                        Criticality::Critical => {
                            ui.label(format!("Type {} to confirm:", "DISABLE"));
                            ui.add(TextEdit::singleline(&mut p.typed).hint_text("DISABLE").desired_width(160.0));
                            p.typed.trim() == "DISABLE"
                        }
                        Criticality::Important => {
                            ui.checkbox(&mut p.acknowledged, "I understand what this affects");
                            p.acknowledged
                        }
                        Criticality::Optional => true,
                    };
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!(
                            "{} You can switch it back on at any time, here or from History.{}",
                            icon::ARROW_COUNTER_CLOCKWISE,
                            if is_service { " Changes take effect at the next restart." } else { "" }
                        ))
                        .small()
                        .color(TEXT_DIM),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if theme::danger_button(ui, format!("{} Disable", icon::POWER), color, ready).clicked() {
                            confirm = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        if item.criticality != Criticality::Optional {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.add_enabled(elevated && self.restore_rx.is_none(), egui::Button::new(format!("{} Restore point first", icon::SHIELD_CHECK))).clicked() {
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    let c = ctx.clone();
                                    std::thread::spawn(move || {
                                        let _ = tx.send(actions::create_restore_point());
                                        c.request_repaint();
                                    });
                                    self.restore_rx = Some(rx);
                                    toasts.info(ctx, "Creating a restore point…");
                                }
                            });
                        }
                    });
                });
            if modal.should_close() {
                cancel = true;
            }
            if confirm {
                let (id, mode) = (p.id.clone(), p.mode);
                self.pending = None;
                self.apply(&id, false, mode, ctx, toasts);
            } else if cancel {
                self.pending = None;
            }
        }

        // Administrator rights needed.
        if let Some(what) = self.admin_prompt.clone() {
            let mut close = false;
            let modal = egui::Modal::new(Id::new("admin_prompt")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::LOCK).size(28.0).color(ORANGE));
                    ui.label(RichText::new("Administrator rights needed").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.add_space(8.0);
                ui.add(Label::new(format!("Wield needs to run as administrator to {what}. Restart now? You'll see a Windows UAC prompt.")).wrap());
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

        // Undo history.
        if self.history.is_some() {
            let mut close = false;
            let mut revert: Option<usize> = None;
            let journal = self.history.as_ref().unwrap();
            let modal = egui::Modal::new(Id::new("history")).show(ctx, |ui| {
                ui.set_width(640.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::CLOCK_COUNTER_CLOCKWISE).size(24.0).color(ACCENT));
                    ui.label(RichText::new("Change history").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.label(RichText::new(format!("Saved in {}", Journal::location())).small().color(TEXT_DIM));
                ui.add_space(8.0);
                if journal.entries.is_empty() {
                    ui.label(RichText::new("No changes yet.").color(TEXT_DIM));
                }
                egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                    egui::Grid::new("history_grid").num_columns(4).striped(true).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                        for (idx, e) in journal.entries.iter().enumerate().rev() {
                            ui.label(RichText::new(journal::format_time(e.when)).small().color(TEXT_DIM));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&e.item_name).strong());
                                ui.label(RichText::new(e.source.label()).small().color(TEXT_DIM));
                            });
                            ui.label(&e.action);
                            if e.reverted {
                                ui.label(RichText::new("Reverted").color(TEXT_DIM));
                            } else if ui.button(format!("{} Revert", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                                revert = Some(idx);
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
            if let Some(idx) = revert {
                match actions::revert(idx) {
                    Ok(name) => {
                        toasts.success(ctx, format!("Restored \"{name}\" to its previous state."));
                        self.rescan(ctx);
                    }
                    Err(e) => toasts.error(ctx, format!("Revert failed: {e}")),
                }
                self.history = Some(Journal::load());
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

/// Box with a colored left accent, an icon, a title and wrapped body text.
fn callout(ui: &mut Ui, color: Color32, ic: &str, title: &str, body: &str) {
    let resp = egui::Frame::new()
        .fill(color.gamma_multiply(0.08))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin { left: 14, right: 10, top: 8, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic).color(color));
                ui.label(RichText::new(title).strong().color(color));
            });
            ui.add(Label::new(RichText::new(body)).wrap());
        });
    let r = resp.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_max(r.left_top(), egui::pos2(r.left() + 4.0, r.bottom())),
        CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 },
        color,
    );
}

fn legend(ui: &mut Ui) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("How entries are rated").strong());
        ui.add_space(4.0);
        for c in [Criticality::Optional, Criticality::Important, Criticality::Critical] {
            ui.horizontal(|ui| {
                theme::badge(ui, format!("{} {}", theme::crit_icon(c), c.label()), theme::crit_color(c));
            });
            let text = match c {
                Criticality::Optional => "Windows doesn't need it. Turn off freely.",
                Criticality::Important => "Turns off a feature, driver helper or updates. Asks you to confirm.",
                Criticality::Critical => "Windows depends on it. You must type DISABLE to confirm.",
            };
            ui.label(RichText::new(text).small().color(TEXT_DIM));
            ui.add_space(4.0);
        }
        ui.separator();
        ui.label(
            RichText::new("Impact is estimated from memory use, program size, known heavy apps and start mode.").small().color(TEXT_DIM),
        );
    });
}

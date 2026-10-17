//! Context Menu Manager screen.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Id, Label, Layout, Margin, RichText, Stroke, Ui};
use egui_extras::{Column, TableBuilder};
use egui_phosphor::regular as icon;

use super::custom::{self, Command, Custom, Position, Window};
use super::edit::{self, Backup, Deleted};
use super::scan::{self, Entry, Kind, Loc};
use crate::theme;
use crate::tools::backup::image::Level;
use crate::toast::Toasts;
use crate::win::{elevation, shell, with_com};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Menus,
    Add,
    Backups,
    Win11,
}

enum Done {
    Scan(Vec<Entry>),
    /// (title, result, rescan afterwards)
    Op(String, Result<String, String>, bool),
    Picked(PickFor, Vec<PathBuf>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickFor {
    Program(usize),
    Icon(usize),
    Test(usize),
    RestoreFolder,
}

enum Confirm {
    Delete(Entry),
    RestoreBackup(PathBuf, Backup),
    DeleteBackup(PathBuf),
    RemoveCustom(Custom),
    Classic(bool),
}

pub struct ContextMenuManager {
    win11: bool,
    tab: Tab,
    entries: Option<Vec<Entry>>,
    rx: Option<Receiver<Done>>,
    place: Loc,
    search: String,
    type_search: String,
    show_windows: bool,
    needs_restart: bool,
    confirm: Option<Confirm>,
    confirm_text: String,
    remove_new: bool,
    // Add your own
    draft: Custom,
    customs: Vec<Custom>,
    last_test: Option<String>,
    // Backups
    backups: Vec<(PathBuf, Backup)>,
    deleted: Vec<Deleted>,
    backup_label: String,
    first_backup_checked: bool,
}

impl ContextMenuManager {
    pub fn new(build: u32) -> Self {
        Self {
            win11: build >= 22000,
            tab: Tab::Menus,
            entries: None,
            rx: None,
            place: Loc::Files,
            search: String::new(),
            type_search: String::new(),
            show_windows: true,
            needs_restart: false,
            confirm: None,
            confirm_text: String::new(),
            remove_new: false,
            draft: Custom::default(),
            customs: custom::load(),
            last_test: None,
            backups: edit::backups(),
            deleted: edit::deleted(),
            backup_label: String::new(),
            first_backup_checked: false,
        }
    }

    pub fn set_tab(&mut self, name: &str) {
        self.tab = match name.to_lowercase().as_str() {
            "add" | "custom" => Tab::Add,
            "backups" | "deleted" => Tab::Backups,
            "win11" | "windows11" => Tab::Win11,
            _ => Tab::Menus,
        };
    }

    pub fn summary_line(&self) -> Option<String> {
        let e = self.entries.as_ref()?;
        let hidden = e.iter().filter(|e| e.hidden).count();
        let mine = self.customs.len();
        Some(format!("{} entries · {hidden} hidden · {mine} added by you", e.len()))
    }

    fn busy(&self) -> bool {
        self.rx.is_some()
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let e = with_com(scan::scan_all);
            let _ = tx.send(Done::Scan(e));
            c.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn run(&mut self, ctx: &egui::Context, title: &str, rescan: bool, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (c, t) = (ctx.clone(), title.to_string());
        std::thread::spawn(move || {
            let r = with_com(f);
            let _ = tx.send(Done::Op(t, r, rescan));
            c.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn pick(&mut self, ctx: &egui::Context, what: PickFor, folder: bool, title: &'static str) {
        let (tx, rx) = std::sync::mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let v = with_com(move || if folder { shell::pick_folder(title).into_iter().collect() } else { shell::pick_files(title) });
            let _ = tx.send(Done::Picked(what, v));
            c.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(rx) = &self.rx else { return };
        let Ok(done) = rx.try_recv() else { return };
        self.rx = None;
        match done {
            Done::Scan(e) => {
                self.entries = Some(e);
                // The very first time, keep a copy of the menus exactly as they were before any change.
                if !self.first_backup_checked {
                    self.first_backup_checked = true;
                    if self.backups.is_empty() {
                        let entries = self.entries.clone().unwrap_or_default();
                        self.run(ctx, "First backup", false, move || {
                            edit::create_backup(&entries, "Original menus (made automatically the first time)").map(|_| "Saved a backup of your menus as they were, so you can always go back.".into())
                        });
                    }
                }
            }
            Done::Op(title, r, rescan) => {
                match r {
                    Ok(m) => toasts.success(ctx, if m.is_empty() { format!("{title}: done.") } else { m }),
                    Err(e) => toasts.error(ctx, format!("{title}: {e}")),
                }
                self.backups = edit::backups();
                self.deleted = edit::deleted();
                self.customs = custom::load();
                if rescan {
                    self.rescan(ctx);
                }
            }
            Done::Picked(what, paths) => {
                let Some(p) = paths.into_iter().next() else { return };
                match what {
                    PickFor::Program(i) => {
                        if let Some(c) = self.draft.commands.get_mut(i) {
                            c.program = p.display().to_string();
                            if c.label.trim().is_empty() {
                                c.label = p.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
                            }
                        }
                    }
                    PickFor::Icon(i) => {
                        if let Some(c) = self.draft.commands.get_mut(i) {
                            c.icon = format!("{},0", p.display());
                        }
                    }
                    PickFor::Test(i) => {
                        if let Some(c) = self.draft.commands.get(i).cloned() {
                            let ph = self.draft.targets.scopes().first().map(|s| s.1).unwrap_or("%1");
                            match custom::test(&c, ph, &p) {
                                Ok(line) => {
                                    self.last_test = Some(line);
                                    toasts.info(ctx, format!("Started \"{}\" for {}", c.label, p.display()));
                                }
                                Err(e) => toasts.error(ctx, e),
                            }
                        }
                    }
                    PickFor::RestoreFolder => match edit::read_backup(&p) {
                        Some(b) => {
                            self.confirm_text.clear();
                            self.confirm = Some(Confirm::RestoreBackup(p, b));
                        }
                        None => toasts.error(ctx, "That folder doesn't contain a Wield menu backup (menus.json)."),
                    },
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if self.entries.is_none() && self.rx.is_none() {
            self.rescan(&ctx);
        }
        self.poll(&ctx, toasts);

        egui::Panel::top("ctx_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui, elevated, toasts));

        if self.entries.is_none() {
            egui::CentralPanel::no_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.add(egui::Spinner::new().size(36.0).color(theme::accent()));
                    ui.label(RichText::new("Reading every right-click menu (all file types, folders, desktop, drives)…").color(theme::text_dim()));
                });
            });
            return;
        }
        match self.tab {
            Tab::Menus => self.menus_tab(ui, elevated),
            Tab::Add => {
                egui::CentralPanel::no_frame().show(ui, |ui| scroll_pad(ui, |ui| self.add_tab(ui, elevated)));
            }
            Tab::Backups => {
                egui::CentralPanel::no_frame().show(ui, |ui| scroll_pad(ui, |ui| self.backups_tab(ui, elevated)));
            }
            Tab::Win11 => {
                egui::CentralPanel::no_frame().show(ui, |ui| scroll_pad(ui, |ui| self.win11_tab(ui)));
            }
        }
        self.dialogs(&ctx, elevated);
    }

    fn header(&mut self, ui: &mut Ui, elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::MOUSE_RIGHT_CLICK).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Context Menu Manager");
                ui.label(RichText::new("Every right-click menu entry for files, folders, the desktop and drives. Hide, delete and restore entries, or add your own.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(!self.busy(), egui::Button::new(format!("{} Rescan", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.rescan(&ctx);
                }
                if ui.add_enabled(!self.busy(), egui::Button::new(format!("{} Restart Explorer", icon::ARROW_CLOCKWISE))).on_hover_text("Applies changes Explorer has cached. Open Explorer windows close and the taskbar reloads.").clicked() {
                    self.needs_restart = false;
                    self.run(&ctx, "Restart Explorer", false, || edit::restart_explorer().map(|_| "Explorer restarted.".into()));
                }
                if self.busy() {
                    ui.spinner();
                }
            });
        });
        ui.add_space(6.0);
        if self.needs_restart {
            let mut clicked = false;
            banner(ui, theme::aqua(), icon::INFO, "Changes to shell extensions and app entries show up after Explorer restarts (or after you sign out and back in).", Some(("Restart Explorer now", &mut clicked)));
            if clicked {
                self.needs_restart = false;
                self.run(&ctx, "Restart Explorer", false, || edit::restart_explorer().map(|_| "Explorer restarted.".into()));
            }
        }
        if !elevated {
            let mut clicked = false;
            banner(
                ui,
                theme::orange(),
                icon::LOCK,
                "Hiding entries and adding your own works now, for your account. Deleting or restoring entries installed for all users needs administrator rights.",
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
                (Tab::Menus, icon::LIST_BULLETS, "Menus"),
                (Tab::Add, icon::PLUS_CIRCLE, "Add your own"),
                (Tab::Backups, icon::CLOCK_COUNTER_CLOCKWISE, "Backups & deleted"),
                (Tab::Win11, icon::WINDOWS_LOGO, "Windows 11 menu"),
            ] {
                let sel = self.tab == t;
                let text = RichText::new(format!("{ic} {label}")).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                    if t == Tab::Backups {
                        self.backups = edit::backups();
                        self.deleted = edit::deleted();
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------------ Menus

    fn menus_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let entries = self.entries.clone().unwrap_or_default();

        egui::Panel::left("ctx_places")
            .resizable(false)
            .exact_size(250.0)
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 10, top: 6, bottom: 10 }))
            .show(ui, |ui| {
                let count = |l: &Loc| entries.iter().filter(|e| e.locs.contains(l) && (self.show_windows || !e.microsoft)).count();
                ui.label(RichText::new("WHERE YOU RIGHT-CLICK").small().color(theme::text_dim()));
                for l in Loc::PLACES {
                    let ic = place_icon(&l);
                    let n = count(&l);
                    let sel = self.place == l;
                    if ui.add(egui::Button::new(RichText::new(format!("{ic}  {}   {n}", l.label())).color(if sel { theme::text() } else { theme::text_dim() })).selected(sel).frame_when_inactive(false).min_size(egui::vec2(ui.available_width(), 26.0))).clicked() {
                        self.place = l;
                    }
                }
                ui.add_space(10.0);
                ui.label(RichText::new("FILE TYPES").small().color(theme::text_dim()));
                ui.add(egui::TextEdit::singleline(&mut self.type_search).hint_text(".webp, .pdf, .zip…").desired_width(f32::INFINITY));
                // Types with extra (non-Windows) entries come first; searching shows any type.
                let q = self.type_search.trim().trim_start_matches('*').to_lowercase();
                let mut types: Vec<(String, usize)> = Vec::new();
                let mut seen = std::collections::HashMap::<String, usize>::new();
                for e in &entries {
                    for l in &e.locs {
                        if let Loc::FileType(x) = l {
                            if (q.is_empty() && !e.microsoft) || (!q.is_empty() && x.contains(&q)) {
                                *seen.entry(x.clone()).or_default() += 1;
                            }
                        }
                    }
                }
                types.extend(seen);
                types.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                if !q.is_empty() && !types.iter().any(|t| t.0 == q || t.0 == format!(".{q}")) {
                    let ext = if q.starts_with('.') { q.clone() } else { format!(".{q}") };
                    if ui.small_button(format!("Show {ext}")).clicked() {
                        self.place = Loc::FileType(ext);
                    }
                }
                egui::ScrollArea::vertical().id_salt("types").show(ui, |ui| {
                    for (t, n) in types.iter().take(300) {
                        let l = Loc::FileType(t.clone());
                        let sel = self.place == l;
                        let label = if q.is_empty() { format!("{t}   {n} added by apps") } else { t.clone() };
                        if ui.add(egui::Button::new(RichText::new(label).color(if sel { theme::text() } else { theme::text_dim() })).selected(sel).frame_when_inactive(false)).clicked() {
                            self.place = l;
                        }
                    }
                });
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin { left: 10, right: 20, top: 6, bottom: 10 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} {}", place_icon(&self.place), self.place.label())).font(FontId::new(17.0, theme::heading_family())));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("{} Search all menus", icon::MAGNIFYING_GLASS)).desired_width(240.0));
                        ui.checkbox(&mut self.show_windows, "Show Windows' own entries");
                    });
                });
                let q = self.search.trim().to_lowercase();
                let ft_files = matches!(self.place, Loc::FileType(_));
                let mut rows: Vec<&Entry> = entries
                    .iter()
                    .filter(|e| self.show_windows || !e.microsoft)
                    .filter(|e| {
                        if !q.is_empty() {
                            return e.name.to_lowercase().contains(&q) || e.command.to_lowercase().contains(&q) || e.publisher.to_lowercase().contains(&q) || e.key.to_lowercase().contains(&q);
                        }
                        e.locs.contains(&self.place) || (ft_files && e.locs.contains(&Loc::Files))
                    })
                    .collect();
                rows.sort_by(|a, b| (a.microsoft, a.name.to_lowercase()).cmp(&(b.microsoft, b.name.to_lowercase())));
                if !q.is_empty() {
                    ui.label(RichText::new(format!("{} result(s) across every menu", rows.len())).color(theme::text_dim()));
                } else {
                    ui.label(RichText::new(place_help(&self.place)).color(theme::text_dim()));
                }
                ui.add_space(4.0);
                if rows.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("Nothing here.").color(theme::text_dim())));
                    return;
                }
                self.table(ui, &ctx, &rows, elevated, !q.is_empty() || ft_files);
            });
        });
    }

    fn table(&mut self, ui: &mut Ui, ctx: &egui::Context, rows: &[&Entry], elevated: bool, show_place: bool) {
        let busy = self.busy();
        let mut action: Option<(&'static str, Entry)> = None;
        TableBuilder::new(ui)
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::exact(52.0))
            .column(Column::remainder().at_least(220.0).clip(true))
            .column(Column::exact(130.0))
            .column(Column::exact(150.0).clip(true))
            .column(Column::remainder().at_least(160.0).clip(true))
            .column(Column::exact(40.0))
            .header(22.0, |mut h| {
                for t in ["Shown", "Entry", "Type", "Publisher", if show_place { "Where / runs" } else { "Runs" }, ""] {
                    h.col(|ui| {
                        ui.label(RichText::new(t).small().color(theme::text_dim()));
                    });
                }
            })
            .body(|body| {
                body.rows(30.0, rows.len(), |mut row| {
                    let e = rows[row.index()];
                    row.col(|ui| {
                        let can = e.can_hide() && !busy;
                        let r = theme::switch(ui, !e.hidden && !e.never_shown, theme::green(), can);
                        if r.clicked() && can {
                            action = Some((if e.hidden { "show" } else { "hide" }, e.clone()));
                        }
                        if e.never_shown {
                            r.on_hover_text("Registered for programs to use; Windows never shows it in the menu.");
                        } else if e.kind == Kind::New {
                            r.on_hover_text("New-menu items can be removed (and restored later) with the ⋯ button.");
                        }
                    });
                    row.col(|ui| {
                        let color = if e.hidden || e.never_shown { theme::text_dim() } else { theme::text() };
                        ui.add(Label::new(RichText::new(&e.name).color(color)).truncate());
                        if e.custom {
                            theme::badge(ui, "Yours", theme::purple());
                        }
                        if e.shift_only {
                            theme::badge(ui, "Shift", theme::aqua()).on_hover_text("Only shows with Shift+right-click");
                        }
                        if e.never_shown {
                            theme::badge(ui, "Never shown", theme::text_dim());
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(e.kind.label()).small().color(theme::text_dim()));
                    });
                    row.col(|ui| {
                        let p = if e.publisher.is_empty() { if e.microsoft { "Windows" } else { "Unknown" } } else { e.publisher.as_str() };
                        ui.add(Label::new(RichText::new(p).color(if e.microsoft { theme::text_dim() } else { theme::text() })).truncate());
                    });
                    row.col(|ui| {
                        let txt = if show_place { format!("{} · {}", e.place(), e.command) } else { e.command.clone() };
                        ui.add(Label::new(RichText::new(txt).small().monospace().color(theme::text_dim())).truncate()).on_hover_text(details(e));
                    });
                    row.col(|ui| {
                        ui.add_enabled_ui(!busy, |ui| {
                            ui.menu_button(icon::DOTS_THREE_VERTICAL, |ui| {
                                if e.kind == Kind::Command && ui.button(if e.shift_only { format!("{} Show without Shift", icon::KEYBOARD) } else { format!("{} Only with Shift+right-click", icon::KEYBOARD) }).clicked() {
                                    action = Some((if e.shift_only { "unshift" } else { "shift" }, e.clone()));
                                }
                                if e.can_delete() && ui.button(RichText::new(format!("{} Delete…", icon::TRASH)).color(theme::red())).clicked() {
                                    action = Some(("delete", e.clone()));
                                }
                                if !e.can_delete() && matches!(e.kind, Kind::Command | Kind::Extension | Kind::New) {
                                    ui.label(RichText::new("Windows' own entry: can be hidden, not deleted").small().color(theme::text_dim()));
                                }
                                ui.separator();
                                if let Some(p) = e.reg_path() {
                                    if ui.button(format!("{} Open in Registry Editor", icon::TREE_STRUCTURE)).clicked() {
                                        let h = if e.in_user { super::reg::Hive::User } else { super::reg::Hive::Machine };
                                        edit::open_in_regedit(h, &p);
                                        ui.close();
                                    }
                                }
                                if let Some(f) = &e.file {
                                    if ui.button(format!("{} Open file location", icon::FOLDER_OPEN)).clicked() {
                                        shell::reveal_in_explorer(f);
                                        ui.close();
                                    }
                                }
                                if ui.button(format!("{} Copy details", icon::COPY)).clicked() {
                                    ui.ctx().copy_text(details(e));
                                    ui.close();
                                }
                            });
                        });
                    });
                });
            });
        if let Some((what, e)) = action {
            match what {
                "delete" => {
                    if e.in_machine && !elevated && e.kind != Kind::SendTo {
                        self.confirm = None;
                        let c = ctx.clone();
                        self.run(&c, "Delete", false, || Err("This entry is installed for all users; deleting it needs administrator rights. You can hide it instead.".into()));
                    } else {
                        self.confirm_text.clear();
                        self.confirm = Some(Confirm::Delete(e));
                    }
                }
                _ => {
                    // Quick registry edits: apply now and update the row in place.
                    let r = match what {
                        "hide" => edit::set_hidden(&e, true),
                        "show" => edit::set_hidden(&e, false),
                        "shift" => edit::set_shift_only(&e, true),
                        _ => edit::set_shift_only(&e, false),
                    };
                    match r {
                        Ok(_) => {
                            if let Some(all) = &mut self.entries {
                                for x in all.iter_mut() {
                                    // Blocking a shell extension hides it everywhere it's registered.
                                    let same_clsid = e.clsid.is_some() && x.clsid == e.clsid && matches!(x.kind, Kind::Extension | Kind::App);
                                    if x.id == e.id || (same_clsid && (what == "hide" || what == "show")) {
                                        match what {
                                            "hide" => x.hidden = true,
                                            "show" => x.hidden = false,
                                            "shift" => x.shift_only = true,
                                            _ => x.shift_only = false,
                                        }
                                    }
                                }
                            }
                            if matches!(e.kind, Kind::Extension | Kind::App) {
                                self.needs_restart = true;
                            }
                        }
                        Err(er) => {
                            let c = ctx.clone();
                            self.run(&c, &e.name, false, move || Err(er));
                        }
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ Add your own

    fn add_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        let editing = !self.draft.id.is_empty();
        section(
            ui,
            icon::PLUS_CIRCLE,
            if editing { "Edit your entry" } else { "Add your own entry" },
            "Run any program, batch file (.bat/.cmd), PowerShell script (.ps1) or command when you right-click. One entry or a whole submenu.",
        );
        if !editing {
            ui.horizontal(|ui| {
                ui.label("Start from:");
                egui::ComboBox::from_id_salt("ctx_template").selected_text("Choose an example…").width(380.0).show_ui(ui, |ui| {
                    for (name, t) in custom::templates() {
                        if ui.selectable_label(false, name).clicked() {
                            self.draft = t;
                            self.last_test = None;
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }

        // Where.
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("1. Where should it appear?").strong());
            let t = &mut self.draft.targets;
            ui.horizontal(|ui| {
                ui.label(format!("{} Files of type", icon::FILE_IMAGE));
                ui.add(egui::TextEdit::singleline(&mut t.types).hint_text(".webp  (several: .webp .png .heic)").desired_width(260.0));
            });
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut t.files, "All files");
                ui.checkbox(&mut t.folders, "Folders");
                ui.checkbox(&mut t.background, "Inside a folder (empty space)");
                ui.checkbox(&mut t.desktop, "Desktop");
                ui.checkbox(&mut t.drives, "Drives");
            });
        });
        ui.add_space(8.0);

        // What.
        let mut remove_cmd = None;
        let mut pick: Option<(PickFor, bool, &'static str)> = None;
        let folder_target = {
            let t = &self.draft.targets;
            !t.files && t.extensions().is_empty() && (t.folders || t.background || t.desktop || t.drives)
        };
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("2. What should it do?").strong());
            ui.horizontal(|ui| {
                ui.label("Submenu name");
                ui.add(egui::TextEdit::singleline(&mut self.draft.submenu).hint_text("optional, e.g. Image tools").desired_width(240.0));
                ui.label(RichText::new("Leave empty to put the command(s) straight in the menu.").small().color(theme::text_dim()));
            });
            if !self.draft.submenu.trim().is_empty() {
                ui.horizontal(|ui| {
                    ui.label("Submenu icon");
                    ui.add(egui::TextEdit::singleline(&mut self.draft.icon).hint_text(r"C:\path\app.exe,0  or  imageres.dll,-68").desired_width(300.0));
                });
            }
            let n = self.draft.commands.len();
            for (i, c) in self.draft.commands.iter_mut().enumerate() {
                ui.add_space(6.0);
                egui::Frame::new().stroke(Stroke::new(1.0, theme::border())).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::Grid::new(("cmd", i)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                        ui.label("Menu text");
                        ui.add(egui::TextEdit::singleline(&mut c.label).hint_text("Convert to JPEG").desired_width(320.0));
                        ui.end_row();
                        ui.label("Program or script");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut c.program).hint_text(r"C:\Scripts\to-jpeg.bat").desired_width(420.0));
                            if ui.button(format!("{} Browse…", icon::FOLDER_OPEN)).clicked() {
                                pick = Some((PickFor::Program(i), false, "Choose the program or script to run"));
                            }
                        });
                        ui.end_row();
                        ui.label("Arguments");
                        ui.vertical(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut c.args).desired_width(420.0).font(egui::TextStyle::Monospace));
                            ui.label(RichText::new("{path} becomes the file or folder you right-clicked. Keep the quotes: \"{path}\". Leave Program empty to type a full command line here.").small().color(theme::text_dim()));
                        });
                        ui.end_row();
                        ui.label("Window");
                        egui::ComboBox::from_id_salt(("win", i)).selected_text(c.window.label()).width(300.0).show_ui(ui, |ui| {
                            for w in [Window::Normal, Window::KeepOpen, Window::Hidden] {
                                ui.selectable_value(&mut c.window, w, w.label());
                            }
                        });
                        ui.end_row();
                        ui.label("Icon");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut c.icon).hint_text("optional; programs use their own icon").desired_width(320.0));
                            if ui.button("Browse…").clicked() {
                                pick = Some((PickFor::Icon(i), false, "Choose a program, .dll or .ico for the icon"));
                            }
                        });
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        let can_test = !c.label.trim().is_empty() && (!c.program.trim().is_empty() || !c.args.trim().is_empty());
                        if ui.add_enabled(can_test, egui::Button::new(format!("{} Test with a {}…", icon::PLAY, if folder_target { "folder" } else { "file" }))).on_hover_text("Runs the command now on a file or folder you choose, exactly as the menu will").clicked() {
                            pick = Some((PickFor::Test(i), folder_target, "Choose a file or folder to test the command on"));
                        }
                        if n > 1 && ui.button(RichText::new(format!("{} Remove this command", icon::TRASH)).color(theme::red())).clicked() {
                            remove_cmd = Some(i);
                        }
                    });
                });
            }
            ui.add_space(6.0);
            if ui.button(format!("{} Add another command{}", icon::PLUS, if self.draft.submenu.trim().is_empty() { "" } else { " to the submenu" })).clicked() {
                self.draft.commands.push(Command::default());
            }
        });
        if let Some(i) = remove_cmd {
            self.draft.commands.remove(i);
        }
        if let Some((p, folder, title)) = pick {
            if !busy {
                self.pick(&ctx, p, folder, title);
            }
        }
        ui.add_space(8.0);

        // Options.
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("3. Options").strong());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.draft.shift_only, "Only show with Shift+right-click");
                ui.label("Position:");
                ui.radio_value(&mut self.draft.position, Position::Default, "Normal");
                ui.radio_value(&mut self.draft.position, Position::Top, "Top");
                ui.radio_value(&mut self.draft.position, Position::Bottom, "Bottom");
                ui.add_enabled(elevated, egui::Checkbox::new(&mut self.draft.all_users, "For all users on this PC")).on_disabled_hover_text("Needs administrator rights");
            });
            if self.win11 {
                ui.label(RichText::new(format!("{} On Windows 11 your entries appear under \"Show more options\", unless you switch to the classic menu (Windows 11 menu tab).", icon::INFO)).small().color(theme::text_dim()));
            }
        });
        ui.add_space(8.0);

        // Preview and checks.
        let checks = custom::validate(&self.draft, elevated, &self.customs);
        let blocked = checks.iter().any(|c| c.level == Level::Block);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("4. Check and save").strong());
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
            if let Some((_, ph)) = self.draft.targets.scopes().first() {
                let wield = std::env::current_exe().unwrap_or_default();
                ui.add_space(4.0);
                ui.label(RichText::new("Windows will run:").small().color(theme::text_dim()));
                for c in &self.draft.commands {
                    ui.add(Label::new(RichText::new(custom::command_line(c, ph, &wield)).monospace().small()).wrap());
                }
            }
            if let Some(t) = &self.last_test {
                ui.label(RichText::new(format!("Last test ran: {t}")).small().color(theme::text_dim()));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = if editing { format!("{} Save changes", icon::CHECK) } else { format!("{} Add to menu", icon::PLUS) };
                if ui.add_enabled(!blocked && !busy, egui::Button::new(RichText::new(label).color(theme::on_accent())).fill(theme::accent()).min_size(egui::vec2(0.0, 32.0))).clicked() {
                    let (d, el) = (self.draft.clone(), elevated);
                    let where_ = d.targets.describe();
                    let title = d.title();
                    self.run(&ctx, "Your entry", true, move || custom::save(d, el).map(|_| format!("\"{title}\" added to the menu for {where_}.")));
                    self.draft = Custom::default();
                    self.last_test = None;
                }
                if (editing || self.draft != Custom::default()) && ui.button("Clear").clicked() {
                    self.draft = Custom::default();
                    self.last_test = None;
                }
            });
        });
        ui.add_space(14.0);

        section(ui, icon::USER, "Your entries", "Entries you added with Wield. They're also listed (marked \"Yours\") in Menus.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.customs.is_empty() {
                ui.label(RichText::new("None yet.").color(theme::text_dim()));
            }
            let mut edit_this = None;
            for c in self.customs.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if c.submenu.trim().is_empty() { icon::CURSOR_CLICK } else { icon::LIST_BULLETS }).color(theme::accent()));
                    ui.label(RichText::new(c.title()).strong());
                    ui.label(RichText::new(format!("on {}", c.targets.describe())).color(theme::text_dim()));
                    if c.all_users {
                        theme::badge(ui, "All users", theme::aqua());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!busy, egui::Button::new(RichText::new(icon::TRASH).color(theme::red()))).on_hover_text("Remove from the menu").clicked() {
                            self.confirm = Some(Confirm::RemoveCustom(c.clone()));
                        }
                        if ui.add_enabled(!busy, egui::Button::new(format!("{} Edit", icon::PENCIL_SIMPLE))).clicked() {
                            edit_this = Some(c.clone());
                        }
                    });
                });
            }
            if let Some(c) = edit_this {
                self.draft = c;
                self.last_test = None;
            }
        });
    }

    // ------------------------------------------------------------------ Backups & deleted

    fn backups_tab(&mut self, ui: &mut Ui, elevated: bool) {
        let ctx = ui.ctx().clone();
        let busy = self.busy();
        section(ui, icon::CLOCK_COUNTER_CLOCKWISE, "Backups", "A backup is a complete copy of every menu: commands, shell extensions, hidden entries, Send To, the New menu and the Windows 11 menu setting.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.backup_label).hint_text("Name (optional), e.g. Before cleaning up").desired_width(320.0));
                if ui.add_enabled(!busy, egui::Button::new(RichText::new(format!("{} Back up now", icon::FLOPPY_DISK)).color(theme::on_accent())).fill(theme::accent())).clicked() {
                    let entries = self.entries.clone().unwrap_or_default();
                    let label = if self.backup_label.trim().is_empty() { "Manual backup".to_string() } else { self.backup_label.trim().to_string() };
                    self.backup_label.clear();
                    self.run(&ctx, "Backup", false, move || edit::create_backup(&entries, &label).map(|d| format!("Backup saved to {}", d.display())));
                }
                if ui.add_enabled(!busy, egui::Button::new(format!("{} Restore from a folder…", icon::FOLDER_OPEN))).on_hover_text("A backup folder copied from elsewhere (contains menus.json)").clicked() {
                    self.pick(&ctx, PickFor::RestoreFolder, true, "Choose a Wield menu backup folder");
                }
            });
            ui.add_space(6.0);
            if self.backups.is_empty() {
                ui.label(RichText::new("No backups yet.").color(theme::text_dim()));
            }
            for (dir, b) in self.backups.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::ARCHIVE).color(theme::accent()));
                    ui.label(crate::tools::startup::journal::format_time(b.when));
                    ui.label(RichText::new(&b.label).strong());
                    ui.label(RichText::new(format!("{} keys, {} Send To items", b.keys.len(), b.sendto.len())).small().color(theme::text_dim()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!busy, egui::Button::new(RichText::new(icon::TRASH).color(theme::red()))).on_hover_text("Delete this backup").clicked() {
                            self.confirm = Some(Confirm::DeleteBackup(dir.clone()));
                        }
                        if ui.button(icon::FOLDER_OPEN).on_hover_text("Show in Explorer (copy the folder to keep it elsewhere)").clicked() {
                            shell::reveal_in_explorer(&dir);
                        }
                        if ui.add_enabled(!busy, egui::Button::new(format!("{} Restore…", icon::ARROW_COUNTER_CLOCKWISE))).clicked() {
                            self.confirm_text.clear();
                            self.remove_new = false;
                            self.confirm = Some(Confirm::RestoreBackup(dir.clone(), b.clone()));
                        }
                    });
                });
            }
        });
        ui.add_space(14.0);

        section(ui, icon::TRASH, "Deleted entries", "Everything you delete with Wield is saved here first. Restore puts it back exactly as it was.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.deleted.is_empty() {
                ui.label(RichText::new("Nothing deleted.").color(theme::text_dim()));
            }
            let mut restore = None;
            let mut forget = None;
            for (i, d) in self.deleted.iter().enumerate().rev() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::TRASH).color(theme::text_dim()));
                    ui.label(RichText::new(&d.name).strong());
                    ui.label(RichText::new(format!("{} · {} · {}", d.kind.label(), d.place, crate::tools::startup::journal::format_time(d.when))).small().color(theme::text_dim()));
                    let all_users = d.keys.iter().any(|k| k.0 == super::reg::Hive::Machine);
                    if all_users {
                        theme::badge(ui, "All users", theme::aqua());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!busy, egui::Button::new(icon::X)).on_hover_text("Remove from this list (the entry stays deleted)").clicked() {
                            forget = Some(i);
                        }
                        let can = !busy && (elevated || !all_users);
                        if ui.add_enabled(can, egui::Button::new(format!("{} Restore", icon::ARROW_COUNTER_CLOCKWISE))).on_disabled_hover_text("Needs administrator rights").clicked() {
                            restore = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = restore {
                self.run(&ctx, "Restore", true, move || edit::restore_deleted(i, elevated));
            }
            if let Some(i) = forget {
                self.run(&ctx, "Deleted list", false, move || edit::forget_deleted(i).map(|_| "Removed from the list.".into()));
            }
        });
    }

    // ------------------------------------------------------------------ Windows 11

    fn win11_tab(&mut self, ui: &mut Ui) {
        section(ui, icon::WINDOWS_LOGO, "Windows 11 menu", "Windows 11 shows a short menu and hides everything else under \"Show more options\". You can switch back to the full classic menu.");
        let classic = edit::classic_menu();
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !self.win11 {
                ui.label(format!("{} This PC runs Windows 10, which always shows the full menu. Nothing to change here.", icon::INFO));
                if classic {
                    ui.label(RichText::new("A classic-menu setting from Windows 11 is present; it has no effect on Windows 10.").small().color(theme::text_dim()));
                }
                return;
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Currently:").strong());
                if classic {
                    theme::badge(ui, "Classic full menu", theme::green());
                } else {
                    theme::badge(ui, "Windows 11 compact menu", theme::accent());
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if !classic && theme::primary_button(ui, format!("{} Use the classic full menu", icon::LIST_BULLETS)).clicked() {
                    self.confirm = Some(Confirm::Classic(true));
                }
                if classic && ui.button(format!("{} Back to the Windows 11 menu", icon::WINDOWS_LOGO)).clicked() {
                    self.confirm = Some(Confirm::Classic(false));
                }
            });
            ui.label(RichText::new("Applies to your account only and needs an Explorer restart, which Wield does for you. Shift+right-click always opens the classic menu too.").small().color(theme::text_dim()));
        });
        ui.add_space(14.0);
        let apps: Vec<Entry> = self.entries.iter().flatten().filter(|e| e.kind == Kind::App).cloned().collect();
        section(ui, icon::APP_WINDOW, "App entries in the Windows 11 menu", "Store and packaged apps add these to the compact menu. Hide them in Menus like any other entry.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if apps.is_empty() {
                ui.label(RichText::new("No packaged apps add menu entries on this PC.").color(theme::text_dim()));
            }
            for e in apps {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if e.hidden { icon::EYE_SLASH } else { icon::EYE }).color(if e.hidden { theme::text_dim() } else { theme::green() }));
                    ui.label(&e.name);
                    ui.label(RichText::new(e.place()).small().color(theme::text_dim()));
                });
            }
        });
    }

    // ------------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context, elevated: bool) {
        let Some(c) = &self.confirm else { return };
        let (head, ic, col, body, word, button): (String, &str, Color32, String, &str, &str) = match c {
            Confirm::Delete(e) => (
                format!("Delete \"{}\"?", e.name),
                icon::TRASH,
                theme::red(),
                format!(
                    "{} · {}\n{}\n\nAn exact copy is saved first; you can restore it any time from Backups & deleted. If you only want it out of the menu, hiding it (the switch) is simpler.",
                    e.kind.label(),
                    e.place(),
                    e.command
                ),
                "",
                "Delete",
            ),
            Confirm::RestoreBackup(_, b) => (
                "Restore this backup?".into(),
                icon::ARROW_COUNTER_CLOCKWISE,
                theme::orange(),
                format!(
                    "{} ({})\n\nEvery menu goes back to how it was then: deleted entries return and entries hidden since are shown again. Your current menus are backed up automatically first.{}",
                    b.label,
                    crate::tools::startup::journal::format_time(b.when),
                    if elevated { "" } else { "\n\nNot running as administrator: entries installed for all users are skipped." }
                ),
                "RESTORE",
                "Restore",
            ),
            Confirm::DeleteBackup(_) => ("Delete this backup?".into(), icon::TRASH, theme::red(), "The backup folder is deleted. Your menus don't change.".into(), "", "Delete backup"),
            Confirm::RemoveCustom(cu) => (
                format!("Remove \"{}\"?", cu.title()),
                icon::TRASH,
                theme::red(),
                format!("It disappears from the menu for {}. Your program or script itself isn't touched.", cu.targets.describe()),
                "",
                "Remove",
            ),
            Confirm::Classic(on) => (
                if *on { "Switch to the classic full menu?".into() } else { "Switch back to the Windows 11 menu?".into() },
                icon::WINDOWS_LOGO,
                theme::accent(),
                "Explorer restarts to apply this: open Explorer windows close and the taskbar reloads for a moment. Save anything you're renaming or moving first.".into(),
                "",
                "Switch and restart Explorer",
            ),
        };
        let mut go = false;
        let mut cancel = false;
        let modal = egui::Modal::new(Id::new("ctx_confirm")).show(ctx, |ui| {
            ui.set_width(500.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic).size(28.0).color(col));
                ui.add(Label::new(RichText::new(&head).font(FontId::new(18.0, theme::heading_family()))).wrap());
            });
            ui.add_space(8.0);
            ui.add(Label::new(body.as_str()).wrap());
            if matches!(c, Confirm::RestoreBackup(..)) {
                ui.add_space(6.0);
                ui.checkbox(&mut self.remove_new, "Also remove entries added after this backup").on_hover_text("Off: entries installed since are kept. On: menus become exactly as in the backup.");
            }
            if !word.is_empty() {
                ui.add_space(8.0);
                ui.label(format!("Type {word} to confirm:"));
                ui.add(egui::TextEdit::singleline(&mut self.confirm_text).desired_width(200.0));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = word.is_empty() || self.confirm_text.trim().eq_ignore_ascii_case(word);
                if theme::danger_button(ui, button, col, ok).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if modal.should_close() || cancel {
            self.confirm = None;
            return;
        }
        if !go {
            return;
        }
        let entries = self.entries.clone().unwrap_or_default();
        match self.confirm.take().unwrap() {
            Confirm::Delete(e) => {
                let ext = matches!(e.kind, Kind::Extension);
                self.run(ctx, "Delete", true, move || edit::delete(&e, elevated));
                self.needs_restart |= ext;
            }
            Confirm::RestoreBackup(dir, _) => {
                let remove_new = self.remove_new;
                self.needs_restart = true;
                self.run(ctx, "Restore backup", true, move || {
                    let r = edit::restore_backup(&dir, &entries, remove_new, elevated)?;
                    let mut msg = format!("Menus restored ({} keys).", r.restored);
                    if r.skipped_admin > 0 {
                        msg.push_str(&format!(" {} all-users key(s) skipped: run as administrator to restore those too.", r.skipped_admin));
                    }
                    if r.errors.is_empty() { Ok(msg) } else { Err(format!("{msg} Some keys failed: {}", r.errors.join("; "))) }
                });
            }
            Confirm::DeleteBackup(dir) => self.run(ctx, "Delete backup", false, move || edit::delete_backup(&dir).map(|_| "Backup deleted.".into())),
            Confirm::RemoveCustom(cu) => {
                let t = cu.title();
                self.run(ctx, "Remove", true, move || custom::remove(&cu.id).map(|_| format!("\"{t}\" removed from the menu.")));
            }
            Confirm::Classic(on) => self.run(ctx, "Windows 11 menu", false, move || {
                edit::set_classic_menu(on)?;
                edit::restart_explorer()?;
                Ok(if on { "Classic menu on. Right-click anything to see it.".into() } else { "Windows 11 menu is back.".into() })
            }),
        }
    }
}

fn place_icon(l: &Loc) -> &'static str {
    match l {
        Loc::Files => icon::FILE,
        Loc::Folders => icon::FOLDER_SIMPLE,
        Loc::Background => icon::FOLDER_OPEN,
        Loc::Desktop => icon::DESKTOP,
        Loc::Drives => icon::HARD_DRIVE,
        Loc::ThisPc => icon::COMPUTER_TOWER,
        Loc::RecycleBin => icon::RECYCLE,
        Loc::SendTo => icon::PAPER_PLANE_TILT,
        Loc::New => icon::FILE_PLUS,
        Loc::FileType(_) => icon::FILE_IMAGE,
    }
}

fn place_help(l: &Loc) -> String {
    match l {
        Loc::Files => "Shown when you right-click any file.".into(),
        Loc::Folders => "Shown when you right-click a folder.".into(),
        Loc::Background => "Shown when you right-click empty space inside an Explorer window.".into(),
        Loc::Desktop => "Shown when you right-click empty space on the desktop.".into(),
        Loc::Drives => "Shown when you right-click a drive in This PC.".into(),
        Loc::ThisPc => "Shown when you right-click This PC.".into(),
        Loc::RecycleBin => "Shown when you right-click the Recycle Bin.".into(),
        Loc::SendTo => "The Send to submenu. Hidden items are kept by Wield and can be shown again.".into(),
        Loc::New => "The New submenu (right-click empty space → New). Remove items you never create; they can be restored.".into(),
        Loc::FileType(e) => format!("Everything shown when you right-click a {e} file, including entries for all files."),
    }
}

fn details(e: &Entry) -> String {
    let mut s = format!("{}\n{}\nWhere: {}", e.name, e.kind.label(), e.place());
    if !e.command.is_empty() {
        s.push_str(&format!("\nRuns: {}", e.command));
    }
    if let Some(c) = &e.clsid {
        s.push_str(&format!("\nCLSID: {c}"));
    }
    if let Some(p) = e.reg_path() {
        let hives: Vec<&str> = [(e.in_machine, "HKLM (all users)"), (e.in_user, "HKCU (you)")].iter().filter(|x| x.0).map(|x| x.1).collect();
        s.push_str(&format!("\nRegistry: {p}  [{}]", hives.join(", ")));
    }
    if let Some(p) = &e.package {
        s.push_str(&format!("\nApp: {p}"));
    }
    if !e.publisher.is_empty() {
        s.push_str(&format!("\nPublisher: {}", e.publisher));
    }
    s
}

fn scroll_pad(ui: &mut Ui, f: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 16 }).show(ui, f);
    });
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

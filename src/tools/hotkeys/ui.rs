//! Keyboard Shortcuts screen.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Label, Layout, Margin, RichText, Stroke, Ui, vec2};
use egui_phosphor::regular as icon;

use super::catalog::{BUILTIN, Builtin, Cat, Control, Os, Toggle};
use super::keys::{self, Combo};
use super::sources::{self, KNOWN_APPS, LinkKey, PtShortcut};
use super::toggles;
use crate::theme;
use crate::toast::Toasts;
use crate::win::{shell, with_com};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Windows,
    Apps,
    InUse,
}

/// Who holds a key combination right now.
#[derive(Clone, Debug)]
enum Owner {
    Windows(String),
    PowerToys(String),
    Shortcut(String),
    App(String),
    Reserved(&'static str),
    Unknown,
}

struct Loaded {
    links: Vec<LinkKey>,
    powertoys: Vec<PtShortcut>,
    remaps: Vec<(String, String)>,
    taken: Vec<Combo>,
    running: HashSet<String>,
}

fn load(ctx: egui::Context) -> Receiver<Loaded> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let links = with_com(sources::link_keys);
        let mut powertoys = sources::powertoys();
        // PowerToys stores each shortcut twice: the current one and a "Default …" copy.
        powertoys.retain(|p| !p.name.starts_with("Default "));
        let l = Loaded { links, powertoys, remaps: sources::powertoys_remaps(), taken: keys::taken(keys::probe_candidates()), running: sources::running_processes() };
        let _ = tx.send(l);
        ctx.request_repaint();
    });
    rx
}

pub struct ShortcutManager {
    build: u32,
    tab: Tab,
    data: Option<Loaded>,
    rx: Option<Receiver<Loaded>>,
    op_rx: Option<Receiver<(String, Result<String, String>)>>,
    search: String,
    only_changeable: bool,
    all_versions: bool,
    needs_explorer: bool,
    states: HashMap<Toggle, Result<bool, String>>,
    disabled_win: String,
    // "Give a program a shortcut key"
    new_target: Option<PathBuf>,
    new_combo: Combo,
    edit_link: Option<(LinkKey, Combo)>,
    test_combo: Combo,
    test_result: Option<String>,
}

const KEY_CHOICES: &[u32] = &[
    0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35,
    0x36, 0x37, 0x38, 0x39, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x20, 0x2C, 0x24, 0x23, 0x2D, 0x2E, 0x21, 0x22, 0xC0, 0xBB, 0xBD,
];

impl ShortcutManager {
    pub fn new(build: u32) -> Self {
        Self {
            build,
            tab: Tab::Windows,
            data: None,
            rx: None,
            op_rx: None,
            search: String::new(),
            only_changeable: false,
            all_versions: false,
            needs_explorer: false,
            states: HashMap::new(),
            disabled_win: String::new(),
            new_target: None,
            new_combo: Combo { ctrl: true, alt: true, vk: 0x4E, ..Default::default() },
            edit_link: None,
            test_combo: Combo { ctrl: true, alt: true, vk: 0x4B, ..Default::default() },
            test_result: None,
        }
    }

    pub fn set_tab(&mut self, name: &str) {
        self.tab = match name.to_lowercase().as_str() {
            "apps" | "programs" => Tab::Apps,
            "inuse" | "in-use" => Tab::InUse,
            _ => Tab::Windows,
        };
    }

    pub fn summary_line(&self) -> Option<String> {
        let d = self.data.as_ref()?;
        let off = self.states.values().filter(|s| matches!(s, Ok(false))).count() + self.disabled_win.len().min(1);
        Some(format!("{} shortcuts in use right now{}", d.taken.len(), if off > 0 { " · some turned off" } else { "" }))
    }

    fn refresh_states(&mut self) {
        let toggles: HashSet<Toggle> = BUILTIN.iter().filter_map(|b| if let Control::Toggle(t) = b.control { Some(t) } else { None }).chain([Toggle::AllWinKeys]).collect();
        self.states = toggles.into_iter().map(|t| (t, toggles::is_on(t, self.build))).collect();
        self.disabled_win = toggles::disabled_win_keys();
    }

    fn reload(&mut self, ctx: &egui::Context) {
        if self.rx.is_none() {
            self.rx = Some(load(ctx.clone()));
        }
        self.refresh_states();
    }

    fn run(&mut self, ctx: &egui::Context, title: &str, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (c, t) = (ctx.clone(), title.to_string());
        std::thread::spawn(move || {
            let r = with_com(f);
            let _ = tx.send((t, r));
            c.request_repaint();
        });
        self.op_rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        if let Some(rx) = &self.rx {
            if let Ok(d) = rx.try_recv() {
                self.data = Some(d);
                self.rx = None;
            }
        }
        if let Some(rx) = &self.op_rx {
            if let Ok((t, r)) = rx.try_recv() {
                self.op_rx = None;
                match r {
                    Ok(m) => toasts.success(ctx, m),
                    Err(e) => toasts.error(ctx, format!("{t}: {e}")),
                }
                self.reload(ctx);
            }
        }
    }

    /// Who holds a combination, as far as Wield can tell.
    fn owner(&self, c: &Combo) -> Owner {
        let Some(d) = &self.data else { return Owner::Unknown };
        if c.vk == 0x7B && !c.ctrl && !c.alt && !c.shift && !c.win {
            return Owner::Reserved("Reserved by Windows for debuggers; no app can use F12 on its own as a global hotkey.");
        }
        if let Some(b) = BUILTIN.iter().find(|b| Combo::parse(b.keys).as_ref() == Some(c)) {
            return Owner::Windows(b.action.to_string());
        }
        if let Some(p) = d.powertoys.iter().find(|p| p.enabled && &p.combo == c) {
            return Owner::PowerToys(format!("{}: {}", p.module, p.name));
        }
        if let Some(l) = d.links.iter().find(|l| l.combo.as_ref() == Some(c)) {
            return Owner::Shortcut(l.name.clone());
        }
        if let Some((_, app, _, action)) = KNOWN_APPS.iter().find(|(p, _, k, _)| d.running.contains(*p) && Combo::parse(k).as_ref() == Some(c)) {
            return Owner::App(format!("{app}: {action}"));
        }
        if c.win {
            return Owner::Windows("A Windows feature (Game Bar, Snipping, accessibility…) or a running app".into());
        }
        Owner::Unknown
    }

    fn is_taken(&self, c: &Combo) -> Option<bool> {
        self.data.as_ref().map(|d| d.taken.contains(c))
    }

    pub fn show(&mut self, ui: &mut Ui, _elevated: bool, toasts: &mut Toasts) {
        let ctx = ui.ctx().clone();
        if self.data.is_none() && self.rx.is_none() {
            self.reload(&ctx);
        }
        self.poll(&ctx, toasts);

        egui::Panel::top("hk_header")
            .frame(egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 6 }))
            .show(ui, |ui| self.header(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 20, right: 20, top: 4, bottom: 20 }).show(ui, |ui| match self.tab {
                    Tab::Windows => self.windows_tab(ui),
                    Tab::Apps => self.apps_tab(ui),
                    Tab::InUse => self.in_use_tab(ui),
                });
            });
        });
        self.edit_dialog(&ctx);
    }

    fn header(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::KEYBOARD).size(26.0).color(theme::accent()));
            ui.vertical(|ui| {
                ui.heading("Keyboard Shortcuts");
                ui.label(RichText::new("Every shortcut on this PC in one place: Windows' own, your programs', PowerToys and whatever running apps have claimed. Turn off the ones you don't want.").color(theme::text_dim()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add_enabled(self.rx.is_none(), egui::Button::new(format!("{} Refresh", icon::ARROWS_CLOCKWISE))).clicked() {
                    self.reload(&ctx);
                }
                if self.rx.is_some() || self.op_rx.is_some() {
                    ui.spinner();
                }
            });
        });
        ui.add_space(6.0);
        if self.needs_explorer {
            let mut clicked = false;
            banner(ui, theme::aqua(), icon::INFO, "Some changes take effect after Explorer restarts (open Explorer windows close and the taskbar reloads for a moment).", Some(("Restart Explorer now", &mut clicked)));
            if clicked {
                self.needs_explorer = false;
                self.run(&ctx, "Restart Explorer", || crate::tools::ctxmenu::edit::restart_explorer().map(|_| "Explorer restarted; your shortcut changes are active.".into()));
            }
        }
        ui.horizontal(|ui| {
            for (t, ic, label) in [(Tab::Windows, icon::WINDOWS_LOGO, "Windows shortcuts"), (Tab::Apps, icon::APP_WINDOW, "Programs & PowerToys"), (Tab::InUse, icon::LIST_CHECKS, "In use right now")] {
                let sel = self.tab == t;
                let text = RichText::new(format!("{ic} {label}")).color(if sel { theme::text() } else { theme::text_dim() });
                if ui.add(egui::Button::new(text).selected(sel).frame_when_inactive(false)).clicked() {
                    self.tab = t;
                }
            }
            ui.add_space(20.0);
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(format!("{} Search keys or actions", icon::MAGNIFYING_GLASS)).desired_width(240.0));
        });
    }

    fn matches(&self, keys: &str, action: &str) -> bool {
        let q = self.search.trim().to_lowercase();
        q.is_empty() || keys.to_lowercase().contains(&q) || action.to_lowercase().contains(&q)
    }

    // ------------------------------------------------------------------ Windows shortcuts

    fn windows_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // Master switch.
        let all_on = matches!(self.states.get(&Toggle::AllWinKeys), Some(Ok(true)) | None);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if theme::switch(ui, all_on, theme::green(), self.op_rx.is_none()).clicked() {
                    match toggles::set(Toggle::AllWinKeys, !all_on) {
                        Ok(()) => {
                            self.needs_explorer = true;
                            self.refresh_states();
                        }
                        Err(e) => self.run(&ctx, "Windows-key shortcuts", move || Err(e)),
                    }
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new("All Windows-key shortcuts").strong());
                    ui.label(RichText::new("Turns every Win+ shortcut on or off at once, for example for gaming. The Windows key alone still opens Start. Applies after Explorer restarts.").small().color(theme::text_dim()));
                });
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.only_changeable, "Only shortcuts Wield can turn off");
            ui.checkbox(&mut self.all_versions, if self.build >= 22000 { "Also show Windows 10-only shortcuts" } else { "Also show Windows 11-only shortcuts" });
        });
        ui.add_space(6.0);
        let win11 = self.build >= 22000;
        for cat in Cat::ALL {
            let rows: Vec<&Builtin> = BUILTIN
                .iter()
                .filter(|b| b.cat == cat)
                .filter(|b| self.all_versions || match b.os {
                    Os::Both => true,
                    Os::Win11 => win11,
                    Os::Win10 => !win11,
                })
                .filter(|b| !self.only_changeable || !matches!(b.control, Control::Fixed(_)))
                .filter(|b| self.matches(b.keys, b.action))
                .collect();
            if rows.is_empty() {
                continue;
            }
            ui.label(RichText::new(cat.label()).font(FontId::new(17.0, theme::heading_family())));
            ui.add_space(4.0);
            theme::card_frame().inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for b in rows {
                    self.builtin_row(ui, b, all_on);
                }
            });
            ui.add_space(12.0);
        }
    }

    fn builtin_row(&mut self, ui: &mut Ui, b: &Builtin, all_win_on: bool) {
        let ctx = ui.ctx().clone();
        let combo = Combo::parse(b.keys);
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            // Switch or info icon.
            match b.control {
                Control::Fixed(why) => {
                    ui.add_sized(vec2(36.0, 20.0), Label::new(RichText::new(icon::INFO).color(theme::text_dim()))).on_hover_text(why);
                }
                Control::WinKeys(k) => {
                    let on = !k.chars().all(|c| self.disabled_win.contains(c));
                    let r = theme::switch(ui, on, theme::green(), self.op_rx.is_none() && all_win_on);
                    if r.clicked() && all_win_on {
                        match toggles::set_win_keys(k, !on) {
                            Ok(()) => {
                                self.needs_explorer = true;
                                self.refresh_states();
                            }
                            Err(e) => self.run(&ctx, b.keys, move || Err(e)),
                        }
                    }
                    let shared = if k.len() > 1 { format!("Covers Win+{} at once. ", k.chars().map(String::from).collect::<Vec<_>>().join("/")) } else { String::new() };
                    r.on_hover_text(format!("{shared}Explorer stops responding to Win+{} (any Win+{} combination). Applies after Explorer restarts.", &k[..1], &k[..1]));
                }
                Control::Toggle(t) => {
                    let st = self.states.get(&t).cloned().unwrap_or(Ok(true));
                    let on = *st.as_ref().unwrap_or(&true);
                    let r = theme::switch(ui, on, theme::green(), st.is_ok() && self.op_rx.is_none());
                    if r.clicked() {
                        match toggles::set(t, !on) {
                            Ok(()) => {
                                if matches!(t, Toggle::Shake | Toggle::AllWinKeys) {
                                    self.needs_explorer = true;
                                }
                                self.refresh_states();
                            }
                            Err(e) => self.run(&ctx, t.label(), move || Err(e)),
                        }
                    }
                    r.on_hover_text(format!("{}. {}", t.label(), t.applies()));
                }
            }
            keycaps(ui, b.keys);
            ui.add(Label::new(b.action).wrap());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match b.os {
                    Os::Win10 => {
                        theme::badge(ui, "Windows 10", theme::text_dim());
                    }
                    Os::Win11 => {
                        theme::badge(ui, "Windows 11", theme::text_dim());
                    }
                    Os::Both => {}
                }
                // Live check: does something hold this combination right now?
                if let (Some(c), Some(d)) = (combo, &self.data) {
                    let held = d.taken.contains(&c);
                    let configured_off = matches!(b.control, Control::WinKeys(k) if k.chars().all(|ch| self.disabled_win.contains(ch)));
                    if configured_off && held && !self.needs_explorer {
                        theme::badge(ui, "Still active", theme::yellow()).on_hover_text("Turned off, but something still uses it: another part of Windows handles this one, so it can't be turned off this way.");
                    }
                }
                if let Control::Fixed(_) = b.control {
                    ui.label(RichText::new("Can't be turned off").small().color(theme::text_dim()));
                }
            });
        });
    }

    // ------------------------------------------------------------------ Programs & PowerToys

    fn apps_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(d) = &self.data else {
            spinner(ui, "Looking for shortcut keys…");
            return;
        };
        let links = d.links.clone();
        let pts = d.powertoys.clone();
        let remaps = d.remaps.clone();
        let running = d.running.clone();

        section(ui, icon::LINK, "Shortcut keys on your shortcuts", "The \"Shortcut key\" set on Start menu, desktop and taskbar shortcuts. Pressing it opens that program from anywhere.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if links.is_empty() {
                ui.label(RichText::new("None of your shortcuts has a shortcut key. Add one below.").color(theme::text_dim()));
            }
            let shown: Vec<LinkKey> = links.iter().filter(|l| self.matches(&l.combo.or(l.disabled).map(|c| c.label()).unwrap_or_default(), &l.name)).cloned().collect();
            for l in &shown {
                ui.horizontal(|ui| {
                    let on = l.combo.is_some();
                    if theme::switch(ui, on, theme::green(), self.op_rx.is_none()).on_hover_text("Off removes the key from the shortcut; Wield remembers it so you can turn it back on.").clicked() {
                        let l2 = l.clone();
                        self.run(&ctx, &l.name, move || if on { sources::disable_link(&l2).map(|_| format!("Shortcut key of {} turned off.", l2.name)) } else { sources::enable_link(&l2).map(|_| format!("Shortcut key of {} is back.", l2.name)) });
                    }
                    keycaps(ui, &l.combo.or(l.disabled).map(|c| c.label()).unwrap_or_default());
                    ui.label(RichText::new(&l.name).strong());
                    ui.add(Label::new(RichText::new(&l.target).small().color(theme::text_dim())).truncate());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if l.all_users {
                            theme::badge(ui, "All users", theme::aqua()).on_hover_text("Changing it needs administrator rights");
                        }
                        if ui.small_button(format!("{} Change", icon::PENCIL_SIMPLE)).clicked() {
                            self.edit_link = Some((l.clone(), l.combo.or(l.disabled).unwrap_or_default()));
                        }
                        if ui.small_button(icon::FOLDER_OPEN).on_hover_text("Show the shortcut in Explorer").clicked() {
                            shell::reveal_in_explorer(&l.path);
                        }
                    });
                });
            }
        });
        ui.add_space(8.0);

        // Give a program a key.
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Give a program a shortcut key").strong());
            ui.label(RichText::new("Creates a Start menu shortcut (in \"Wield shortcuts\") with the key you choose, so the program opens from anywhere.").small().color(theme::text_dim()));
            ui.horizontal(|ui| {
                if ui.button(format!("{} Choose program…", icon::FOLDER_OPEN)).clicked() {
                    if let Some(p) = with_com(shell::pick_program) {
                        self.new_target = Some(p);
                    }
                }
                ui.label(self.new_target.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "No program chosen".into()));
            });
            let mut c = self.new_combo;
            combo_editor(ui, "new", &mut c, false);
            self.new_combo = c;
            let check = sources::valid_link_combo(&c).err().map(String::from).or_else(|| self.conflict(&c));
            if let Some(w) = &check {
                ui.label(RichText::new(format!("{} {w}", icon::WARNING)).color(theme::yellow()));
            }
            let can = self.new_target.is_some() && sources::valid_link_combo(&c).is_ok() && self.op_rx.is_none();
            if ui.add_enabled(can, egui::Button::new(RichText::new(format!("{} Create shortcut key {}", icon::PLUS, c.label())).color(theme::on_accent())).fill(theme::accent())).clicked() {
                let t = self.new_target.take().unwrap();
                self.run(&ctx, "Shortcut key", move || sources::new_program_key(&t, c).map(|p| format!("{} now opens {}.", c.label(), p.file_stem().unwrap_or_default().to_string_lossy())));
            }
        });
        ui.add_space(14.0);

        section(ui, icon::LIGHTNING, "PowerToys", "PowerToys' own shortcuts. Change them in PowerToys Settings; Wield shows them so you can see what's taken.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if pts.is_empty() {
                ui.label(RichText::new("PowerToys isn't installed (App Installer can install it).").color(theme::text_dim()));
            }
            let mut last = String::new();
            for p in pts.iter().filter(|p| self.matches(&p.combo.label(), &format!("{} {}", p.module, p.name))) {
                if p.module != last {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&p.module).strong());
                        if !p.enabled {
                            theme::badge(ui, "Module off", theme::text_dim());
                        }
                    });
                    last = p.module.clone();
                }
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    keycaps(ui, &p.combo.label());
                    ui.label(RichText::new(&p.name).color(if p.enabled { theme::text() } else { theme::text_dim() }));
                });
            }
            if !remaps.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("Keyboard Manager remaps").strong());
                for (from, to) in &remaps {
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        keycaps(ui, from);
                        ui.label(RichText::new(format!("→ {to}")).color(theme::text_dim()));
                    });
                }
            }
        });
        ui.add_space(14.0);

        section(ui, icon::APP_WINDOW, "Other apps", "Default global hotkeys of popular apps that are running now. Change them in each app's settings.");
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            let rows: Vec<_> = KNOWN_APPS.iter().filter(|(p, ..)| running.contains(*p)).filter(|(_, a, k, act)| self.matches(k, &format!("{a} {act}"))).collect();
            if rows.is_empty() {
                ui.label(RichText::new("None of the apps Wield knows (NVIDIA and AMD overlays, ShareX, Greenshot, Flow Launcher, Windows Terminal, Ditto…) is running.").color(theme::text_dim()));
            }
            for (_, app, k, act) in rows {
                let c = Combo::parse(k);
                ui.horizontal(|ui| {
                    keycaps(ui, k);
                    ui.label(RichText::new(*app).strong());
                    ui.label(RichText::new(*act).color(theme::text_dim()));
                    if let Some(true) = c.and_then(|c| self.is_taken(&c)) {
                        theme::badge(ui, "Active", theme::green());
                    }
                });
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Apps can also have shortcuts that only work inside them (like Ctrl+S); those are listed in each app's menus or help.").small().color(theme::text_dim()));
        });
    }

    /// Who already uses a combination, as a warning.
    fn conflict(&self, c: &Combo) -> Option<String> {
        if self.is_taken(c) != Some(true) {
            return None;
        }
        Some(match self.owner(c) {
            Owner::Windows(a) => format!("{} is already used by Windows ({a}).", c.label()),
            Owner::PowerToys(a) => format!("{} is already used by PowerToys ({a}).", c.label()),
            Owner::Shortcut(n) => format!("{} already opens {n}.", c.label()),
            Owner::App(a) => format!("{} is already used by {a}.", c.label()),
            Owner::Reserved(r) => r.to_string(),
            Owner::Unknown => format!("{} is already taken by a running program.", c.label()),
        })
    }

    fn edit_dialog(&mut self, ctx: &egui::Context) {
        let Some((l, mut c)) = self.edit_link.clone() else { return };
        let mut save = false;
        let mut close = false;
        let m = egui::Modal::new(egui::Id::new("hk_edit")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(format!("Shortcut key for {}", l.name)).font(FontId::new(18.0, theme::heading_family())));
            ui.add_space(6.0);
            combo_editor(ui, "edit", &mut c, false);
            let invalid = sources::valid_link_combo(&c).err();
            if let Some(w) = invalid.map(String::from).or_else(|| if Some(c) == l.combo { None } else { self.conflict(&c) }) {
                ui.label(RichText::new(format!("{} {w}", icon::WARNING)).color(theme::yellow()));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(invalid.is_none(), egui::Button::new(RichText::new("Save").color(theme::on_accent())).fill(theme::accent())).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if save {
            let path = l.path.clone();
            self.run(ctx, "Shortcut key", move || sources::set_link_key(&path, Some(c)).map(|_| format!("Shortcut key changed to {}.", c.label())));
            self.edit_link = None;
        } else if close || m.should_close() {
            self.edit_link = None;
        } else {
            self.edit_link = Some((l, c));
        }
    }

    // ------------------------------------------------------------------ In use right now

    fn in_use_tab(&mut self, ui: &mut Ui) {
        let Some(d) = &self.data else {
            spinner(ui, "Checking which key combinations are taken…");
            return;
        };
        let taken = d.taken.clone();
        section(
            ui,
            icon::LIST_CHECKS,
            "Global hotkeys taken right now",
            "Wield tried each common key combination; these are already claimed by Windows or a running program. Windows doesn't say which program holds a hotkey, so \"a running program\" means: close apps one by one to find it.",
        );
        // Check a combination.
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Is a combination free?").strong());
            let mut c = self.test_combo;
            combo_editor(ui, "test", &mut c, true);
            if c != self.test_combo {
                self.test_result = None;
            }
            self.test_combo = c;
            ui.horizontal(|ui| {
                if ui.button(format!("{} Check {}", icon::MAGNIFYING_GLASS, c.label())).clicked() {
                    let held = !keys::taken(vec![c]).is_empty();
                    self.test_result = Some(if held { self.conflict(&c).unwrap_or_else(|| format!("{} is taken.", c.label())) } else { format!("{} is free: no program uses it as a global hotkey right now.", c.label()) });
                }
                if let Some(r) = &self.test_result {
                    let free = r.contains("is free");
                    ui.label(RichText::new(r).color(if free { theme::green() } else { theme::yellow() }));
                }
            });
        });
        ui.add_space(10.0);
        // The surprising ones first; Windows' own (the long list) last.
        let mut groups: Vec<(&str, Vec<(Combo, String)>)> = vec![("A running program", vec![]), ("Apps", vec![]), ("PowerToys", vec![]), ("Your shortcuts", vec![]), ("Windows", vec![])];
        for c in &taken {
            let (g, what) = match self.owner(c) {
                Owner::Windows(a) => (4, a),
                Owner::Reserved(r) => (4, r.to_string()),
                Owner::PowerToys(a) => (2, a),
                Owner::Shortcut(n) => (3, format!("Opens {n}")),
                Owner::App(a) => (1, a),
                Owner::Unknown => (0, "Not a Windows or PowerToys shortcut; close apps one by one to find it".into()),
            };
            if self.matches(&c.label(), &what) {
                groups[g].1.push((*c, what));
            }
        }
        for (name, rows) in groups {
            if rows.is_empty() {
                continue;
            }
            ui.label(RichText::new(format!("{name} ({})", rows.len())).font(FontId::new(16.0, theme::heading_family())));
            theme::card_frame().inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (c, what) in rows {
                    ui.horizontal(|ui| {
                        keycaps(ui, &c.label());
                        ui.add(Label::new(RichText::new(what).color(theme::text_dim())).truncate());
                    });
                }
            });
            ui.add_space(10.0);
        }
    }
}

/// Draws "Ctrl+Alt+K" as key caps.
fn keycaps(ui: &mut Ui, keys: &str) {
    let w = ui.available_width().min(260.0);
    ui.allocate_ui_with_layout(vec2(w, 22.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_width(w);
        ui.spacing_mut().item_spacing.x = 3.0;
        let parts: Vec<&str> = if keys.contains('+') && !keys.ends_with('+') { keys.split('+').collect() } else { vec![keys] };
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                ui.label(RichText::new("+").small().color(theme::text_dim()));
            }
            egui::Frame::new()
                .fill(theme::card_hover())
                .stroke(Stroke::new(1.0, theme::border()))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(6, 1))
                .show(ui, |ui| {
                    ui.label(RichText::new(*p).monospace().size(12.5));
                });
        }
    });
}

/// Modifier checkboxes and a key picker.
fn combo_editor(ui: &mut Ui, id: &str, c: &mut Combo, allow_win: bool) {
    ui.horizontal(|ui| {
        if allow_win {
            ui.checkbox(&mut c.win, "Win");
        }
        ui.checkbox(&mut c.ctrl, "Ctrl");
        ui.checkbox(&mut c.alt, "Alt");
        ui.checkbox(&mut c.shift, "Shift");
        ui.label("+");
        egui::ComboBox::from_id_salt(("hk_key", id)).selected_text(keys::key_name(c.vk)).width(110.0).show_ui(ui, |ui| {
            for &vk in KEY_CHOICES {
                ui.selectable_value(&mut c.vk, vk, keys::key_name(vk));
            }
        });
    });
}

fn spinner(ui: &mut Ui, text: &str) {
    ui.add_space(30.0);
    ui.vertical_centered(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).color(theme::text_dim()));
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

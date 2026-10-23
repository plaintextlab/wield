//! Application shell: sidebar navigation, home grid and tool routing.

use eframe::egui::{self, Align, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icon;

use crate::theme;
use crate::toast::Toasts;
use crate::tools::apps::ui::AppInstaller;
use crate::tools::backup::ui::BackupManager;
use crate::tools::ctxmenu::ui::ContextMenuManager;
use crate::tools::debloat::ui::DebloatManager;
use crate::tools::drivers::ui::DriverManager;
use crate::tools::hotkeys::ui::ShortcutManager;
use crate::tools::malware::ui::MalwareManager;
use crate::tools::network::ui::NetworkManager;
use crate::tools::procs::ui::{Intent, ProcessManager};
use crate::tools::privacy::ui::PrivacyManager;
use crate::tools::startup::ui::StartupManager;
use crate::tools::updates::ui::UpdateManager;
use crate::win::elevation;
use crate::win::sysver::{OsInfo, os_info};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolId {
    Startup,
    Privacy,
    Debloat,
    Network,
    Malware,
    Processes,
    Backup,
    ContextMenu,
    Apps,
    Drivers,
    Hotkeys,
    Updates,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Tool(ToolId),
    Settings,
}

struct ToolInfo {
    id: Option<ToolId>,
    icon: &'static str,
    title: &'static str,
    blurb: &'static str,
}

/// Tools shown on the home grid. Entries with `id: None` are planned but not built yet.
const TOOLS: &[ToolInfo] = &[
    ToolInfo {
        id: Some(ToolId::Startup),
        icon: icon::ROCKET_LAUNCH,
        title: "Startup Manager",
        blurb: "Control apps, services and scheduled tasks that run when Windows starts.",
    },
    ToolInfo {
        id: Some(ToolId::Privacy),
        icon: icon::EYE_SLASH,
        title: "Privacy & Telemetry",
        blurb: "Turn off telemetry, ads, web search and tracking, with one-click restore.",
    },
    ToolInfo {
        id: Some(ToolId::Debloat),
        icon: icon::PACKAGE,
        title: "App Debloater",
        blurb: "Remove preinstalled Store apps you don't use. Reinstall any time.",
    },
    ToolInfo {
        id: Some(ToolId::Processes),
        icon: icon::CPU,
        title: "Process Explorer",
        blurb: "Every process with CPU, memory, I/O, open files, DLLs and services. End, suspend or throttle.",
    },
    ToolInfo {
        id: Some(ToolId::Malware),
        icon: icon::SHIELD_WARNING,
        title: "Malware Scanner",
        blurb: "Defender scans, signatures and online hash checks; exclusions and quarantine restore.",
    },
    ToolInfo {
        id: Some(ToolId::Apps),
        icon: icon::STOREFRONT,
        title: "App Installer",
        blurb: "90 free essentials (VLC, 7-Zip, Steam, OBS, VS Code…): tick them and install all at once, silently.",
    },
    ToolInfo {
        id: Some(ToolId::Drivers),
        icon: icon::CIRCUITRY,
        title: "Driver Manager",
        blurb: "Missing and outdated drivers: Windows Update, NVIDIA's newest graphics driver and your PC maker's tools.",
    },
    ToolInfo {
        id: Some(ToolId::Hotkeys),
        icon: icon::KEYBOARD,
        title: "Keyboard Shortcuts",
        blurb: "Every shortcut in one place: Windows', your programs' and PowerToys'. Turn off the ones you don't want.",
    },
    ToolInfo {
        id: Some(ToolId::Backup),
        icon: icon::FLOPPY_DISK,
        title: "Backup & Restore",
        blurb: "Bootable system images, restore points, folder backups and a reinstall kit.",
    },
    ToolInfo {
        id: Some(ToolId::ContextMenu),
        icon: icon::MOUSE_RIGHT_CLICK,
        title: "Context Menu",
        blurb: "Every right-click entry: hide, delete and restore, back up, or add your own commands.",
    },
    ToolInfo { id: None, icon: icon::BROOM, title: "Disk Cleanup", blurb: "Find and clear temp files, caches and leftovers." },
    ToolInfo {
        id: Some(ToolId::Updates),
        icon: icon::CLOUD_ARROW_DOWN,
        title: "Update Control",
        blurb: "Pause or run Windows Update, and update your installed apps, with the ones you choose left alone.",
    },
    ToolInfo {
        id: Some(ToolId::Network),
        icon: icon::NETWORK,
        title: "Network Toolkit",
        blurb: "Per-app traffic and blocking, open ports, network scanner, DNS and repairs.",
    },
    ToolInfo { id: None, icon: icon::TRASH, title: "Uninstaller", blurb: "Remove programs completely, including leftovers." },
    ToolInfo { id: None, icon: icon::DESKTOP, title: "System Info", blurb: "Hardware, drivers and Windows edition at a glance." },
];

pub struct WieldApp {
    view: View,
    os: OsInfo,
    elevated: bool,
    toasts: Toasts,
    startup: StartupManager,
    privacy: PrivacyManager,
    debloat: DebloatManager,
    network: NetworkManager,
    malware: MalwareManager,
    processes: ProcessManager,
    backup: BackupManager,
    ctxmenu: ContextMenuManager,
    apps: AppInstaller,
    drivers: DriverManager,
    hotkeys: ShortcutManager,
    updates: UpdateManager,
    settings: crate::settings::Settings,
    updater: crate::settings_ui::Updater,
    dashboard: crate::dashboard::Dashboard,
}

impl WieldApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = crate::settings::Settings::load();
        theme::install(&cc.egui_ctx, settings.dark);
        // `wield.exe --tool startup` opens a tool directly (handy for shortcuts).
        let args: Vec<String> = std::env::args().collect();
        let view = args.iter().position(|a| a == "--tool").and_then(|i| args.get(i + 1)).and_then(|n| view_for(n)).unwrap_or(View::Home);
        let mut network = NetworkManager::new();
        if let Some(tab) = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)) {
            network.set_tab(tab);
        }
        // `--scan <path>` (used by Explorer's "Scan with Wield") opens the scanner and starts right away.
        let mut malware = MalwareManager::new();
        let mut view = view;
        let mut processes = ProcessManager::new();
        if let Some(pid) = args.iter().position(|a| a == "--pid").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()) {
            processes.select_pid(pid);
        }
        let mut backup = BackupManager::new();
        let mut drivers = DriverManager::new();
        let mut updates = UpdateManager::new();
        let os = os_info();
        let mut hotkeys = ShortcutManager::new(os.build.split('.').next().and_then(|b| b.parse().ok()).unwrap_or(0));
        let mut ctxmenu = ContextMenuManager::new(os.build.split('.').next().and_then(|b| b.parse().ok()).unwrap_or(0));
        if let Some(tab) = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)) {
            malware.set_tab(tab);
            backup.set_tab(tab);
            ctxmenu.set_tab(tab);
            drivers.set_tab(tab);
            hotkeys.set_tab(tab);
            updates.set_tab(tab);
        }
        if let Some(p) = args.iter().position(|a| a == "--scan").and_then(|i| args.get(i + 1)) {
            view = View::Tool(ToolId::Malware);
            malware.scan_paths(vec![std::path::PathBuf::from(p)], &cc.egui_ctx);
        }
        let mut updater = crate::settings_ui::Updater::new();
        updater.maybe_auto_check(&settings, &cc.egui_ctx);
        Self { dashboard: crate::dashboard::Dashboard::new(), settings, updater, view, privacy: PrivacyManager::new(os.clone()), debloat: DebloatManager::new(), network, backup, ctxmenu, apps: AppInstaller::new(), drivers, hotkeys, updates, malware, processes, os, elevated: elevation::is_elevated(), toasts: Toasts::default(), startup: StartupManager::new() }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::LIGHTNING).size(26.0).color(theme::orange()));
            ui.vertical(|ui| {
                ui.label(RichText::new("Wield").font(FontId::new(20.0, theme::heading_family())));
                ui.label(RichText::new("Windows toolkit").small().color(theme::text_dim()));
            });
        });
        ui.add_space(18.0);

        if nav_item(ui, icon::SQUARES_FOUR, "Home", self.view == View::Home, true) {
            self.view = View::Home;
        }
        ui.add_space(10.0);
        ui.label(RichText::new("TOOLS").small().color(theme::text_dim()));
        // The tool list scrolls when the window is too short, so it never runs into the footer.
        egui::ScrollArea::vertical().id_salt("nav_tools").max_height((ui.available_height() - 170.0).max(80.0)).auto_shrink([false, true]).show(ui, |ui| {
            for t in TOOLS {
                let active = t.id.is_some_and(|id| self.view == View::Tool(id));
                if nav_item(ui, t.icon, t.title, active, t.id.is_some()) {
                    if let Some(id) = t.id {
                        self.view = View::Tool(id);
                    }
                }
            }
        });

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(self.os.summary()).small().color(theme::text_dim())).wrap());
            if self.elevated {
                ui.label(RichText::new(format!("{} Administrator", icon::SHIELD_CHECK)).small().color(theme::green()));
            } else {
                let r = ui
                    .add(egui::Button::new(RichText::new(format!("{} Restart as admin", icon::LOCK)).small().color(theme::orange())).frame(false))
                    .on_hover_text("Relaunch Wield with administrator rights (UAC prompt)");
                if r.clicked() {
                    match elevation::relaunch_as_admin() {
                        Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.toasts.error(ui.ctx(), e),
                    }
                }
            }
            ui.separator();
            ui.horizontal(|ui| {
                let dark = theme::is_dark();
                let (ic, label) = if dark { (icon::SUN, "Light") } else { (icon::MOON, "Dark") };
                if ui.add(egui::Button::new(RichText::new(format!("{ic}  {label}")).color(theme::text_dim())).frame(false)).clicked() {
                    theme::set_dark(ui.ctx(), !dark);
                    self.settings = crate::settings::Settings::update(|s| s.dark = !dark);
                }
                let active = self.view == View::Settings;
                if ui.add(egui::Button::new(RichText::new(format!("{}  Settings", icon::GEAR)).color(if active { theme::accent() } else { theme::text_dim() })).frame(false)).clicked() {
                    self.view = View::Settings;
                }
            });
            if let Some(v) = self.updater.pending_version() {
                if ui.add(egui::Button::new(RichText::new(format!("{} Update to {v}", icon::SPARKLE)).color(theme::on_accent())).fill(theme::accent())).clicked() {
                    self.view = View::Settings;
                }
            }
        });
    }

    /// Opens a tool by its `--tool` name, on a tab if given.
    fn open_tool(&mut self, tool: &str, tab: Option<&str>) {
        if let Some(v) = view_for(tool) {
            self.view = v;
        }
        if let Some(t) = tab {
            match self.view {
                View::Tool(ToolId::Drivers) => self.drivers.set_tab(t),
                View::Tool(ToolId::Backup) => self.backup.set_tab(t),
                View::Tool(ToolId::Malware) => self.malware.set_tab(t),
                View::Tool(ToolId::Network) => self.network.set_tab(t),
                View::Tool(ToolId::Updates) => self.updates.set_tab(t),
                _ => {}
            }
        }
    }

    fn home(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::same(28)).show(ui, |ui| {
                ui.label(RichText::new("Welcome to Wield").font(FontId::new(28.0, theme::heading_family())));
                ui.label(RichText::new(format!("{}  ·  {}", self.os.summary(), if self.elevated { "Administrator" } else { "Standard user" })).color(theme::text_dim()));
                ui.add_space(18.0);
                if let Some((tool, tab)) = self.dashboard.show(ui, &mut self.toasts) {
                    self.open_tool(tool, tab);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SQUARES_FOUR).size(20.0).color(theme::accent()));
                    ui.label(RichText::new("Tools").font(FontId::new(18.0, theme::heading_family())));
                });
                ui.add_space(8.0);

                let card_w = 260.0;
                let gap = 16.0;
                let cols = (((ui.available_width() + gap) / (card_w + gap)).floor() as usize).max(1);
                let width = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
                for chunk in TOOLS.chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for t in chunk {
                            if self.tool_card(ui, t, width) {
                                if let Some(id) = t.id {
                                    self.view = View::Tool(id);
                                }
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            });
        });
    }

    /// Draws one home-grid card. Returns true when clicked.
    fn tool_card(&self, ui: &mut Ui, t: &ToolInfo, width: f32) -> bool {
        let available = t.id.is_some();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 190.0), if available { Sense::click() } else { Sense::hover() });
        let hovered = resp.hovered() && available;
        let fill = if hovered { theme::card_hover() } else { theme::card() };
        let stroke = if hovered { Stroke::new(1.5, theme::accent()) } else { Stroke::new(1.0, theme::border()) };
        ui.painter().rect(rect, CornerRadius::same(12), fill, stroke, StrokeKind::Inside);

        let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink(18.0)).layout(Layout::top_down(Align::Min)));
        let accent = if available { theme::accent() } else { theme::text_dim() };
        child.horizontal(|ui| {
            egui::Frame::new()
                .fill(accent.gamma_multiply(0.15))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| ui.label(RichText::new(t.icon).size(24.0).color(accent)));
            if !available {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    theme::badge(ui, "Coming soon", theme::text_dim());
                });
            }
        });
        child.add_space(10.0);
        let title_color = if available { theme::text() } else { theme::text_dim() };
        child.label(RichText::new(t.title).font(FontId::new(16.0, theme::heading_family())).color(title_color));
        child.add(egui::Label::new(RichText::new(t.blurb).small().color(theme::text_dim())).wrap());
        let summary = match t.id {
            Some(ToolId::Startup) => self.startup.summary_line(),
            Some(ToolId::Privacy) => self.privacy.summary_line(),
            Some(ToolId::Debloat) => self.debloat.summary_line(),
            Some(ToolId::Network) => self.network.summary_line(),
            Some(ToolId::Malware) => self.malware.summary_line(),
            Some(ToolId::Processes) => self.processes.summary_line(),
            Some(ToolId::Backup) => self.backup.summary_line(),
            Some(ToolId::ContextMenu) => self.ctxmenu.summary_line(),
            Some(ToolId::Apps) => self.apps.summary_line(),
            Some(ToolId::Drivers) => self.drivers.summary_line(),
            Some(ToolId::Hotkeys) => self.hotkeys.summary_line(),
            Some(ToolId::Updates) => self.updates.summary_line(),
            None => None,
        };
        // Live status sits on its own line at the bottom of the card, above a divider.
        if let Some(s) = summary {
            let inner = rect.shrink(18.0);
            let line_y = inner.bottom() - 26.0;
            ui.painter().hline(inner.x_range(), line_y, Stroke::new(1.0, theme::border()));
            let mut foot = ui.new_child(
                UiBuilder::new().max_rect(egui::Rect::from_min_max(egui::pos2(inner.left(), line_y + 8.0), inner.right_bottom())),
            );
            foot.add(egui::Label::new(RichText::new(s).small().color(theme::accent())).truncate());
        }
        if available { resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() } else { false }
    }
}

/// The screen for a `--tool` name (also used by dashboard recommendations).
fn view_for(name: &str) -> Option<View> {
    Some(match name {
        "home" => View::Home,
        "startup" => View::Tool(ToolId::Startup),
        "privacy" => View::Tool(ToolId::Privacy),
        "debloat" => View::Tool(ToolId::Debloat),
        "network" => View::Tool(ToolId::Network),
        "malware" => View::Tool(ToolId::Malware),
        "processes" => View::Tool(ToolId::Processes),
        "backup" => View::Tool(ToolId::Backup),
        "contextmenu" | "ctxmenu" => View::Tool(ToolId::ContextMenu),
        "apps" => View::Tool(ToolId::Apps),
        "drivers" => View::Tool(ToolId::Drivers),
        "shortcuts" | "hotkeys" => View::Tool(ToolId::Hotkeys),
        "updates" => View::Tool(ToolId::Updates),
        "settings" => View::Settings,
        _ => return None,
    })
}

/// Sidebar entry. Returns true when clicked.
fn nav_item(ui: &mut Ui, ic: &str, label: &str, active: bool, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, CornerRadius::same(8), theme::accent().gamma_multiply(0.18));
        painter.rect_filled(egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 8.0), vec2(3.0, 18.0)), CornerRadius::same(2), theme::accent());
    } else if resp.hovered() && enabled {
        painter.rect_filled(rect, CornerRadius::same(8), theme::card_hover());
    }
    let color = if active {
        theme::text()
    } else if enabled {
        theme::text()
    } else {
        theme::text_dim().gamma_multiply(0.7)
    };
    painter.text(rect.left_center() + vec2(14.0, 0.0), egui::Align2::LEFT_CENTER, ic, FontId::proportional(17.0), if active { theme::accent() } else { color });
    painter.text(rect.left_center() + vec2(42.0, 0.0), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), color);
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp.on_hover_text("Coming soon") };
    resp.clicked()
}

impl eframe::App for WieldApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(232.0)
            .frame(egui::Frame::new().fill(theme::sidebar()).inner_margin(Margin::same(14)).stroke(Stroke::new(1.0, theme::border())))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(theme::bg())).show(ui, |ui| match self.view {
            View::Home => self.home(ui),
            View::Tool(ToolId::Startup) => self.startup.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Privacy) => self.privacy.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Debloat) => self.debloat.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Network) => self.network.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Malware) => self.malware.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Processes) => self.processes.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Backup) => self.backup.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::ContextMenu) => self.ctxmenu.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Apps) => self.apps.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Drivers) => self.drivers.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Hotkeys) => self.hotkeys.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Updates) => self.updates.show(ui, self.elevated, &mut self.toasts),
            View::Settings => self.updater.show_settings(ui, &mut self.settings, &mut self.toasts),
        });

        let ctx = ui.ctx().clone();
        self.updater.poll(&ctx, &mut self.toasts);
        if let Some(Intent::Scan(path)) = self.processes.take_intent() {
            self.view = View::Tool(ToolId::Malware);
            self.malware.scan_paths(vec![path], &ctx);
        }
        self.toasts.show(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.updater.on_exit();
    }
}

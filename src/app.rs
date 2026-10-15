//! Application shell: sidebar navigation, home grid and tool routing.

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, vec2};
use egui_phosphor::regular as icon;

use crate::theme::{self, ACCENT, BORDER, GREEN, ORANGE, TEXT_DIM};
use crate::toast::Toasts;
use crate::tools::privacy::ui::PrivacyManager;
use crate::tools::startup::ui::StartupManager;
use crate::win::elevation;
use crate::win::sysver::{OsInfo, os_info};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolId {
    Startup,
    Privacy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Tool(ToolId),
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
    ToolInfo { id: None, icon: icon::PACKAGE, title: "App Debloater", blurb: "Remove preinstalled Store apps you don't use." },
    ToolInfo { id: None, icon: icon::BROOM, title: "Disk Cleanup", blurb: "Find and clear temp files, caches and leftovers." },
    ToolInfo { id: None, icon: icon::CLOUD_ARROW_DOWN, title: "Update Control", blurb: "Pause, schedule and tune Windows Update." },
    ToolInfo { id: None, icon: icon::NETWORK, title: "Network Toolkit", blurb: "DNS, hosts file, adapter reset and diagnostics." },
    ToolInfo { id: None, icon: icon::TRASH, title: "Uninstaller", blurb: "Remove programs completely, including leftovers." },
    ToolInfo { id: None, icon: icon::CPU, title: "System Info", blurb: "Hardware, drivers and Windows edition at a glance." },
];

pub struct WieldApp {
    view: View,
    os: OsInfo,
    elevated: bool,
    toasts: Toasts,
    startup: StartupManager,
    privacy: PrivacyManager,
}

impl WieldApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        // `wield.exe --tool startup` opens a tool directly (handy for shortcuts).
        let args: Vec<String> = std::env::args().collect();
        let view = match args.iter().position(|a| a == "--tool").and_then(|i| args.get(i + 1)).map(String::as_str) {
            Some("startup") => View::Tool(ToolId::Startup),
            Some("privacy") => View::Tool(ToolId::Privacy),
            _ => View::Home,
        };
        let os = os_info();
        Self { view, privacy: PrivacyManager::new(os.clone()), os, elevated: elevation::is_elevated(), toasts: Toasts::default(), startup: StartupManager::new() }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::LIGHTNING).size(26.0).color(ACCENT));
            ui.vertical(|ui| {
                ui.label(RichText::new("Wield").font(FontId::new(20.0, theme::heading_family())));
                ui.label(RichText::new("Windows toolkit").small().color(TEXT_DIM));
            });
        });
        ui.add_space(18.0);

        if nav_item(ui, icon::SQUARES_FOUR, "Home", self.view == View::Home, true) {
            self.view = View::Home;
        }
        ui.add_space(10.0);
        ui.label(RichText::new("TOOLS").small().color(TEXT_DIM));
        for t in TOOLS {
            let active = t.id.is_some_and(|id| self.view == View::Tool(id));
            if nav_item(ui, t.icon, t.title, active, t.id.is_some()) {
                if let Some(id) = t.id {
                    self.view = View::Tool(id);
                }
            }
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(self.os.summary()).small().color(TEXT_DIM)).wrap());
            if self.elevated {
                ui.label(RichText::new(format!("{} Administrator", icon::SHIELD_CHECK)).small().color(GREEN));
            } else {
                let r = ui
                    .add(egui::Button::new(RichText::new(format!("{} Restart as admin", icon::LOCK)).small().color(ORANGE)).frame(false))
                    .on_hover_text("Relaunch Wield with administrator rights (UAC prompt)");
                if r.clicked() {
                    match elevation::relaunch_as_admin() {
                        Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.toasts.error(ui.ctx(), e),
                    }
                }
            }
            ui.separator();
        });
    }

    fn home(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(Margin::same(28)).show(ui, |ui| {
                ui.label(RichText::new("Welcome to Wield").font(FontId::new(28.0, theme::heading_family())));
                ui.label(RichText::new(format!("{}  ·  {}", self.os.summary(), if self.elevated { "Administrator" } else { "Standard user" })).color(TEXT_DIM));
                ui.add_space(22.0);

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
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 150.0), if available { Sense::click() } else { Sense::hover() });
        let hovered = resp.hovered() && available;
        let fill = if hovered { theme::CARD_HOVER } else { theme::CARD };
        let stroke = if hovered { Stroke::new(1.5, ACCENT) } else { Stroke::new(1.0, BORDER) };
        ui.painter().rect(rect, CornerRadius::same(12), fill, stroke, StrokeKind::Inside);

        let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink(18.0)).layout(Layout::top_down(Align::Min)));
        let accent = if available { ACCENT } else { TEXT_DIM };
        child.horizontal(|ui| {
            egui::Frame::new()
                .fill(accent.gamma_multiply(0.15))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(8))
                .show(ui, |ui| ui.label(RichText::new(t.icon).size(24.0).color(accent)));
            if !available {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    theme::badge(ui, "Coming soon", TEXT_DIM);
                });
            }
        });
        child.add_space(10.0);
        let title_color = if available { theme::TEXT } else { TEXT_DIM };
        child.label(RichText::new(t.title).font(FontId::new(16.0, theme::heading_family())).color(title_color));
        child.add(egui::Label::new(RichText::new(t.blurb).small().color(TEXT_DIM)).wrap());
        let summary = match t.id {
            Some(ToolId::Startup) => self.startup.summary_line(),
            Some(ToolId::Privacy) => self.privacy.summary_line(),
            None => None,
        };
        if let Some(s) = summary {
            child.label(RichText::new(s).small().color(ACCENT));
        }
        if available { resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() } else { false }
    }
}

/// Sidebar entry. Returns true when clicked.
fn nav_item(ui: &mut Ui, ic: &str, label: &str, active: bool, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, CornerRadius::same(8), ACCENT.gamma_multiply(0.18));
        painter.rect_filled(egui::Rect::from_min_size(rect.left_top() + vec2(0.0, 8.0), vec2(3.0, 18.0)), CornerRadius::same(2), ACCENT);
    } else if resp.hovered() && enabled {
        painter.rect_filled(rect, CornerRadius::same(8), Color32::from_white_alpha(8));
    }
    let color = if active {
        theme::TEXT
    } else if enabled {
        Color32::from_rgb(0xc3, 0xc8, 0xd2)
    } else {
        TEXT_DIM.gamma_multiply(0.7)
    };
    painter.text(rect.left_center() + vec2(14.0, 0.0), egui::Align2::LEFT_CENTER, ic, FontId::proportional(17.0), if active { ACCENT } else { color });
    painter.text(rect.left_center() + vec2(42.0, 0.0), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), color);
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp.on_hover_text("Coming soon") };
    resp.clicked()
}

impl eframe::App for WieldApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(232.0)
            .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(Margin::same(14)).stroke(Stroke::new(1.0, BORDER)))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::no_frame().frame(egui::Frame::new().fill(theme::BG)).show(ui, |ui| match self.view {
            View::Home => self.home(ui),
            View::Tool(ToolId::Startup) => self.startup.show(ui, self.elevated, &mut self.toasts),
            View::Tool(ToolId::Privacy) => self.privacy.show(ui, self.elevated, &mut self.toasts),
        });

        let ctx = ui.ctx().clone();
        self.toasts.show(&ctx);
    }
}

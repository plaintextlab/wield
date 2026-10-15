//! Gruvbox colors (dark and light), fonts and small reusable widgets.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Response, RichText, Sense, Stroke, StrokeKind,
    TextStyle, Theme, Ui, Vec2, pos2, vec2,
};

use crate::tools::startup::model::{Criticality, Impact};

/// Semantic colors. Values are the official Gruvbox palette (github.com/morhetz/gruvbox).
pub struct Palette {
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub red: Color32,
    pub orange: Color32,
    pub yellow: Color32,
    pub green: Color32,
    pub aqua: Color32,
    pub purple: Color32,
    /// Text drawn on top of a filled accent/red/orange button.
    pub on_accent: Color32,
    pub extreme: Color32,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const GRUVBOX_DARK: Palette = Palette {
    bg: hex(0x282828),         // bg0
    sidebar: hex(0x1d2021),    // bg0_h
    card: hex(0x32302f),       // bg0_s
    card_hover: hex(0x3c3836), // bg1
    border: hex(0x504945),     // bg2
    text: hex(0xebdbb2),       // fg1
    text_dim: hex(0xa89984),   // fg4
    accent: hex(0x83a598),     // blue
    red: hex(0xfb4934),
    orange: hex(0xfe8019),
    yellow: hex(0xfabd2f),
    green: hex(0xb8bb26),
    aqua: hex(0x8ec07c),
    purple: hex(0xd3869b),
    on_accent: hex(0x1d2021),
    extreme: hex(0x1d2021),
};

pub const GRUVBOX_LIGHT: Palette = Palette {
    bg: hex(0xfbf1c7),         // bg0
    sidebar: hex(0xf2e5bc),    // bg0_s
    card: hex(0xf9f5d7),       // bg0_h
    card_hover: hex(0xebdbb2), // bg1
    border: hex(0xd5c4a1),     // bg2
    text: hex(0x3c3836),       // fg1
    text_dim: hex(0x7c6f64),   // fg4
    accent: hex(0x076678),     // blue
    red: hex(0x9d0006),
    orange: hex(0xaf3a03),
    yellow: hex(0xb57614),
    green: hex(0x79740e),
    aqua: hex(0x427b58),
    purple: hex(0x8f3f71),
    on_accent: hex(0xfbf1c7),
    extreme: hex(0xf9f5d7),
};

static DARK: AtomicBool = AtomicBool::new(true);

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

pub fn palette() -> &'static Palette {
    if is_dark() { &GRUVBOX_DARK } else { &GRUVBOX_LIGHT }
}

pub fn bg() -> Color32 {
    palette().bg
}
pub fn sidebar() -> Color32 {
    palette().sidebar
}
pub fn card() -> Color32 {
    palette().card
}
pub fn card_hover() -> Color32 {
    palette().card_hover
}
pub fn border() -> Color32 {
    palette().border
}
pub fn text() -> Color32 {
    palette().text
}
pub fn text_dim() -> Color32 {
    palette().text_dim
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn red() -> Color32 {
    palette().red
}
pub fn orange() -> Color32 {
    palette().orange
}
pub fn yellow() -> Color32 {
    palette().yellow
}
pub fn green() -> Color32 {
    palette().green
}
pub fn aqua() -> Color32 {
    palette().aqua
}
pub fn purple() -> Color32 {
    palette().purple
}
pub fn on_accent() -> Color32 {
    palette().on_accent
}

/// Family for semibold headings (falls back to the regular font when Segoe UI is missing).
pub fn heading_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn crit_color(c: Criticality) -> Color32 {
    match c {
        Criticality::Critical => red(),
        Criticality::Important => orange(),
        Criticality::Optional => green(),
    }
}

pub fn crit_icon(c: Criticality) -> &'static str {
    use egui_phosphor::regular as i;
    match c {
        Criticality::Critical => i::WARNING_OCTAGON,
        Criticality::Important => i::WARNING,
        Criticality::Optional => i::CHECK_CIRCLE,
    }
}

pub fn impact_color(i: Impact) -> Color32 {
    match i {
        Impact::High => red(),
        Impact::Medium => yellow(),
        Impact::Low => green(),
    }
}

/// Loads fonts and applies the saved light/dark preference. Call once at startup.
pub fn install(ctx: &egui::Context, dark: bool) {
    let mut fonts = FontDefinitions::default();
    let fonts_dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
    // Use the native Windows UI font when present; egui's bundled font is the fallback.
    if let Ok(bytes) = std::fs::read(fonts_dir.join("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    if let Ok(bytes) = std::fs::read(fonts_dir.join("seguisb.ttf")) {
        fonts.font_data.insert("segoe-sb".into(), Arc::new(FontData::from_owned(bytes)));
    }
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let mut heading: Vec<String> = Vec::new();
    if fonts.font_data.contains_key("segoe-sb") {
        heading.push("segoe-sb".into());
    }
    heading.extend(fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    fonts.families.insert(heading_family(), heading);
    ctx.set_fonts(fonts);

    style_theme(ctx, Theme::Dark, &GRUVBOX_DARK);
    style_theme(ctx, Theme::Light, &GRUVBOX_LIGHT);
    set_dark(ctx, dark);
}

/// Switches between Gruvbox dark and light.
pub fn set_dark(ctx: &egui::Context, dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
    ctx.set_theme(if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light });
    ctx.request_repaint();
}

fn style_theme(ctx: &egui::Context, theme: Theme, p: &Palette) {
    ctx.style_mut_of(theme, |style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Heading, FontId::new(22.0, heading_family())),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;

        let v = &mut style.visuals;
        v.panel_fill = p.bg;
        v.window_fill = p.card;
        v.window_stroke = Stroke::new(1.0, p.border);
        v.window_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = p.extreme;
        v.faint_bg_color = p.card;
        v.code_bg_color = p.card_hover;
        v.override_text_color = Some(p.text);
        v.hyperlink_color = p.accent;
        v.selection.bg_fill = p.accent.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, p.accent);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(4);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.noninteractive.bg_fill = p.card;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
        v.widgets.inactive.weak_bg_fill = p.card_hover;
        v.widgets.inactive.bg_fill = p.card_hover;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
        v.widgets.hovered.weak_bg_fill = p.border;
        v.widgets.hovered.bg_fill = p.border;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.text_dim);
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
        v.widgets.active.weak_bg_fill = p.border;
        v.widgets.active.bg_fill = p.border;
        v.widgets.active.fg_stroke = Stroke::new(2.0, p.text);
        v.widgets.open.weak_bg_fill = p.card_hover;
        v.window_shadow.color = Color32::from_black_alpha(if theme == Theme::Dark { 90 } else { 30 });
        v.popup_shadow.color = v.window_shadow.color;
    });
}

/// Rounded "pill" label with a tinted background.
pub fn badge(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    // Painted directly so it keeps its natural size inside stretched table cells.
    let galley = ui.painter().layout_no_wrap(text.into(), FontId::proportional(12.0), color);
    let size = galley.size() + vec2(16.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = color.gamma_multiply(if is_dark() { 0.16 } else { 0.12 });
        painter.rect(rect, CornerRadius::same(10), fill, Stroke::new(1.0, color.gamma_multiply(0.5)), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response
}

/// Toggle switch. The "on" color carries meaning (criticality, protection…).
pub fn switch(ui: &mut Ui, on: bool, on_color: Color32, enabled: bool) -> Response {
    let size = vec2(36.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, on);
        let mut bg = lerp_color(border(), on_color, t);
        if !enabled {
            bg = bg.gamma_multiply(0.45);
        }
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(10), bg);
        if response.hovered() && enabled {
            painter.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, text_dim()), StrokeKind::Outside);
        }
        let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        let knob = if enabled { if is_dark() { text() } else { card() } } else { text_dim() };
        painter.circle_filled(pos2(x, rect.center().y), 7.0, knob);
    }
    if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Card-style container used throughout the app.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(card())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
}

/// Primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Button::new(RichText::new(text.into()).color(on_accent())).fill(accent()))
}

/// Button filled with a warning color (used to confirm risky actions).
pub fn danger_button(ui: &mut Ui, text: impl Into<String>, color: Color32, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text.into()).color(on_accent())).fill(color).min_size(Vec2::new(0.0, 30.0)),
    )
}

/// App window icon, drawn by the same code that builds the .exe icon.
pub fn window_icon() -> egui::IconData {
    egui::IconData { rgba: crate::icon_art::render_icon(64), width: 64, height: 64 }
}

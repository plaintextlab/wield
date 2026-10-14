//! Colors, fonts and small reusable widgets.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Response, RichText, Sense, Stroke,
    StrokeKind, TextStyle, Theme, Ui, Vec2, pos2, vec2,
};

use crate::tools::startup::model::{Criticality, Impact};

pub const BG: Color32 = Color32::from_rgb(0x14, 0x16, 0x1b);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x10, 0x12, 0x16);
pub const CARD: Color32 = Color32::from_rgb(0x1c, 0x1f, 0x26);
pub const CARD_HOVER: Color32 = Color32::from_rgb(0x23, 0x27, 0x30);
pub const BORDER: Color32 = Color32::from_rgb(0x2b, 0x30, 0x3a);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xec);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8b, 0x93, 0xa1);
pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
pub const RED: Color32 = Color32::from_rgb(0xef, 0x4f, 0x55);
pub const ORANGE: Color32 = Color32::from_rgb(0xf2, 0x9d, 0x2a);
pub const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc5, 0x4a);
pub const GREEN: Color32 = Color32::from_rgb(0x3c, 0xb3, 0x71);

/// Family for semibold headings (falls back to the regular font when Segoe UI is missing).
pub fn heading_family() -> FontFamily {
    FontFamily::Name("semibold".into())
}

pub fn crit_color(c: Criticality) -> Color32 {
    match c {
        Criticality::Critical => RED,
        Criticality::Important => ORANGE,
        Criticality::Optional => GREEN,
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
        Impact::High => RED,
        Impact::Medium => YELLOW,
        Impact::Low => GREEN,
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fonts_dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts");
    // Use the native Windows UI font when present; egui's bundled font is the fallback.
    if let Ok(bytes) = std::fs::read(fonts_dir.join("segoeui.ttf")) {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    let semibold = std::fs::read(fonts_dir.join("seguisb.ttf")).ok();
    if let Some(bytes) = semibold {
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

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(Theme::Dark, |style| {
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
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = Color32::from_rgb(0x0e, 0x10, 0x14);
        v.faint_bg_color = Color32::from_rgb(0x19, 0x1c, 0x22);
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = ACCENT;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(6);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x25, 0x29, 0x32);
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x25, 0x29, 0x32);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2e, 0x33, 0x3e);
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2e, 0x33, 0x3e);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(0x36, 0x3c, 0x48);
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
        painter.rect(rect, CornerRadius::same(10), color.gamma_multiply(0.16), Stroke::new(1.0, color.gamma_multiply(0.45)), StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    response
}

/// iOS-style switch. The "on" color reflects the entry's criticality.
pub fn switch(ui: &mut Ui, on: bool, on_color: Color32, enabled: bool) -> Response {
    let size = vec2(36.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, on);
        let off_color = Color32::from_rgb(0x3a, 0x3f, 0x4b);
        let mut bg = lerp_color(off_color, on_color, t);
        if !enabled {
            bg = bg.gamma_multiply(0.45);
        }
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(10), bg);
        if response.hovered() && enabled {
            painter.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.4)), StrokeKind::Outside);
        }
        let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        let knob = if enabled { Color32::WHITE } else { Color32::from_gray(150) };
        painter.circle_filled(pos2(x, rect.center().y), 7.0, knob);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Card-style container used throughout the app.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(14))
}

/// Primary (accent-filled) button.
pub fn primary_button(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.add(egui::Button::new(RichText::new(text.into()).color(Color32::WHITE)).fill(ACCENT))
}

/// Button filled with a warning color (used to confirm risky actions).
pub fn danger_button(ui: &mut Ui, text: impl Into<String>, color: Color32, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text.into()).color(Color32::WHITE)).fill(color).min_size(Vec2::new(0.0, 30.0)),
    )
}

/// App window icon: a rounded gradient tile with a lightning bolt.
pub fn window_icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    let bolt = [(36.0, 6.0), (16.0, 36.0), (30.0, 36.0), (26.0, 58.0), (48.0, 26.0), (34.0, 26.0), (36.0, 6.0)];
    let inside_bolt = |x: f32, y: f32| {
        // Even-odd ray casting.
        let mut inside = false;
        for w in bolt.windows(2) {
            let ((x1, y1), (x2, y2)) = (w[0], w[1]);
            if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
                inside = !inside;
            }
        }
        inside
    };
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded square mask, radius 14.
            let r = 14.0;
            let cx = fx.clamp(r, n as f32 - r);
            let cy = fy.clamp(r, n as f32 - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            let alpha = (r + 0.5 - d).clamp(0.0, 1.0);
            let t = (fx + fy) / (2.0 * n as f32);
            let (mut red, mut green, mut blue) = (
                (0x4c as f32 + (0x8a - 0x4c) as f32 * t) as u8,
                (0x8d as f32 + (0x5c - 0x8d) as f32 * t) as u8,
                (0xff as f32 + (0xf6 - 0xff) as f32 * t) as u8,
            );
            if inside_bolt(fx, fy) {
                (red, green, blue) = (255, 255, 255);
            }
            let i = (y * n + x) * 4;
            rgba[i..i + 4].copy_from_slice(&[red, green, blue, (alpha * 255.0) as u8]);
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}

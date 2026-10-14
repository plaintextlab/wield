//! Short-lived notifications shown in the bottom-right corner.

use eframe::egui::{self, Align2, Color32, CornerRadius, Margin, RichText, Stroke, vec2};

use crate::theme;

pub struct Toast {
    text: String,
    color: Color32,
    expires: f64,
}

#[derive(Default)]
pub struct Toasts {
    list: Vec<Toast>,
}

impl Toasts {
    fn push(&mut self, ctx: &egui::Context, text: impl Into<String>, color: Color32, secs: f64) {
        let now = ctx.input(|i| i.time);
        self.list.push(Toast { text: text.into(), color, expires: now + secs });
        ctx.request_repaint();
    }
    pub fn success(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.push(ctx, text, theme::GREEN, 4.0);
    }
    pub fn info(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.push(ctx, text, theme::ACCENT, 4.0);
    }
    pub fn error(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        self.push(ctx, text, theme::RED, 8.0);
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.list.retain(|t| t.expires > now);
        if self.list.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for t in &self.list {
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .stroke(Stroke::new(1.0, t.color.gamma_multiply(0.7)))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").color(t.color));
                                ui.add(egui::Label::new(RichText::new(&t.text).color(theme::TEXT)).wrap());
                            });
                        });
                    ui.add_space(6.0);
                }
            });
        // Keep repainting so toasts disappear on time.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

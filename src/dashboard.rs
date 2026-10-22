//! The "PC health" section at the top of Home: live gauges and recommendations.

use std::collections::HashSet;
use std::sync::mpsc::Receiver;

use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Label, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use egui_phosphor::regular as icon;

use crate::health::{Action, Health, Level, Monitor};
use crate::theme;
use crate::toast::Toasts;
use crate::win::fileinfo::format_bytes;

pub struct Dashboard {
    pub monitor: Monitor,
    dismissed: HashSet<&'static str>,
    confirm_clean: bool,
    clean_rx: Option<Receiver<(u64, usize, usize)>>,
}

impl Dashboard {
    pub fn new() -> Self {
        Self { monitor: Monitor::new(), dismissed: HashSet::new(), confirm_clean: false, clean_rx: None }
    }

    /// Draws the section. Returns a tool (and tab) to open when a recommendation's button is clicked.
    pub fn show(&mut self, ui: &mut Ui, toasts: &mut Toasts) -> Option<(&'static str, Option<&'static str>)> {
        let ctx = ui.ctx().clone();
        self.monitor.start(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        if let Some(rx) = &self.clean_rx {
            if let Ok((freed, n, skipped)) = rx.try_recv() {
                self.clean_rx = None;
                toasts.success(&ctx, format!("Freed {} ({n} files).{}", format_bytes(freed), if skipped > 0 { format!(" {skipped} in use were left alone.") } else { String::new() }));
                self.monitor.recheck();
            }
        }
        let h = self.monitor.snapshot();
        let mut open = None;

        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::HEARTBEAT).size(20.0).color(theme::accent()));
            ui.label(RichText::new("PC health").font(FontId::new(18.0, theme::heading_family())));
            ui.label(RichText::new("live").small().color(theme::green()));
        });
        ui.add_space(8.0);
        if h.mem_total == 0 {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Measuring…").color(theme::text_dim()));
            });
            ui.add_space(12.0);
            return None;
        }
        self.gauges(ui, &h);
        ui.add_space(14.0);
        open = open.or(self.recommendations(ui, &h));
        self.clean_dialog(&ctx);
        open
    }

    fn gauges(&self, ui: &mut Ui, h: &Health) {
        let gap = 12.0;
        let min_w = 230.0;
        let cols = (((ui.available_width() + gap) / (min_w + gap)).floor() as usize).clamp(1, 6);
        let w = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).floor();
        let mut cards: Vec<Box<dyn Fn(&mut Ui)>> = Vec::new();

        // CPU
        let (cpu, hist) = (h.cpu, h.cpu_hist.clone());
        let temp = h.temp;
        let stale = h.temp_stale();
        // Until a few readings are in, staleness is unknown: show it neutrally.
        let settled = h.temp_hist.len() >= 6;
        let cpu_name = h.cpu_name.clone();
        let cores = h.cores;
        cards.push(Box::new(move |ui| {
            head(ui, icon::CPU, "Processor");
            big(ui, &format!("{cpu:.0}%"), load_color(cpu));
            spark(ui, &hist, 100.0, load_color(cpu));
            ui.horizontal(|ui| {
                small(ui, &format!("{cores} threads"), None);
                if let Some(t) = temp {
                    let (text, c) = if stale { (format!("· board sensor {t:.0} °C (not updating)"), theme::text_dim()) } else { (format!("· board sensor {t:.0} °C"), if settled { temp_color(t) } else { theme::text_dim() }) };
                    ui.add(Label::new(RichText::new(text).small().color(c)).truncate()).on_hover_text(
                        "From the motherboard's ACPI thermal zone, the only temperature Windows shares without a driver. It's near the processor but not the exact core temperature, and many laptops only update it occasionally.",
                    );
                }
            });
            ui.add(Label::new(RichText::new(&cpu_name).small().color(theme::text_dim())).truncate());
        }));
        // Memory
        let (used, total, mhist) = (h.mem_used, h.mem_total, h.mem_hist.clone());
        cards.push(Box::new(move |ui| {
            let pct = used as f32 / total.max(1) as f32 * 100.0;
            head(ui, icon::MEMORY, "Memory");
            big(ui, &format!("{} free", format_bytes(total.saturating_sub(used))), load_color(pct));
            spark(ui, &mhist, 100.0, load_color(pct));
            small(ui, &format!("{} of {} in use ({pct:.0}%)", format_bytes(used), format_bytes(total)), None);
        }));
        // Disks
        for d in h.disks.clone() {
            cards.push(Box::new(move |ui| {
                let free_pct = d.free as f32 / d.total.max(1) as f32 * 100.0;
                let c = if free_pct < 5.0 { theme::red() } else if free_pct < 12.0 { theme::yellow() } else { theme::green() };
                head(ui, icon::HARD_DRIVES, &if d.label.is_empty() { format!("Drive {}", d.letter) } else { format!("{} {}", d.letter, d.label) });
                big(ui, &format!("{} free", format_bytes(d.free)), c);
                bar(ui, 1.0 - free_pct / 100.0, c);
                small(ui, &format!("{:.0}% free of {}", free_pct, format_bytes(d.total)), None);
            }));
        }
        // GPUs (NVIDIA reports temperature and load)
        for g in h.gpus.clone() {
            cards.push(Box::new(move |ui| {
                head(ui, icon::GRAPHICS_CARD, "Graphics");
                let load = g.load.unwrap_or(0.0);
                big(ui, &g.temp.map(|t| format!("{t:.0} °C")).unwrap_or_else(|| format!("{load:.0}%")), g.temp.map(temp_color).unwrap_or(theme::text()));
                bar(ui, load / 100.0, load_color(load));
                let mem = match (g.mem_used_mb, g.mem_total_mb) {
                    (Some(u), Some(t)) => format!(" · VRAM {:.1}/{:.0} GB", u as f32 / 1024.0, t as f32 / 1024.0),
                    _ => String::new(),
                };
                small(ui, &format!("{load:.0}% load{mem}"), None);
                ui.add(Label::new(RichText::new(&g.name).small().color(theme::text_dim())).truncate());
            }));
        }
        // Battery / uptime
        let (battery, uptime, procs) = (h.battery, h.uptime, h.processes);
        cards.push(Box::new(move |ui| {
            if let Some((pct, charging)) = battery {
                head(ui, if charging { icon::BATTERY_CHARGING } else { icon::BATTERY_HIGH }, "Battery");
                big(ui, &format!("{pct}%"), if pct < 20 && !charging { theme::red() } else { theme::green() });
                bar(ui, pct as f32 / 100.0, if pct < 20 { theme::red() } else { theme::green() });
                small(ui, if charging { "Plugged in" } else { "On battery" }, None);
            } else {
                head(ui, icon::CLOCK, "Uptime");
            }
            let days = uptime / 86_400;
            let text = if days > 0 { format!("Up {days} d {} h", (uptime % 86_400) / 3600) } else { format!("Up {} h {} min", uptime / 3600, (uptime % 3600) / 60) };
            if battery.is_none() {
                big(ui, &text, if days >= 7 { theme::yellow() } else { theme::text() });
                small(ui, &format!("{procs} processes running"), None);
            } else {
                small(ui, &format!("{text} · {procs} processes"), if days >= 7 { Some(theme::yellow()) } else { None });
            }
        }));

        for row in cards.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for draw in row {
                    egui::Frame::new()
                        .fill(theme::card())
                        .stroke(Stroke::new(1.0, theme::border()))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::same(12))
                        .show(ui, |ui| {
                            // Frame width = content + 2×12 margin + 2×1 stroke.
                            ui.set_width(w - 26.0);
                            ui.set_max_width(w - 26.0);
                            ui.set_min_height(118.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                draw(ui);
                            });
                        });
                }
            });
            ui.add_space(gap);
        }
    }

    fn recommendations(&mut self, ui: &mut Ui, h: &Health) -> Option<(&'static str, Option<&'static str>)> {
        let mut open = None;
        let recs: Vec<_> = h.recs.iter().filter(|r| !self.dismissed.contains(r.id)).cloned().collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::LIGHTBULB).size(18.0).color(theme::accent()));
            ui.label(RichText::new("Recommendations").font(FontId::new(16.0, theme::heading_family())));
        });
        ui.add_space(6.0);
        theme::card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !h.checked {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Checking startup apps, devices, protection and temporary files…").color(theme::text_dim()));
                });
                return;
            }
            if recs.is_empty() {
                ui.label(RichText::new(format!("{} Your PC looks healthy. Nothing to do right now.", icon::CHECK_CIRCLE)).color(theme::green()));
                return;
            }
            for (i, r) in recs.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                let (ic, c) = match r.level {
                    Level::Critical => (icon::WARNING_CIRCLE, theme::red()),
                    Level::Warn => (icon::WARNING, theme::yellow()),
                    Level::Tip => (icon::LIGHTBULB, theme::aqua()),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(ic).size(20.0).color(c));
                    let reserve = 260.0;
                    ui.scope(|ui| {
                        ui.set_max_width((ui.available_width() - reserve).max(200.0));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&r.title).strong());
                            ui.add(Label::new(RichText::new(&r.detail).small().color(theme::text_dim())).wrap());
                        });
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(icon::X).on_hover_text("Hide until Wield restarts").clicked() {
                            self.dismissed.insert(r.id);
                        }
                        if let Some((label, action)) = &r.action {
                            let busy = matches!(action, Action::CleanTemp) && self.clean_rx.is_some();
                            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                match action {
                                    Action::Tool(t, tab) => open = Some((*t, *tab)),
                                    Action::Open(uri) => crate::win::shell::open(uri),
                                    Action::CleanTemp => self.confirm_clean = true,
                                }
                            }
                            if busy {
                                ui.spinner();
                            }
                        }
                    });
                });
            }
        });
        open
    }

    fn clean_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_clean {
            return;
        }
        let mut go = false;
        let mut cancel = false;
        let m = egui::Modal::new(egui::Id::new("clean_temp")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::BROOM).size(28.0).color(theme::accent()));
                ui.label(RichText::new("Clean temporary files?").font(FontId::new(18.0, theme::heading_family())));
            });
            ui.add_space(6.0);
            ui.add(Label::new(format!("Deletes files in your Temp folder ({}) that haven't changed for more than a day. Files programs are using right now are skipped, and nothing outside the Temp folder is touched. Deleted files don't go to the Recycle Bin.", std::env::temp_dir().display())).wrap());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "Clean").clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if go {
            let (tx, rx) = std::sync::mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::health::clean_temp());
                c.request_repaint();
            });
            self.clean_rx = Some(rx);
        }
        if go || cancel || m.should_close() {
            self.confirm_clean = false;
        }
    }
}

fn head(ui: &mut Ui, ic: &str, title: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic).color(theme::accent()));
        ui.add(Label::new(RichText::new(title).small().color(theme::text_dim())).truncate());
    });
}

fn big(ui: &mut Ui, text: &str, c: Color32) {
    ui.add(Label::new(RichText::new(text).font(FontId::new(22.0, theme::heading_family())).color(c)).truncate());
}

fn small(ui: &mut Ui, text: &str, c: Option<Color32>) {
    ui.add(Label::new(RichText::new(text).small().color(c.unwrap_or(theme::text_dim()))).truncate());
}

fn bar(ui: &mut Ui, frac: f32, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(4), theme::border());
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    p.rect_filled(fill, CornerRadius::same(4), c);
}

/// A small line chart of recent values.
fn spark(ui: &mut Ui, values: &std::collections::VecDeque<f32>, max: f32, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(3), theme::bg(), Stroke::NONE, StrokeKind::Inside);
    let n = 60usize;
    let pts: Vec<_> = values.iter().rev().take(n).rev().copied().collect();
    if pts.len() < 2 {
        return;
    }
    let step = rect.width() / (n - 1) as f32;
    let x0 = rect.right() - step * (pts.len() - 1) as f32;
    let line: Vec<_> = pts.iter().enumerate().map(|(i, v)| pos2(x0 + i as f32 * step, rect.bottom() - (v / max).clamp(0.0, 1.0) * (rect.height() - 2.0) - 1.0)).collect();
    p.add(egui::Shape::line(line, Stroke::new(1.5, c)));
}

fn load_color(pct: f32) -> Color32 {
    if pct >= 90.0 {
        theme::red()
    } else if pct >= 75.0 {
        theme::yellow()
    } else {
        theme::green()
    }
}

fn temp_color(t: f32) -> Color32 {
    if t >= 90.0 {
        theme::red()
    } else if t >= 80.0 {
        theme::yellow()
    } else {
        theme::text()
    }
}

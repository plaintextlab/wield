// Release builds are GUI-only; debug builds keep a console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod icon_art;
mod theme;
mod toast;
mod tools;
mod win;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Wield — Windows toolkit")
            .with_inner_size([1400.0, 820.0])
            .with_min_inner_size([980.0, 600.0])
            .with_icon(theme::window_icon()),
        ..Default::default()
    };
    eframe::run_native("Wield", options, Box::new(|cc| Ok(Box::new(app::WieldApp::new(cc)))))
}

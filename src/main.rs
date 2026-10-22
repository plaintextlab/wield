// Release builds are GUI-only; debug builds keep a console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dashboard;
mod health;
mod icon_art;
mod settings;
mod settings_ui;
mod theme;
mod toast;
mod tools;
mod update;
mod win;

fn main() -> eframe::Result<()> {
    // Context menu entries set to "Run hidden" start their command through Wield without a window.
    if tools::ctxmenu::custom::run_hidden_main() {
        return Ok(());
    }
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

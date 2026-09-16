// Unter Windows im Release kein Konsolenfenster öffnen.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod beep;
mod level;
mod settings;
mod updater;

use eframe::egui;

fn main() -> eframe::Result {
    let settings = settings::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Lärmampel")
        .with_inner_size(app::COMPACT_SIZE)
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false);
    if settings.always_on_top {
        viewport = viewport.with_always_on_top();
    }
    if let Some([x, y]) = settings.window_pos {
        viewport = viewport.with_position([x, y]);
    }

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Lärmampel",
        options,
        Box::new(|cc| Ok(Box::new(app::LaermampelApp::new(settings, &cc.egui_ctx)))),
    )
}

// Unter Windows im Release kein Konsolenfenster öffnen.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod autostart;
mod beep;
mod instance;
mod level;
mod placement;
mod settings;
mod tray;
mod updater;

use std::time::Duration;

use eframe::egui;

fn main() -> eframe::Result {
    // Nach einem Update wartet die neue Version, bis die alte beendet ist.
    updater::remember_exe_path();
    let after_update = std::env::args().any(|a| a == updater::RESTART_ARG);
    let wait = if after_update { Duration::from_secs(30) } else { Duration::ZERO };
    let Some(instance) = instance::acquire(wait) else {
        return Ok(());
    };

    let settings = settings::load();

    let viewport = egui::ViewportBuilder::default()
        .with_title("Lärmampel")
        .with_inner_size([settings.dot_size, settings.dot_size])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_taskbar(false)
        .with_active(false);

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "Lärmampel",
        options,
        Box::new(move |cc| Ok(Box::new(app::LaermampelApp::new(settings, instance, &cc.egui_ctx)))),
    )
}

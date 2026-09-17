// Unter Windows im Release kein Konsolenfenster öffnen.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Zuerst, damit das log!-Makro in allen anderen Modulen verfügbar ist.
#[macro_use]
mod log;
mod agc;
mod apo_link;
mod apo_setup;
mod app;
mod audio;
mod autostart;
mod beep;
mod instance;
mod level;
mod placement;
mod settings;
mod strip;
mod tray;
mod updater;

use std::time::Duration;

use eframe::egui;

fn main() -> eframe::Result {
    log::init();
    log!("Start v{} {:?}", updater::CURRENT_VERSION, std::env::args().skip(1).collect::<Vec<_>>());
    // Einrichten des Audio-Filters läuft als eigener Aufruf mit Adminrechten, ganz ohne Fenster.
    if let Some(code) = apo_setup::handle_command_line() {
        std::process::exit(code);
    }
    // Nach einem Update wartet die neue Version, bis die alte beendet ist.
    let after_update = std::env::args().any(|a| a == updater::RESTART_ARG);
    let wait = if after_update { Duration::from_secs(30) } else { Duration::ZERO };
    let Some(instance) = instance::acquire(wait) else {
        log!("Läuft schon, dort Einstellungen angefordert");
        return Ok(());
    };

    let settings = settings::load();

    let viewport = egui::ViewportBuilder::default()
        .with_title("Lärmampel")
        .with_icon(app::app_icon())
        .with_inner_size([settings.dot_size, settings.dot_size])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_taskbar(false)
        .with_active(false);

    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let result = eframe::run_native(
        "Lärmampel",
        options,
        Box::new(move |cc| Ok(Box::new(app::LaermampelApp::new(settings, instance, &cc.egui_ctx)))),
    );
    if let Err(e) = &result {
        log!("Fenster-System beendet mit Fehler: {e}");
    }
    log!("Ende");
    result
}

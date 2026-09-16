use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2, ViewportCommand, ViewportId};

use crate::agc::{self, AgcParams};
use crate::audio::{self, AgcSetup, InputDevice, Meter};
use crate::autostart;
use crate::beep;
use crate::instance;
use crate::level::{Calibration, Level, Zone};
use crate::placement::{self, Anchor, Monitor, PhysicalRect};
use crate::settings::{self, DisplayMode, Settings};
use crate::tray::{Tray, TrayAction};
use crate::updater::{self, Status, Updater};

/// Anzeigebereich des Pegelbalkens in dBFS.
const BAR_MIN_DB: f32 = -60.0;
const BAR_MAX_DB: f32 = 0.0;
const BAR_HEIGHT: f32 = 28.0;

const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const MONITOR_SCAN_INTERVAL: Duration = Duration::from_secs(2);
/// So oft wird die Anzeige wieder nach ganz vorne geholt, falls ein Spiel sie verdeckt hat.
const REASSERT_INTERVAL: Duration = Duration::from_secs(1);

const PREVIEW_DURATION: Duration = Duration::from_secs(3);

const RED_TEXT: Color32 = Color32::from_rgb(235, 90, 90);

/// Ampel-Icon für Fenster, erzeugt von scripts/make_icon.py.
pub fn app_icon() -> egui::IconData {
    egui::IconData { rgba: include_bytes!("../assets/icon-64.rgba").to_vec(), width: 64, height: 64 }
}

pub fn zone_rgb(zone: Zone) -> [u8; 3] {
    match zone {
        Zone::Green => [40, 200, 90],
        Zone::Yellow => [245, 190, 20],
        Zone::Red => [235, 45, 45],
    }
}

fn zone_color(zone: Zone) -> Color32 {
    let [r, g, b] = zone_rgb(zone);
    Color32::from_rgb(r, g, b)
}

pub struct LaermampelApp {
    settings: Settings,
    saved: Settings,
    last_save: Instant,

    meter: Option<Meter>,
    meter_error: Option<String>,
    last_retry: Instant,
    devices: Vec<InputDevice>,
    output_devices: Vec<InputDevice>,
    agc_params: Arc<AgcParams>,

    level: Level,
    last_tick: Instant,
    red_count: u32,

    calibration: Option<Calibration>,
    calibration_message: Option<String>,

    monitors: Vec<Monitor>,
    last_monitor_scan: Instant,
    applied_rect: Option<PhysicalRect>,
    last_reassert: Instant,

    settings_open: bool,
    focus_settings: bool,
    preview_until: Option<Instant>,
    autostart_enabled: bool,
    autostart_error: Option<String>,

    tray: Option<Tray>,
    instance: instance::Guard,
    updater: Updater,
}

impl LaermampelApp {
    pub fn new(settings: Settings, instance: instance::Guard, ctx: &egui::Context) -> Self {
        let tray = Tray::new();
        let mut app = Self {
            saved: settings.clone(),
            settings,
            last_save: Instant::now(),
            meter: None,
            meter_error: None,
            last_retry: Instant::now(),
            devices: audio::list_input_devices(),
            output_devices: agc::list_output_devices(),
            agc_params: Arc::new(AgcParams::default()),
            level: Level::new(),
            last_tick: Instant::now(),
            red_count: 0,
            calibration: None,
            calibration_message: None,
            monitors: placement::monitors(),
            last_monitor_scan: Instant::now(),
            applied_rect: None,
            last_reassert: Instant::now(),
            // Ohne Symbol im Infobereich kämen wir sonst nie an die Einstellungen.
            settings_open: tray.is_none(),
            focus_settings: false,
            preview_until: None,
            autostart_enabled: autostart::is_enabled(),
            autostart_error: None,
            tray,
            instance,
            updater: Updater::new(),
        };
        app.sync_agc_params();
        app.restart_meter();
        app.updater.check(ctx);
        app
    }

    fn restart_meter(&mut self) {
        self.meter = None;
        self.last_retry = Instant::now();
        let agc_setup = self.settings.agc_enabled.then(|| AgcSetup {
            output_id: self.settings.agc_output_id.clone(),
            params: Arc::clone(&self.agc_params),
        });
        match Meter::start(self.settings.device_id.as_deref(), agc_setup) {
            Ok(m) => {
                self.meter = Some(m);
                self.meter_error = None;
            }
            Err(e) => self.meter_error = Some(e),
        }
    }

    fn sync_agc_params(&self) {
        let (s, p) = (&self.settings, &self.agc_params);
        p.target_db.set(s.agc_target_db);
        p.max_gain_db.set(s.agc_max_gain_db);
        p.max_cut_db.set(s.agc_max_cut_db);
        p.attack_ms.set(s.agc_attack_ms);
        p.release_ms.set(s.agc_release_ms);
        p.gate_db.set(s.agc_gate_db);
        p.ceiling_db.set(s.agc_ceiling_db);
    }

    /// Angezeigte Farbe; während der Vorschau immer Rot.
    fn shown_zone(&self) -> Zone {
        if self.preview_until.is_some_and(|t| Instant::now() < t) {
            Zone::Red
        } else {
            self.level.zone
        }
    }

    fn zone_brightness(&self, zone: Zone) -> f32 {
        match zone {
            Zone::Green => self.settings.green_brightness,
            _ => self.settings.brightness,
        }
    }

    fn indicator_size(&self) -> [f32; 2] {
        match self.settings.display {
            DisplayMode::Dot => [self.settings.dot_size, self.settings.dot_size],
            DisplayMode::Bar => [self.settings.bar_width, BAR_HEIGHT],
        }
    }

    fn open_settings(&mut self) {
        self.settings_open = true;
        self.focus_settings = true;
        self.devices = audio::list_input_devices();
        self.output_devices = agc::list_output_devices();
        self.monitors = placement::monitors();
    }

    fn close_settings(&mut self, ctx: &egui::Context) {
        self.settings_open = false;
        if self.tray.is_none() {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }
    }

    fn apply_calibration(&mut self, normal_db: f32) {
        let s = &mut self.settings;
        s.yellow_db = (normal_db + s.yellow_offset_db).min(BAR_MAX_DB);
        s.red_db = (normal_db + s.red_offset_db).min(BAR_MAX_DB);
        self.calibration_message = Some(format!(
            "Normale Stimme: {normal_db:.1} dB → Gelb ab {:.1}, Rot ab {:.1}",
            s.yellow_db, s.red_db
        ));
    }

    fn update_placement(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.last_monitor_scan.elapsed() >= MONITOR_SCAN_INTERVAL {
            self.monitors = placement::monitors();
            self.last_monitor_scan = Instant::now();
        }
        let target = placement::target_rect(
            &self.monitors,
            self.settings.monitor,
            self.settings.anchor,
            self.indicator_size(),
            self.settings.margin,
        );
        let Some(target) = target else { return };
        if Some(target) != self.applied_rect || self.last_reassert.elapsed() >= REASSERT_INTERVAL {
            placement::apply(frame, target, ctx);
            self.applied_rect = Some(target);
            self.last_reassert = Instant::now();
        }
    }

    fn draw_level_bar(&self, painter: &egui::Painter, rect: Rect) {
        let to_x = |db: f32| {
            let t = ((db - BAR_MIN_DB) / (BAR_MAX_DB - BAR_MIN_DB)).clamp(0.0, 1.0);
            rect.left() + t * rect.width()
        };

        painter.rect_filled(rect, CornerRadius::same(4), Color32::from_black_alpha(140));
        let fill = Rect::from_min_max(rect.min, Pos2::new(to_x(self.level.display_db), rect.max.y));
        painter.rect_filled(fill, CornerRadius::same(4), zone_color(self.level.zone));

        for (db, zone) in [(self.settings.yellow_db, Zone::Yellow), (self.settings.red_db, Zone::Red)] {
            let x = to_x(db);
            painter.line_segment(
                [Pos2::new(x, rect.top() - 2.0), Pos2::new(x, rect.bottom() + 2.0)],
                Stroke::new(2.0, zone_color(zone)),
            );
        }
    }

    fn draw_indicator(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter();
        let zone = self.shown_zone();
        let brightness = self.zone_brightness(zone);
        let tint = zone_color(zone).gamma_multiply(brightness);

        match self.settings.display {
            DisplayMode::Dot => {
                if brightness <= 0.01 {
                    return;
                }
                let radius = rect.width().min(rect.height()) / 2.0 - 1.0;
                // Dunkler Rand, damit der Punkt auch vor hellem Hintergrund zu sehen ist.
                let outline = Color32::from_black_alpha((160.0 * brightness) as u8);
                painter.circle(rect.center(), radius, tint, Stroke::new(1.5, outline));
            }
            DisplayMode::Bar => {
                painter.rect_filled(rect, CornerRadius::same(8), tint);
                self.draw_level_bar(painter, rect.shrink(7.0));
            }
        }
    }

    fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let id = ViewportId::from_hash_of("einstellungen");
        let builder = egui::ViewportBuilder::default()
            .with_title("Lärmampel – Einstellungen")
            .with_icon(app_icon())
            .with_inner_size([400.0, 760.0])
            .with_min_inner_size([340.0, 300.0]);

        ctx.show_viewport_immediate(id, builder, |ui, _class| {
            egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.settings_ui(ui));
            });
            if ui.input(|i| i.viewport().close_requested()) {
                self.close_settings(ui.ctx());
            }
        });

        if self.focus_settings {
            ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
            self.focus_settings = false;
        }
    }

    fn version_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Version");
        ui.label(format!("Lärmampel v{}", updater::CURRENT_VERSION));
        let ctx = ui.ctx().clone();
        match self.updater.status() {
            Status::Idle => {
                if ui.button("Nach Updates suchen").clicked() {
                    self.updater.check(&ctx);
                }
            }
            Status::UpToDate => {
                ui.label("Du hast die neueste Version.");
                if ui.button("Nach Updates suchen").clicked() {
                    self.updater.check(&ctx);
                }
            }
            Status::Failed(e) => {
                ui.colored_label(RED_TEXT, e);
                if ui.button("Nochmal versuchen").clicked() {
                    self.updater.check(&ctx);
                }
            }
            Status::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Suche nach Updates …");
                });
            }
            Status::Available(release) => {
                ui.colored_label(Color32::from_rgb(90, 200, 120), format!("Neue Version v{} verfügbar", release.version));
                ui.horizontal(|ui| {
                    if release.installable() && ui.button("⬆ Update installieren").clicked() {
                        self.updater.install(&ctx, release.clone());
                    }
                    ui.hyperlink_to("Was ist neu?", &release.page_url);
                });
            }
            Status::Installing(release) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Lade v{} herunter …", release.version));
                });
            }
            Status::Installed(release) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Installiere v{} …", release.version));
                });
            }
        }
    }

    /// Der Installer läuft: Platz machen, damit er die Dateien ersetzen kann.
    fn exit_for_update(&mut self) {
        settings::save(&self.settings);
        self.saved = self.settings.clone();
        // Symbol im Infobereich sauber entfernen, sonst bleibt ein Geist-Symbol stehen.
        self.tray = None;
        std::process::exit(0);
    }

    fn display_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Anzeige");

        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut s.display, DisplayMode::Dot, "● Punkt");
            ui.selectable_value(&mut s.display, DisplayMode::Bar, "▬ Leiste mit Pegel");
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Monitor");
            let selected = match self.monitors.get(s.monitor) {
                Some(m) => m.label(s.monitor),
                None => format!("Monitor {} (nicht angeschlossen)", s.monitor + 1),
            };
            egui::ComboBox::from_id_salt("monitor").selected_text(selected).width(240.0).show_ui(ui, |ui| {
                for (i, m) in self.monitors.iter().enumerate() {
                    ui.selectable_value(&mut s.monitor, i, m.label(i));
                }
            });
        });

        ui.add_space(4.0);
        ui.label("Position");
        egui::Grid::new("anchor").num_columns(3).show(ui, |ui| {
            for row in [Anchor::TOP_ROW, Anchor::BOTTOM_ROW] {
                for anchor in row {
                    ui.selectable_value(&mut s.anchor, anchor, anchor.label());
                }
                ui.end_row();
            }
        });

        ui.add_space(4.0);
        match s.display {
            DisplayMode::Dot => ui.add(egui::Slider::new(&mut s.dot_size, 6.0..=80.0).text("Größe").suffix(" px")),
            DisplayMode::Bar => ui.add(egui::Slider::new(&mut s.bar_width, 100.0..=800.0).text("Breite").suffix(" px")),
        };
        ui.add(egui::Slider::new(&mut s.margin, 0.0..=200.0).text("Abstand zum Rand").suffix(" px"));
        ui.add(egui::Slider::new(&mut s.brightness, 0.0..=1.0).text("Helligkeit Gelb/Rot"));
        ui.add(egui::Slider::new(&mut s.green_brightness, 0.0..=1.0).text("Helligkeit Grün"));

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Vorschau: 3 s rot").on_hover_text("Zeigt, wo die Anzeige gerade sitzt").clicked() {
                self.preview_until = Some(Instant::now() + PREVIEW_DURATION);
            }
            match self.applied_rect {
                Some(r) => ui.weak(format!("Position {}, {} · {}×{} px", r.x, r.y, r.w, r.h)),
                None => ui.colored_label(RED_TEXT, "Kein Monitor gefunden"),
            };
        });
    }

    fn microphone_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Mikrofon");
        // Auch ohne laufende Aufnahme anzeigen, was ausgewählt ist.
        let selected = match &self.settings.device_id {
            None => "Standardgerät".to_string(),
            Some(id) => self
                .devices
                .iter()
                .find(|d| &d.id == id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "Gewähltes Mikrofon (nicht gefunden)".to_string()),
        };
        let mut new_device: Option<Option<String>> = None;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("device").selected_text(selected).width(260.0).show_ui(ui, |ui| {
                if ui.selectable_label(self.settings.device_id.is_none(), "Standardgerät").clicked() {
                    new_device = Some(None);
                }
                for d in &self.devices {
                    let active = self.settings.device_id.as_deref() == Some(d.id.as_str());
                    if ui.selectable_label(active, &d.name).clicked() {
                        new_device = Some(Some(d.id.clone()));
                    }
                }
            });
            if ui.button("⟳").on_hover_text("Liste aktualisieren").clicked() {
                self.devices = audio::list_input_devices();
            }
        });
        if let Some(err) = &self.meter_error {
            ui.colored_label(RED_TEXT, err);
        }
        if let Some(device) = new_device {
            self.settings.device_id = device;
            self.restart_meter();
        }

        // Live-Pegel direkt hier, damit man beim Einstellen sieht, was passiert.
        ui.add_space(4.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
        self.draw_level_bar(ui.painter(), rect);
        ui.label(format!("Aktuell: {:.1} dB", self.level.display_db));
    }

    fn agc_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Automatische Lautstärke");
        ui.label("Hebt leise Sprache an und regelt laute runter, für alle Programme. Braucht VB-Cable.");

        let mut restart = false;
        restart |= ui.checkbox(&mut self.settings.agc_enabled, "Mikrofon automatisch angleichen").changed();

        if self.settings.agc_enabled {
            let active = self.meter.as_ref().and_then(|m| m.agc_output_name.clone());
            let error = self.meter.as_ref().and_then(|m| m.agc_error.clone());

            ui.horizontal(|ui| {
                ui.label("Ausgabe");
                let selected = active.clone().unwrap_or_else(|| "–".to_string());
                egui::ComboBox::from_id_salt("agc_output").selected_text(selected).width(240.0).show_ui(ui, |ui| {
                    if ui.selectable_label(self.settings.agc_output_id.is_none(), "VB-Cable automatisch").clicked() {
                        self.settings.agc_output_id = None;
                        restart = true;
                    }
                    for d in &self.output_devices {
                        let chosen = self.settings.agc_output_id.as_deref() == Some(d.id.as_str());
                        if ui.selectable_label(chosen, &d.name).clicked() {
                            self.settings.agc_output_id = Some(d.id.clone());
                            restart = true;
                        }
                    }
                });
                if ui.button("⟳").on_hover_text("Liste aktualisieren").clicked() {
                    self.output_devices = agc::list_output_devices();
                    restart = true;
                }
            });

            if let Some(err) = error {
                ui.colored_label(RED_TEXT, err);
                ui.hyperlink_to("VB-Cable herunterladen", agc::VB_CABLE_URL);
            } else if active.is_some() {
                ui.label("In Discord, Spielen usw. als Mikrofon „CABLE Output“ wählen.");
                let gain = self.agc_params.current_gain_db.get();
                ui.label(format!("Aktuelle Anpassung: {gain:+.1} dB"));
            }

            let s = &mut self.settings;
            ui.add(egui::Slider::new(&mut s.agc_target_db, -40.0..=-6.0).text("Ziellautstärke").suffix(" dB"));
            ui.add(egui::Slider::new(&mut s.agc_max_gain_db, 0.0..=30.0).text("Höchstens lauter").suffix(" dB"));
            ui.add(egui::Slider::new(&mut s.agc_max_cut_db, 0.0..=30.0).text("Höchstens leiser").suffix(" dB"));
            ui.add(egui::Slider::new(&mut s.agc_attack_ms, 5.0..=500.0).text("Runterregeln").suffix(" ms"));
            ui.add(egui::Slider::new(&mut s.agc_release_ms, 100.0..=5000.0).text("Hochregeln").suffix(" ms"));
            ui.add(egui::Slider::new(&mut s.agc_gate_db, -80.0..=-20.0).text("Pause unter").suffix(" dB"))
                .on_hover_text("Leiser als das gilt als Sprechpause, dann wird nichts hochgezogen");
            ui.add(egui::Slider::new(&mut s.agc_ceiling_db, -12.0..=0.0).text("Limiter bei").suffix(" dB"));
        }

        if restart {
            self.restart_meter();
        }
    }

    fn calibration_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Einlernen");
        ui.label("Rede ein paar Sekunden in normaler Lautstärke. Gelb und Rot werden dann relativ dazu gesetzt.");
        if let Some(cal) = &self.calibration {
            ui.add(egui::ProgressBar::new(cal.progress()).text("Rede jetzt normal …"));
        } else if ui.add_enabled(self.meter.is_some(), egui::Button::new("🎤 Einlernen starten")).clicked() {
            self.calibration = Some(Calibration::new());
            self.calibration_message = None;
        }
        if let Some(msg) = &self.calibration_message {
            ui.label(msg);
        }
        let s = &mut self.settings;
        ui.add(egui::Slider::new(&mut s.yellow_offset_db, 1.0..=20.0).text("Gelb über normal").suffix(" dB"));
        ui.add(egui::Slider::new(&mut s.red_offset_db, 1.0..=30.0).text("Rot über normal").suffix(" dB"));
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        self.version_ui(ui);
        ui.separator();
        self.display_ui(ui);
        ui.separator();
        self.microphone_ui(ui);
        ui.separator();
        self.agc_ui(ui);
        ui.separator();
        self.calibration_ui(ui);
        ui.separator();

        let s = &mut self.settings;
        ui.heading("Schwellen");
        ui.add(egui::Slider::new(&mut s.yellow_db, BAR_MIN_DB..=BAR_MAX_DB).text("Gelb ab").suffix(" dB"));
        ui.add(egui::Slider::new(&mut s.red_db, BAR_MIN_DB..=BAR_MAX_DB).text("Rot ab").suffix(" dB"));
        if s.red_db < s.yellow_db {
            s.red_db = s.yellow_db;
        }

        ui.separator();
        ui.heading("Reaktion");
        ui.add(egui::Slider::new(&mut s.attack_ms, 0.0..=500.0).text("Anstieg").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.release_ms, 0.0..=3000.0).text("Abklingen").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.hold_ms, 0.0..=5000.0).text("Gelb/Rot halten").suffix(" ms"));

        ui.separator();
        ui.heading("Ton");
        ui.checkbox(&mut s.beep_enabled, "Kurzer Ton, wenn es rot wird");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut s.beep_volume, 0.0..=1.0).text("Lautstärke"));
            if ui.button("Testen").clicked() {
                beep::play(s.beep_volume);
            }
        });

        if autostart::SUPPORTED {
            ui.separator();
            ui.heading("Start");
            let mut enabled = self.autostart_enabled;
            if ui.checkbox(&mut enabled, "Mit Windows starten").changed() {
                match autostart::set_enabled(enabled) {
                    Ok(()) => {
                        self.autostart_enabled = enabled;
                        self.autostart_error = None;
                    }
                    Err(e) => self.autostart_error = Some(e),
                }
            }
            if let Some(err) = &self.autostart_error {
                ui.colored_label(RED_TEXT, err);
            }
        }

        ui.separator();
        ui.label(format!("Rot seit Programmstart: {}×", self.red_count));
        if ui.button("Lärmampel beenden").clicked() {
            ui.ctx().send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }
    }
}

impl eframe::App for LaermampelApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.2);
        self.last_tick = now;

        if let Some(fault) = self.meter.as_ref().and_then(Meter::fault) {
            self.meter = None;
            self.meter_error = Some(format!("{fault} Verbinde neu …"));
        }
        if self.meter.is_none() && self.last_retry.elapsed() >= RETRY_INTERVAL {
            self.restart_meter();
        }

        let input = self.meter.as_ref().and_then(Meter::take_peak_db);
        let became_red = self.level.update(input, dt, now, &self.settings);
        if became_red {
            self.red_count += 1;
            if self.settings.beep_enabled {
                beep::play(self.settings.beep_volume);
            }
        }

        if let (Some(cal), Some(db)) = (&mut self.calibration, input) {
            cal.push(db);
        }
        if self.calibration.as_ref().is_some_and(Calibration::done) {
            let cal = self.calibration.take().expect("checked above");
            match cal.result() {
                Ok(normal) => self.apply_calibration(normal),
                Err(e) => self.calibration_message = Some(e.to_string()),
            }
        }

        self.sync_agc_params();

        let actions = self.tray.as_ref().map(Tray::poll).unwrap_or_default();
        for action in actions {
            match action {
                TrayAction::OpenSettings => self.open_settings(),
                TrayAction::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
        if self.instance.show_requested() {
            self.open_settings();
        }
        let zone = self.shown_zone();
        if let Some(tray) = &mut self.tray {
            tray.set_zone(zone);
        }

        if matches!(self.updater.status(), Status::Installed(_)) {
            self.exit_for_update();
        }

        self.update_placement(ctx, frame);

        if self.settings != self.saved && self.last_save.elapsed() >= SAVE_INTERVAL {
            settings::save(&self.settings);
            self.saved = self.settings.clone();
            self.last_save = now;
        }

        // Dauerhaft etwa 60 Mal pro Sekunde auswerten, auch wenn sich nichts bewegt.
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw_indicator(ui);
        let ctx = ui.ctx().clone();
        self.show_settings_window(&ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        settings::save(&self.settings);
    }
}

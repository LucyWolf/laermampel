use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

use crate::audio::{self, InputDevice, Meter};
use crate::beep;
use crate::level::{Calibration, Level, Zone};
use crate::settings::{self, Settings};
use crate::updater::{self, Status, Updater};

pub const COMPACT_SIZE: Vec2 = Vec2::new(300.0, 56.0);
const EXPANDED_SIZE: Vec2 = Vec2::new(300.0, 720.0);

/// Anzeigebereich des Balkens in dBFS.
const BAR_MIN_DB: f32 = -60.0;
const BAR_MAX_DB: f32 = 0.0;

const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

pub struct LaermampelApp {
    settings: Settings,
    saved: Settings,
    last_save: Instant,

    meter: Option<Meter>,
    meter_error: Option<String>,
    last_retry: Instant,
    devices: Vec<InputDevice>,

    level: Level,
    last_tick: Instant,
    red_count: u32,

    calibration: Option<Calibration>,
    calibration_message: Option<String>,

    show_settings: bool,
    applied_on_top: bool,
    updater: Updater,
}

impl LaermampelApp {
    pub fn new(settings: Settings, ctx: &egui::Context) -> Self {
        let applied_on_top = settings.always_on_top;
        let mut app = Self {
            saved: settings.clone(),
            settings,
            last_save: Instant::now(),
            meter: None,
            meter_error: None,
            last_retry: Instant::now(),
            devices: audio::list_input_devices(),
            level: Level::new(),
            last_tick: Instant::now(),
            red_count: 0,
            calibration: None,
            calibration_message: None,
            show_settings: false,
            applied_on_top,
            updater: Updater::new(),
        };
        app.restart_meter();
        app.updater.check(ctx);
        app
    }

    fn restart_meter(&mut self) {
        self.meter = None;
        self.last_retry = Instant::now();
        match Meter::start(self.settings.device_id.as_deref()) {
            Ok(m) => {
                self.meter = Some(m);
                self.meter_error = None;
            }
            Err(e) => self.meter_error = Some(e),
        }
    }

    fn zone_color(&self, zone: Zone) -> Color32 {
        match zone {
            Zone::Green => Color32::from_rgb(40, 200, 90),
            Zone::Yellow => Color32::from_rgb(245, 190, 20),
            Zone::Red => Color32::from_rgb(235, 45, 45),
        }
    }

    fn zone_brightness(&self, zone: Zone) -> f32 {
        match zone {
            Zone::Green => self.settings.green_brightness,
            _ => self.settings.brightness,
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

    fn draw_meter(&self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
        let painter = ui.painter();
        let to_x = |db: f32| {
            let t = ((db - BAR_MIN_DB) / (BAR_MAX_DB - BAR_MIN_DB)).clamp(0.0, 1.0);
            rect.left() + t * rect.width()
        };

        painter.rect_filled(rect, CornerRadius::same(4), Color32::from_black_alpha(140));
        let fill = Rect::from_min_max(rect.min, Pos2::new(to_x(self.level.display_db), rect.max.y));
        painter.rect_filled(fill, CornerRadius::same(4), self.zone_color(self.level.zone));

        for (db, color) in [(self.settings.yellow_db, Color32::from_rgb(245, 190, 20)), (self.settings.red_db, Color32::from_rgb(235, 45, 45))] {
            let x = to_x(db);
            painter.line_segment(
                [Pos2::new(x, rect.top() - 2.0), Pos2::new(x, rect.bottom() + 2.0)],
                Stroke::new(2.0, color),
            );
        }
    }

    fn set_settings_open(&mut self, ctx: &egui::Context, open: bool) {
        self.show_settings = open;
        let size = if open { EXPANDED_SIZE } else { COMPACT_SIZE };
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
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
                ui.colored_label(Color32::from_rgb(235, 90, 90), e);
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
                ui.label(format!("v{} ist installiert.", release.version));
                if ui.button("Jetzt neu starten").clicked() {
                    settings::save(&self.settings);
                    self.saved = self.settings.clone();
                    match updater::restart() {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self.updater.fail(format!("Neustart fehlgeschlagen: {e}")),
                    }
                }
            }
        }
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        self.version_ui(ui);
        ui.add_space(8.0);

        let s = &mut self.settings;

        ui.heading("Mikrofon");
        let selected = self
            .meter
            .as_ref()
            .map(|m| m.device_name.clone())
            .unwrap_or_else(|| "–".to_string());
        let mut new_device: Option<Option<String>> = None;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("device")
                .selected_text(selected)
                .width(200.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(s.device_id.is_none(), "Standardgerät").clicked() {
                        new_device = Some(None);
                    }
                    for d in &self.devices {
                        let active = s.device_id.as_deref() == Some(d.id.as_str());
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
            ui.colored_label(Color32::from_rgb(235, 90, 90), err);
        }

        ui.add_space(8.0);
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
        ui.add(egui::Slider::new(&mut s.yellow_offset_db, 1.0..=20.0).text("Gelb über normal").suffix(" dB"));
        ui.add(egui::Slider::new(&mut s.red_offset_db, 1.0..=30.0).text("Rot über normal").suffix(" dB"));

        ui.add_space(8.0);
        ui.heading("Schwellen");
        ui.label(format!("Aktuell: {:.1} dB", self.level.display_db));
        ui.add(egui::Slider::new(&mut s.yellow_db, BAR_MIN_DB..=BAR_MAX_DB).text("Gelb ab").suffix(" dB"));
        ui.add(egui::Slider::new(&mut s.red_db, BAR_MIN_DB..=BAR_MAX_DB).text("Rot ab").suffix(" dB"));
        if s.red_db < s.yellow_db {
            s.red_db = s.yellow_db;
        }

        ui.add_space(8.0);
        ui.heading("Reaktion");
        ui.add(egui::Slider::new(&mut s.attack_ms, 0.0..=500.0).text("Anstieg").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.release_ms, 0.0..=3000.0).text("Abklingen").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.hold_ms, 0.0..=5000.0).text("Gelb/Rot halten").suffix(" ms"));

        ui.add_space(8.0);
        ui.heading("Anzeige");
        ui.add(egui::Slider::new(&mut s.brightness, 0.0..=1.0).text("Helligkeit Gelb/Rot"));
        ui.add(egui::Slider::new(&mut s.green_brightness, 0.0..=1.0).text("Helligkeit Grün"));
        ui.checkbox(&mut s.always_on_top, "Immer im Vordergrund");

        ui.add_space(8.0);
        ui.heading("Ton");
        ui.checkbox(&mut s.beep_enabled, "Kurzer Ton, wenn es rot wird");
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut s.beep_volume, 0.0..=1.0).text("Lautstärke"));
            if ui.button("Testen").clicked() {
                beep::play(s.beep_volume);
            }
        });

        ui.add_space(8.0);
        ui.label(format!("Rot seit Programmstart: {}×", self.red_count));

        if let Some(device) = new_device {
            self.settings.device_id = device;
            self.restart_meter();
        }
    }
}

impl eframe::App for LaermampelApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.2);
        self.last_tick = now;

        if self.meter.as_ref().is_some_and(Meter::failed) {
            self.meter = None;
            self.meter_error = Some("Mikrofon getrennt, verbinde neu …".to_string());
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

        if let Some(rect) = ctx.input(|i| i.viewport().outer_rect) {
            self.settings.window_pos = Some([rect.min.x, rect.min.y]);
        }
        if self.settings.always_on_top != self.applied_on_top {
            self.applied_on_top = self.settings.always_on_top;
            let level = if self.settings.always_on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
        if self.settings != self.saved && self.last_save.elapsed() >= SAVE_INTERVAL {
            settings::save(&self.settings);
            self.saved = self.settings.clone();
            self.last_save = now;
        }

        // Dauerhaft etwa 60 Mal pro Sekunde auswerten, auch wenn sich nichts bewegt.
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let zone = self.level.zone;
        let tint = self.zone_color(zone).gamma_multiply(self.zone_brightness(zone));
        let background = Color32::from_rgb(24, 24, 28);

        egui::Frame::new()
            .fill(background)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(0.0)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());

                // Kopfzeile: leuchtende Fläche in der Zonenfarbe, zum Verschieben des Fensters.
                let (header, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), COMPACT_SIZE.y), Sense::click_and_drag());
                if response.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.painter().rect_filled(header, CornerRadius::same(10), tint);

                let inner = header.shrink2(Vec2::new(12.0, 10.0));
                let meter_rect = Rect::from_min_max(
                    Pos2::new(inner.left(), inner.center().y - 7.0),
                    Pos2::new(inner.right() - 82.0, inner.center().y + 7.0),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(meter_rect), |ui| self.draw_meter(ui));

                let buttons = Rect::from_min_max(Pos2::new(inner.right() - 76.0, inner.top()), inner.max);
                let mut close = false;
                let mut toggle_settings = false;
                let mut open_settings = false;
                let update_available = matches!(self.updater.status(), Status::Available(_) | Status::Installed(_));
                ui.scope_builder(egui::UiBuilder::new().max_rect(buttons), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = ui.small_button("✕").on_hover_text("Beenden").clicked();
                        let label = if self.show_settings { "▲" } else { "⚙" };
                        toggle_settings = ui.small_button(label).on_hover_text("Einstellungen").clicked();
                        if update_available && !self.show_settings {
                            open_settings = ui.small_button("⬆").on_hover_text("Update verfügbar").clicked();
                        }
                    });
                });
                if toggle_settings || open_settings {
                    let open = open_settings || !self.show_settings;
                    self.set_settings_open(ui.ctx(), open);
                }
                if close {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }

                if self.show_settings {
                    egui::Frame::new().inner_margin(12.0).show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| self.settings_ui(ui));
                    });
                }
            });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        settings::save(&self.settings);
    }
}

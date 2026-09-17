use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, ViewportCommand, ViewportId};

use crate::agc::{self, ChainControl};
#[cfg(windows)]
use crate::apo_setup;
use crate::audio::{self, InputDevice, Meter, VoiceSetup};
use crate::autostart;
use crate::beep;
use crate::dsp::{CompSettings, GateSettings, Settings as ChainSettings};
use crate::instance;
use crate::level::{Level, Zone};
use crate::placement::{self, Anchor, Monitor, PhysicalRect};
use crate::settings::{self, DisplayMode, Settings};
use crate::strip;
use crate::tray::{Tray, TrayAction};
use crate::updater::{self, Status, Updater};
use crate::volume_gate::{GateParams, VolumeGate};

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

fn gate_on(settings: &Settings) -> bool {
    settings.gate_threshold_db > settings::GATE_OFF_DB + 0.5
}

/// Comp-Knopf 1 bis 10 → höchstens 2 bis 20 dB lauter bzw. leiser.
fn comp_range_db(knob: f32) -> f32 {
    knob * 2.0
}

const KNOB_OFF: f32 = 0.05;

/// Zustand beim Austragen eines alten Audio-Filters (läuft im Hintergrund, wartet auf UAC).
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
enum ApoJob {
    Idle,
    Running(&'static str),
    Failed(String),
}

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
    chain_control: Arc<ChainControl>,

    level: Level,
    last_tick: Instant,
    red_count: u32,


    monitors: Vec<Monitor>,
    last_monitor_scan: Instant,
    applied_rect: Option<PhysicalRect>,
    last_reassert: Instant,

    settings_open: bool,
    focus_settings: bool,
    preview_until: Option<Instant>,
    /// Kleines Fenster mit den Anzeige-Einstellungen, geöffnet über den Knopf im Kanalzug.
    display_window_open: bool,
    /// Allgemeine Einstellungen (Version, Autostart, Beenden) hinter dem Zahnrad oben rechts.
    general_window_open: bool,
    autostart_enabled: bool,
    autostart_error: Option<String>,

    tray: Option<Tray>,
    /// Einmal erzeugt: egui vergleicht Icons nur per Zeiger. Ein neues pro Bild würde
    /// Windows 60-mal pro Sekunde ein neues Fenster-Icon setzen lassen.
    icon: Arc<egui::IconData>,
    settings_window_seen: bool,
    last_logged_error: Option<String>,
    instance: instance::Guard,
    updater: Updater,

    /// Gate und Mute über den Windows-Regler, wenn VB-Cable das Mikrofon nicht bearbeitet.
    volume_gate: VolumeGate,
    #[cfg_attr(not(windows), allow(dead_code))]
    apo_job: Arc<Mutex<ApoJob>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    /// Ist noch ein Filter aus einer älteren Version eingetragen? Gelegentlich neu gelesen.
    leftover_filter: Option<(bool, Instant)>,
}

impl LaermampelApp {
    pub fn new(settings: Settings, instance: instance::Guard, ctx: &egui::Context) -> Self {
        let tray = Tray::new();
        log!("Symbol im Infobereich: {}", if tray.is_some() { "ok" } else { "nicht verfügbar" });
        let mut app = Self {
            saved: settings.clone(),
            settings,
            last_save: Instant::now(),
            meter: None,
            meter_error: None,
            last_retry: Instant::now(),
            devices: audio::list_input_devices(),
            output_devices: agc::list_output_devices(),
            chain_control: Arc::new(ChainControl::default()),
            level: Level::new(),
            last_tick: Instant::now(),
            red_count: 0,
            monitors: placement::monitors(),
            last_monitor_scan: Instant::now(),
            applied_rect: None,
            last_reassert: Instant::now(),
            // Ohne Symbol im Infobereich kämen wir sonst nie an die Einstellungen.
            settings_open: tray.is_none(),
            focus_settings: false,
            preview_until: None,
            display_window_open: false,
            general_window_open: false,
            autostart_enabled: autostart::is_enabled(),
            autostart_error: None,
            tray,
            icon: Arc::new(app_icon()),
            settings_window_seen: false,
            last_logged_error: None,
            instance,
            updater: Updater::new(),
            volume_gate: VolumeGate::new(),
            apo_job: Arc::new(Mutex::new(ApoJob::Idle)),
            leftover_filter: None,
        };
        // Alte Auswahl von Kopfhörern oder Lautsprechern als Ausgabe verwerfen (Rückkopplung).
        if let Some(id) = &app.settings.agc_output_id
            && !app.output_devices.iter().any(|d| &d.id == id)
        {
            log!("Ausgabe-Auswahl verworfen, kein virtuelles Gerät: {id}");
            app.settings.agc_output_id = None;
        }
        app.sync_chain();
        app.restart_meter();
        app.updater.check(ctx);
        app
    }

    fn restart_meter(&mut self) {
        self.meter = None;
        self.last_retry = Instant::now();
        // Der Kanalzug gibt immer auf VB-Cable aus, sofern vorhanden.
        let voice_setup = Some(VoiceSetup {
            output_id: self.settings.agc_output_id.clone(),
            control: Arc::clone(&self.chain_control),
        });
        let Some(device_id) = self.settings.device_id.clone() else {
            self.meter_error = Some(audio::NO_DEVICE.to_string());
            return;
        };
        match Meter::start(&device_id, voice_setup) {
            Ok(m) => {
                let message = format!(
                    "Mikrofon läuft: {} · Ausgabe: {}",
                    m.input_name,
                    m.agc_output_name.clone().or(m.agc_error.clone()).unwrap_or_default()
                );
                self.log_once(message);
                self.meter = Some(m);
                self.meter_error = None;
            }
            Err(e) => {
                self.log_once(format!("Mikrofon-Fehler: {e}"));
                self.meter_error = Some(e);
            }
        }
    }

    /// Wiederholte gleiche Meldungen (z.B. Neuverbinden alle 2 s) nur einmal ins Log.
    fn log_once(&mut self, message: String) {
        if self.last_logged_error.as_ref() != Some(&message) {
            log!("{message}");
            self.last_logged_error = Some(message);
        }
    }

    /// Einstellungen aus der Oberfläche für die Bearbeitungskette.
    fn chain_settings(&self) -> ChainSettings {
        let s = &self.settings;
        ChainSettings {
            gate: GateSettings {
                enabled: gate_on(s),
                threshold_db: s.gate_threshold_db,
                range_db: s.gate_range_db,
                attack_ms: s.gate_attack_ms,
                hold_ms: s.gate_hold_ms,
                release_ms: s.gate_release_ms,
            },
            comp: CompSettings {
                enabled: s.comp_knob > KNOB_OFF,
                target_db: s.agc_target_db,
                max_gain_db: comp_range_db(s.comp_knob),
                max_cut_db: comp_range_db(s.comp_knob),
                attack_ms: s.agc_attack_ms,
                release_ms: s.agc_release_ms,
                pause_db: s.agc_gate_db,
            },
            fader_db: s.fader_db,
            muted: s.mic_muted,
            ceiling_db: s.agc_ceiling_db,
        }
    }

    fn sync_chain(&self) {
        self.chain_control.set(self.chain_settings());
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
        log!("Einstellungen angefordert");
        self.settings_open = true;
        self.focus_settings = true;
        self.devices = audio::list_input_devices();
        self.output_devices = agc::list_output_devices();
        self.monitors = placement::monitors();
    }

    fn close_settings(&mut self, ctx: &egui::Context) {
        log!("Einstellungen geschlossen");
        self.settings_open = false;
        self.settings_window_seen = false;
        if self.tray.is_none() {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }
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

        // Stumm soll man immer sehen, auch wenn Grün ausgeblendet ist.
        let muted = self.settings.mic_muted;
        let mute_ring = Stroke::new(2.5, Color32::from_rgb(220, 40, 40));

        match self.settings.display {
            DisplayMode::Dot => {
                let radius = rect.width().min(rect.height()) / 2.0 - 1.5;
                if muted {
                    painter.circle(rect.center(), radius, Color32::from_gray(90), mute_ring);
                    return;
                }
                if brightness <= 0.01 {
                    return;
                }
                // Dunkler Rand, damit der Punkt auch vor hellem Hintergrund zu sehen ist.
                let outline = Color32::from_black_alpha((160.0 * brightness) as u8);
                painter.circle(rect.center(), radius, tint, Stroke::new(1.5, outline));
            }
            DisplayMode::Bar => {
                painter.rect_filled(rect, CornerRadius::same(8), tint);
                self.draw_level_bar(painter, rect.shrink(7.0));
                if muted {
                    painter.rect_stroke(rect.shrink(1.5), CornerRadius::same(8), mute_ring, egui::StrokeKind::Inside);
                }
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
            .with_icon(Arc::clone(&self.icon))
            .with_inner_size([220.0, 470.0])
            .with_resizable(false)
            // Keine Windows-Titelleiste und durchsichtiger Rand: der Kanalzug selbst ist das Fenster.
            .with_decorations(false)
            .with_transparent(true);

        ctx.show_viewport_immediate(id, builder, |ui, class| {
            if !self.settings_window_seen {
                self.settings_window_seen = true;
                let kind = if matches!(class, egui::ViewportClass::EmbeddedWindow) { "eingebettet" } else { "eigenes Fenster" };
                log!("Einstellungsfenster gezeichnet ({kind})");
            }
            self.settings_ui(ui);

            let ctx = ui.ctx().clone();
            if self.display_window_open {
                self.display_window_open = self.sub_window(&ctx, "anzeige", "Lärmampel – Anzeige", [360.0, 420.0], Self::display_ui);
            }
            if self.general_window_open {
                self.general_window_open =
                    self.sub_window(&ctx, "allgemein", "Lärmampel – Einstellungen", [400.0, 640.0], Self::general_ui);
            }
            if ui.input(|i| i.viewport().close_requested()) {
                self.close_settings(ui.ctx());
            }
        });

        if self.focus_settings {
            // Falls es über das eigene „–“ minimiert wurde, erst wiederherstellen.
            ctx.send_viewport_cmd_to(id, ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd_to(id, ViewportCommand::Focus);
            self.focus_settings = false;
        }
    }

    /// Eigenes kleines Fenster. Gibt `false` zurück, sobald es geschlossen wurde.
    fn sub_window(&mut self, ctx: &egui::Context, id: &str, title: &str, size: [f32; 2], content: fn(&mut Self, &mut egui::Ui)) -> bool {
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_icon(Arc::clone(&self.icon))
            .with_inner_size(size)
            .with_min_inner_size([280.0, 200.0]);
        let mut open = true;
        ctx.show_viewport_immediate(ViewportId::from_hash_of(id), builder, |ui, _class| {
            egui::Frame::central_panel(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| content(self, ui));
            });
            if ui.input(|i| i.viewport().close_requested()) {
                open = false;
            }
        });
        open
    }

    fn version_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Version");
        ui.horizontal(|ui| {
            ui.label(format!("Lärmampel v{}", updater::CURRENT_VERSION));
            #[cfg(windows)]
            if let Some(path) = crate::log::path()
                && ui.small_button("Log-Datei zeigen").clicked()
            {
                let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
            }
        });
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
        log!("Installer gestartet, beende für das Update");
        self.volume_gate.release();
        settings::save(&self.settings);
        self.saved = self.settings.clone();
        // Symbol im Infobereich sauber entfernen, sonst bleibt ein Geist-Symbol stehen.
        self.tray = None;
        std::process::exit(0);
    }

    fn display_ui(&mut self, ui: &mut egui::Ui) {
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

    fn strip_ui(&mut self, ui: &mut egui::Ui) {
        let output_name = self.meter.as_ref().and_then(|m| m.agc_output_name.clone());
        let output_error = self.meter.as_ref().and_then(|m| m.agc_error.clone());
        let devices = self.devices.clone();
        let chosen_name = self
            .settings
            .device_id
            .as_ref()
            .and_then(|id| devices.iter().find(|d| &d.id == id).map(|d| d.name.clone()));
        let device_problem = match (&self.settings.device_id, &chosen_name) {
            (None, _) => Some(audio::NO_DEVICE),
            (Some(_), None) => Some(audio::MISSING_DEVICE),
            _ => None,
        };
        // Andere Fehler (Zugriff verweigert, belegt …) zusätzlich unter dem Gerätenamen.
        let other_error = self
            .meter_error
            .clone()
            .filter(|e| e != audio::NO_DEVICE && e != audio::MISSING_DEVICE && device_problem.is_none());
        let current_id = self.settings.device_id.clone();
        let mut picked: Option<String> = None;
        let mut refresh_devices = false;

        let running = output_name.is_some();
        let voice_db = if self.meter.is_some() { self.level.display_db } else { -120.0 };
        // Werte vom Filter, sonst vom VB-Cable-Weg; ohne beides wird gar nicht bearbeitet.
        let feedback = running.then(|| self.chain_control.feedback());
        let volume_gating = feedback.is_none() && gate_on(&self.settings);
        let gate_open = gate_on(&self.settings)
            && match feedback {
                Some(f) => f.gate_open,
                None => self.volume_gate.is_open(),
            };
        // Anzeige wie beim Mischpult: was nach der Bearbeitung übrig bleibt. Nur bei Mute grau die Stimme.
        let meter_db = if self.settings.mic_muted {
            voice_db
        } else if let Some(f) = feedback {
            f.out_level_db
        } else if volume_gating {
            // Wie bei Voicemeeter: je höher das Gate, desto weniger Ausschlag.
            voice_db - self.volume_gate.reduction_db()
        } else {
            voice_db
        };
        let s = &self.settings;
        // Gate und Mute gehen notfalls über den Windows-Regler; Comp., Fader und Limiter nur mit VB-Cable.
        let wants_processing = s.comp_knob > KNOB_OFF || s.fader_db.abs() > 0.05 || s.agc_ceiling_db < 0.0;
        let nothing_processes = wants_processing && feedback.is_none();

        egui::Frame::new().fill(strip::PANEL).corner_radius(CornerRadius::same(8)).inner_margin(10.0).show(ui, |ui| {
            // Füllt das ganze Fenster aus, drumherum ist es durchsichtig.
            ui.set_width(200.0);
            ui.set_min_height(ui.available_height());

            // Eigene Titelzeile: links zum Verschieben, rechts Zahnrad, Minimieren, Schließen.
            let mut minimize = false;
            let mut close = false;
            let update_available = matches!(self.updater.status(), Status::Available(_));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let drag_width = ui.available_width() - 3.0 * (strip::TITLE_BUTTON_WIDTH + 2.0);
                let (drag_rect, drag) = ui.allocate_exact_size(egui::vec2(drag_width, 24.0), egui::Sense::click_and_drag());
                if drag.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
                }
                // Links wie beim Headpat Server: Punkt in der Ampelfarbe, Name, Version.
                let painter = ui.painter();
                let dot = Pos2::new(drag_rect.left() + 5.0, drag_rect.center().y);
                painter.circle_filled(dot, 4.0, zone_color(self.shown_zone()));
                let name_rect = painter.text(
                    Pos2::new(dot.x + 10.0, drag_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Lärmampel",
                    egui::FontId::proportional(14.0),
                    Color32::WHITE,
                );
                painter.text(
                    Pos2::new(name_rect.right() + 5.0, drag_rect.center().y + 1.0),
                    egui::Align2::LEFT_CENTER,
                    format!("v{}", updater::CURRENT_VERSION),
                    egui::FontId::proportional(11.0),
                    Color32::from_rgb(140, 146, 156),
                );
                if update_available {
                    painter.circle_filled(Pos2::new(drag_rect.right() - 6.0, drag_rect.center().y), 4.0, strip::ACCENT);
                }
                if strip::title_button(ui, strip::TitleIcon::Gear, self.general_window_open)
                    .on_hover_text(if update_available { "Einstellungen · Update verfügbar" } else { "Einstellungen" })
                    .clicked()
                {
                    self.general_window_open = !self.general_window_open;
                }
                minimize = strip::title_button(ui, strip::TitleIcon::Minimize, false).on_hover_text("Minimieren").clicked();
                close = strip::title_button(ui, strip::TitleIcon::Close, false).on_hover_text("Schließen").clicked();
            });
            if minimize {
                ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
            }
            if close {
                self.close_settings(ui.ctx());
            }

            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("MIKROFON").strong().size(14.0).color(Color32::WHITE));

                // Gerätename anklicken öffnet die Auswahl. Fehlt ein Gerät, blinkt es rot.
                let text = match (device_problem, &chosen_name) {
                    (Some(problem), _) => {
                        let blink_on = (ui.input(|i| i.time) * 2.0).fract() < 0.5;
                        let red = Color32::from_rgb(235, 60, 60);
                        egui::RichText::new(format!("{problem} ▾"))
                            .small()
                            .strong()
                            .color(if blink_on { red } else { red.gamma_multiply(0.35) })
                    }
                    (None, Some(name)) => egui::RichText::new(format!("{name} ▾")).small().weak(),
                    (None, None) => egui::RichText::new("▾").small(),
                };
                let menu = ui.menu_button(text, |ui| {
                    if devices.is_empty() {
                        ui.label("Keine Mikrofone gefunden");
                    }
                    for d in &devices {
                        let active = current_id.as_deref() == Some(d.id.as_str());
                        let label = if agc::is_virtual_device(&d.name) { format!("{} (virtuell)", d.name) } else { d.name.clone() };
                        if ui.selectable_label(active, label).clicked() {
                            picked = Some(d.id.clone());
                            ui.close();
                        }
                    }
                });
                if menu.response.clicked() {
                    refresh_devices = true;
                }
                if let Some(err) = &other_error {
                    ui.label(egui::RichText::new(err).small().color(RED_TEXT));
                }
            });
            ui.add_space(6.0);

            let s = &mut self.settings;
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                strip::knob(ui, &mut s.comp_knob, 10.0, "Comp.", false).on_hover_text(if s.comp_knob > KNOB_OFF {
                    format!(
                        "Automatische Lautstärke: bis ±{:.0} dB, gerade {:+.1} dB",
                        comp_range_db(s.comp_knob),
                        feedback.map_or(0.0, |f| f.comp_gain_db)
                    )
                } else {
                    "Automatische Lautstärke: aus".to_string()
                });
                let gate_hover = if gate_on(s) {
                    format!(
                        "Noise Gate: Schwelle {:.0} dB, Mikrofon gerade {:.0} dB",
                        s.gate_threshold_db,
                        feedback.map_or(voice_db, |f| f.gate_level_db)
                    )
                } else {
                    "Noise Gate: aus (ganz links)".to_string()
                };
                strip::knob_db(ui, &mut s.gate_threshold_db, settings::GATE_OFF_DB, 0.0, "Gate", gate_open).on_hover_text(gate_hover);
            });
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                // Gelb und Rot nur zeigen, solange der Warnton an ist.
                let gate_marker = gate_on(s).then_some((s.gate_threshold_db, gate_open));
                let thresholds = s.beep_enabled.then_some((&mut s.yellow_db, &mut s.red_db));
                let muted = s.mic_muted;
                strip::level_meter(ui, meter_db, muted, gate_marker, &mut s.agc_ceiling_db, thresholds, 230.0);
                strip::fader(ui, &mut s.fader_db, -60.0, 12.0, 230.0).on_hover_text("Gain · Doppelklick: 0 dB");
                ui.vertical(|ui| {
                    let mut display_open = self.display_window_open;
                    strip::toggle_button(ui, &mut display_open, "Anzeige", Color32::from_rgb(70, 110, 170))
                        .on_hover_text("Punkt oder Leiste, Monitor, Position, Größe, Helligkeit");
                    self.display_window_open = display_open;
                    ui.add_space(122.0);
                    if strip::toggle_button(ui, &mut s.beep_enabled, "Ton", Color32::from_rgb(200, 120, 30))
                        .on_hover_text("Warnton, wenn deine Stimme über den roten Pfeil in der Anzeige kommt")
                        .clicked()
                        && s.beep_enabled
                    {
                        beep::play(s.beep_volume);
                    }
                    ui.add_space(4.0);
                    strip::mute_button(ui, &mut s.mic_muted);
                });
            });

            // Genauer Wert zum Ablesen, z.B. um die Gate-Schwelle passend zu setzen.
            let reading = if voice_db > -99.5 { format!("Pegel {voice_db:.0} dB") } else { "Pegel –".to_string() };
            ui.label(egui::RichText::new(reading).small().color(Color32::from_rgb(170, 176, 186)));

            if nothing_processes {
                let warning = egui::RichText::new("⚠ Comp., Fader, Limiter: nur mit VB-Cable").small().color(RED_TEXT);
                if ui
                    .add(egui::Label::new(warning).sense(egui::Sense::click()))
                    .on_hover_text("Wirkt in anderen Programmen erst, wenn VB-Cable installiert ist. Öffnet die Einstellungen.")
                    .clicked()
                {
                    self.general_window_open = true;
                }
            }
            // Fehlendes VB-Cable ist kein Fehler, nur kaputte Einstellungen werden gemeldet.
            else if output_error.as_deref().is_some_and(|e| e != agc::VB_CABLE_MISSING) {
                let warning = egui::RichText::new("⚠ Ausgabe prüfen").small().color(RED_TEXT);
                if ui.add(egui::Label::new(warning).sense(egui::Sense::click())).on_hover_text("Öffnet die Einstellungen").clicked() {
                    self.general_window_open = true;
                }
            }
        });

        if refresh_devices {
            self.devices = audio::list_input_devices();
        }
        if let Some(id) = picked {
            self.settings.device_id = Some(id);
            self.restart_meter();
        }

    }

    /// Ausgabe, Hinweise zu VB-Cable und die Feineinstellungen des Kanalzugs.
    fn channel_details_ui(&mut self, ui: &mut egui::Ui) {
        let output_name = self.meter.as_ref().and_then(|m| m.agc_output_name.clone());
        let output_error = self.meter.as_ref().and_then(|m| m.agc_error.clone());
        if output_error.as_deref() == Some(agc::VB_CABLE_MISSING) {
            ui.label(
                "VB-Cable ist nicht installiert. Gate und Mute gehen auch ohne; Comp., Fader und Limiter \
                 wirken in anderen Programmen nur mit VB-Cable.",
            );
            ui.hyperlink_to("VB-Cable herunterladen", agc::VB_CABLE_URL);
        } else if let Some(err) = &output_error {
            ui.colored_label(RED_TEXT, err);
        } else if output_name.is_some() {
            ui.label("In Discord, Spielen usw. als Mikrofon „CABLE Output“ wählen.");
        }

        let mut restart = false;

        ui.horizontal(|ui| {
            ui.label("Ausgabe");
            let selected = output_name.clone().unwrap_or_else(|| "–".to_string());
            egui::ComboBox::from_id_salt("agc_output").selected_text(selected).width(220.0).show_ui(ui, |ui| {
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

        let s = &mut self.settings;
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Gate").strong());
        ui.add(egui::Slider::new(&mut s.gate_range_db, 0.0..=80.0).text("Absenkung").suffix(" dB"))
            .on_hover_text("Wie viel leiser, wenn zu. 80 dB ist praktisch stumm, 10–20 dB klingt natürlicher.");
        ui.add(egui::Slider::new(&mut s.gate_attack_ms, 0.5..=50.0).text("Öffnen").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.gate_hold_ms, 0.0..=2000.0).text("Halten").suffix(" ms"))
            .on_hover_text("So lange bleibt es nach dem letzten Wort offen.");
        ui.add(egui::Slider::new(&mut s.gate_release_ms, 10.0..=1000.0).text("Schließen").suffix(" ms"));

        ui.add_space(6.0);
        ui.label(egui::RichText::new("Ampel und Ton").strong());
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut s.beep_volume, 0.0..=1.0).text("Ton-Lautstärke"));
            if ui.button("Testen").clicked() {
                beep::play(s.beep_volume);
            }
        });
        ui.add(egui::Slider::new(&mut s.attack_ms, 0.0..=500.0).text("Anstieg").suffix(" ms"))
            .on_hover_text("Wie schnell die Ampel auf lautere Stimme reagiert");
        ui.add(egui::Slider::new(&mut s.release_ms, 0.0..=3000.0).text("Abklingen").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.hold_ms, 0.0..=5000.0).text("Gelb/Rot halten").suffix(" ms"));

        ui.add_space(6.0);
        ui.label(egui::RichText::new("Comp.").strong());
        ui.add(egui::Slider::new(&mut s.agc_target_db, -40.0..=-6.0).text("Ziellautstärke").suffix(" dB"));
        ui.add(egui::Slider::new(&mut s.agc_attack_ms, 5.0..=500.0).text("Runterregeln").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.agc_release_ms, 100.0..=5000.0).text("Hochregeln").suffix(" ms"));
        ui.add(egui::Slider::new(&mut s.agc_gate_db, -80.0..=-20.0).text("Pause unter").suffix(" dB"))
            .on_hover_text("Leiser als das gilt als Sprechpause, dann wird nichts hochgezogen");




        if restart {
            self.restart_meter();
        }
    }

    /// Nur sichtbar, wenn aus einer älteren Version noch ein Audio-Filter eingetragen ist.
    #[cfg(windows)]
    fn leftover_filter_ui(&mut self, ui: &mut egui::Ui) {
        let stale = self.leftover_filter.is_none_or(|(_, t)| t.elapsed() > Duration::from_secs(5));
        if stale {
            self.leftover_filter = Some((apo_setup::any_installed(), Instant::now()));
        }
        let installed = self.leftover_filter.is_some_and(|(installed, _)| installed);
        let job = self.apo_job.lock().map(|j| j.clone()).unwrap_or(ApoJob::Idle);
        if !installed && !matches!(job, ApoJob::Running(_)) {
            return;
        }
        ui.separator();
        match job {
            ApoJob::Running(what) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(what);
                });
            }
            ApoJob::Failed(e) => {
                ui.colored_label(RED_TEXT, e);
            }
            ApoJob::Idle => {}
        }
        if installed {
            ui.label("Aus einer älteren Version ist noch ein Mikrofon-Filter eingetragen.");
            if ui.button("Entfernen").clicked() {
                self.start_apo_job("Entferne Filter, der Ton ist kurz weg …", "--apo uninstall-all".to_string());
            }
        }
    }

    #[cfg(windows)]
    fn start_apo_job(&mut self, what: &'static str, args: String) {
        let job = Arc::clone(&self.apo_job);
        if let Ok(mut j) = job.lock() {
            *j = ApoJob::Running(what);
        }
        self.leftover_filter = None;
        log!("APO: starte {args}");
        std::thread::spawn(move || {
            let result = apo_setup::run_elevated(&args);
            if let Ok(mut j) = job.lock() {
                *j = match result {
                    Ok(()) => ApoJob::Idle,
                    Err(e) => ApoJob::Failed(e),
                };
            }
        });
        // Nach dem Neustart des Audiodienstes ist die Aufnahme weg; die Lärmampel verbindet sich
        // über die normale Fehlerbehandlung von selbst neu.
    }

    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        self.strip_ui(ui);
    }

    fn general_ui(&mut self, ui: &mut egui::Ui) {
        self.version_ui(ui);

        ui.separator();
        egui::CollapsingHeader::new("Kanalzug: Ausgabe und Feineinstellungen")
            .id_salt("strip_details")
            .default_open(self.meter.as_ref().and_then(|m| m.agc_error.as_deref()).is_some_and(|e| e != agc::VB_CABLE_MISSING))
            .show(ui, |ui| self.channel_details_ui(ui));

        #[cfg(windows)]
        self.leftover_filter_ui(ui);

        if autostart::SUPPORTED {
            ui.separator();
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
            self.log_once(format!("Audio-Fehler: {fault}"));
            self.meter = None;
            self.meter_error = Some(format!("{fault} Verbinde neu …"));
        }
        if self.meter.is_none() && self.last_retry.elapsed() >= RETRY_INTERVAL {
            self.restart_meter();
        }

        let meter_input = self.meter.as_ref().and_then(Meter::take_peak_db);
        // Mit Filter bekommt auch die Lärmampel schon bearbeitetes Audio (z.B. stumm),
        // deshalb misst dann der Filter selbst die echte Lautstärke.
        // Bearbeitet weder Filter noch VB-Cable das Mikrofon, macht der Windows-Regler das Gate.
        let chain_running = self.meter.as_ref().is_some_and(|m| m.agc_output_name.is_some());
        let volume_gate_on = gate_on(&self.settings) && !chain_running;
        let volume_mute_on = self.settings.mic_muted && !chain_running;
        let gate_params = GateParams {
            threshold_db: self.settings.gate_threshold_db,
            range_db: self.settings.gate_range_db,
            attack_ms: self.settings.gate_attack_ms,
            hold_ms: self.settings.gate_hold_ms,
            release_ms: self.settings.gate_release_ms,
        };
        let device_id = self.settings.device_id.clone();
        let input = self.volume_gate.update(device_id.as_deref(), volume_gate_on, volume_mute_on, meter_input, &gate_params);
        let became_red = self.level.update(input, dt, now, &self.settings);
        if became_red {
            self.red_count += 1;
            if self.settings.beep_enabled {
                beep::play(self.settings.beep_volume);
            }
        }

        self.sync_chain();

        let actions = self.tray.as_ref().map(Tray::poll).unwrap_or_default();
        for action in actions {
            match action {
                TrayAction::OpenSettings => self.open_settings(),
                TrayAction::ToggleMute => self.settings.mic_muted = !self.settings.mic_muted,
                TrayAction::Quit => ctx.send_viewport_cmd(ViewportCommand::Close),
            }
        }
        if self.instance.show_requested() {
            self.open_settings();
        }
        let zone = self.shown_zone();
        if let Some(tray) = &mut self.tray {
            tray.set_state(zone, self.settings.mic_muted);
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
        self.volume_gate.release();
        settings::save(&self.settings);
    }
}

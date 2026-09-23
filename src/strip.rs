//! Bedienelemente im Stil eines Mischpult-Kanalzugs: Drehknopf, Fader, Pegelanzeige.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2,
};

use crate::lang::t;

pub const PANEL: Color32 = Color32::from_rgb(40, 44, 52);
pub const ACCENT: Color32 = Color32::from_rgb(90, 200, 120);
const TRACK: Color32 = Color32::from_rgb(22, 24, 28);
const LABEL: Color32 = Color32::from_rgb(170, 176, 186);

/// Drehknopf von 0 bis `max`. Ziehen nach oben/unten oder Mausrad ändert, Doppelklick setzt auf 0.
pub fn knob(ui: &mut Ui, value: &mut f32, max: f32, label: &str, lit: bool) -> Response {
    let text = format!("{:.1}", *value);
    knob_range(ui, value, 0.0, max, 0.1, text, label, lit)
}

/// Drehknopf in dB von `min_db` bis `max_db`; ganz links heißt „aus“. Doppelklick: aus.
pub fn knob_db(ui: &mut Ui, value: &mut f32, min_db: f32, max_db: f32, label: &str, lit: bool) -> Response {
    let text = if *value <= min_db + 0.5 { t("aus", "off").to_string() } else { format!("{:.0} dB", *value) };
    knob_range(ui, value, min_db, max_db, 1.0, text, label, lit)
}

/// Drehknopf in Hz; ganz links heißt „aus“. Doppelklick: aus.
pub fn knob_hz(ui: &mut Ui, value: &mut f32, min_hz: f32, max_hz: f32, label: &str, lit: bool) -> Response {
    let text = if *value <= min_hz + 0.5 { t("aus", "off").to_string() } else { format!("{:.0} Hz", *value) };
    knob_range(ui, value, min_hz, max_hz, 5.0, text, label, lit)
}

#[allow(clippy::too_many_arguments)]
fn knob_range(ui: &mut Ui, value: &mut f32, min: f32, max: f32, step: f32, text: String, label: &str, lit: bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(64.0, 92.0), Sense::click_and_drag());
    let span = max - min;

    // Ungerundeten Wert zwischen den Bildern merken, sonst gehen kleine Mausbewegungen beim
    // Runden auf die Schrittweite verloren.
    let raw_id = response.id.with("ungerundet");
    let mut raw = ui.data(|d| d.get_temp::<f32>(raw_id)).filter(|r| (r - *value).abs() <= step).unwrap_or(*value);
    if response.dragged() {
        raw -= response.drag_delta().y * span / 150.0;
    }
    // Nur wenn das Fenster auch vorn ist: Windows schickt das Mausrad an das Fenster unter
    // dem Zeiger, selbst wenn man gerade in einem anderen Programm scrollt („Inaktive Fenster
    // beim Daraufzeigen scrollen“ ist ab Werk an). Ohne diese Abfrage verstellt ein Dreh am
    // Rad das Mikrofon, ohne dass man es merkt – und hinterher weiß niemand, warum.
    if response.hovered() && ui.input(|i| i.focused) {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        raw += scroll / 40.0 * span / 20.0;
    }
    if response.double_clicked() {
        raw = min;
    }
    raw = raw.clamp(min, max);
    ui.data_mut(|d| d.insert_temp(raw_id, raw));
    let new = ((raw / step).round() * step).clamp(min, max) + 0.0;
    if new != *value {
        *value = new;
        response.mark_changed();
    }

    let painter = ui.painter();
    let center = pos2(rect.center().x, rect.top() + 28.0);
    let radius = 22.0;
    let start = 135f32.to_radians();
    let sweep = 270f32.to_radians();
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let steps = 40;
        (0..=steps)
            .map(|i| {
                let a = from + (to - from) * i as f32 / steps as f32;
                center + vec2(a.cos(), a.sin()) * radius
            })
            .collect()
    };

    painter.circle_filled(center, radius - 5.0, Color32::from_rgb(58, 63, 72));
    painter.add(Shape::line(arc(start, start + sweep), Stroke::new(4.0, TRACK)));
    let t = if span > 0.0 { ((*value - min) / span).clamp(0.0, 1.0) } else { 0.0 };
    if t > 0.0 {
        painter.add(Shape::line(arc(start, start + sweep * t), Stroke::new(4.0, ACCENT)));
    }
    let pointer = start + sweep * t;
    painter.line_segment(
        [center + vec2(pointer.cos(), pointer.sin()) * 6.0, center + vec2(pointer.cos(), pointer.sin()) * 15.0],
        Stroke::new(2.0, Color32::WHITE),
    );
    painter.text(center + vec2(0.0, radius + 3.0), Align2::CENTER_TOP, text, FontId::proportional(11.0), Color32::WHITE);

    // Beschriftung, bei Bedarf mit kleiner Lampe (z.B. Gate offen).
    let label_pos = pos2(rect.center().x, rect.bottom() - 2.0);
    painter.text(label_pos, Align2::CENTER_BOTTOM, label, FontId::proportional(12.0), LABEL);
    if lit {
        painter.circle_filled(pos2(rect.right() - 6.0, rect.bottom() - 8.0), 3.5, ACCENT);
    }

    response
}

/// Senkrechter Fader in ganzen dB. Ziehen, Mausrad; Doppelklick setzt auf 0 dB.
pub fn fader(ui: &mut Ui, value: &mut f32, min: f32, max: f32, height: f32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(56.0, height), Sense::click_and_drag());
    let top = rect.top() + 18.0;
    let bottom = rect.bottom() - 18.0;
    let to_y = |v: f32| bottom - (v - min) / (max - min) * (bottom - top);

    // Ungerundeten Wert merken, damit kleine Mausrad-Bewegungen sich zu ganzen dB summieren.
    let raw_id = response.id.with("ungerundet");
    let mut raw = ui.data(|d| d.get_temp::<f32>(raw_id)).filter(|r| (r - *value).abs() <= 1.0).unwrap_or(*value);
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        raw = min + (bottom - pos.y) / (bottom - top) * (max - min);
    }
    // Siehe `knob_range`: sonst verstellt das Mausrad den Fader, während man woanders scrollt.
    if response.hovered() && ui.input(|i| i.focused) {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        raw += scroll / 40.0;
    }
    if response.double_clicked() {
        raw = 0.0;
    }
    raw = raw.clamp(min, max);
    ui.data_mut(|d| d.insert_temp(raw_id, raw));
    // Auf ganze dB, damit „0dB“ auch wirklich 0 ist.
    let new = raw.round().clamp(min, max) + 0.0;
    if new != *value {
        *value = new;
        response.mark_changed();
    }

    let painter = ui.painter();
    let x = rect.center().x;
    let track = Rect::from_min_max(pos2(x - 4.0, top), pos2(x + 4.0, bottom));
    painter.rect_filled(track, CornerRadius::same(3), TRACK);
    let fill = Rect::from_min_max(pos2(x - 4.0, to_y(*value)), pos2(x + 4.0, bottom));
    painter.rect_filled(fill, CornerRadius::same(3), ACCENT.gamma_multiply(0.7));

    // Markierung bei 0 dB
    let zero = to_y(0.0);
    painter.line_segment([pos2(x - 12.0, zero), pos2(x - 7.0, zero)], Stroke::new(1.0, LABEL));
    painter.line_segment([pos2(x + 7.0, zero), pos2(x + 12.0, zero)], Stroke::new(1.0, LABEL));

    let knob_center = pos2(x, to_y(*value));
    painter.circle(knob_center, 18.0, Color32::from_rgb(225, 230, 235), Stroke::new(2.0, ACCENT));
    // + 0.0 macht aus -0 eine 0, sonst steht bei -0.3 „-0dB“ da.
    let text = format!("{:.0}dB", value.round() + 0.0);
    painter.text(knob_center, Align2::CENTER_CENTER, text, FontId::proportional(11.0), Color32::from_rgb(30, 30, 34));

    response
}

pub const METER_MIN_DB: f32 = -100.0;
pub const LIMIT_MIN_DB: f32 = -40.0;
/// 0 dB = Limiter aus (nur die harte Grenze des Formats).
pub const LIMIT_OFF_DB: f32 = 0.0;

const RED: Color32 = Color32::from_rgb(235, 45, 45);
const YELLOW: Color32 = Color32::from_rgb(245, 190, 20);

#[derive(Clone, Copy, PartialEq)]
enum MeterLine {
    Yellow,
    Red,
    Limit,
}

/// Breite der dB-Skala rechts neben dem Balken.
const SCALE_WIDTH: f32 = 26.0;

/// Breite des Randes links für die Pfeile. Bleibt immer frei, damit nichts springt.
const ARROW_GUTTER: f32 = 12.0;

/// Pegelanzeige, -100 bis 0 dB.
/// `zones` sind Gelb und Rot der Ampel: dieselben Schwellen wie beim Punkt bzw. der Leiste
/// auf dem Bildschirm, damit beide Anzeigen dasselbe sagen. Links am Rand sitzen sie als
/// greifbare Pfeile.
/// Der Limiter ist auf 0 dB aus und erscheint dann nur, wenn die Maus über seinem Balken ist.
pub fn level_meter(
    ui: &mut Ui,
    voice_db: f32,
    muted: bool,
    gate: Option<(f32, bool)>,
    // Ohne Ausgang kann der Limiter nichts begrenzen; dann wird er grau gezeichnet.
    limit_running: bool,
    limit_db: &mut f32,
    zones: (&mut f32, &mut f32),
    // Löst der rote Bereich auch den Warnton aus? Ändert nur den Hinweistext.
    beeps: bool,
    height: f32,
) -> Response {
    let (yellow, red) = zones;
    // Ein Balken: ein Mikrofon ist mono.
    let (rect, mut response) = ui.allocate_exact_size(vec2(34.0 + ARROW_GUTTER + SCALE_WIDTH, height), Sense::click_and_drag());
    let gutter = Rect::from_min_max(rect.min, pos2(rect.left() + ARROW_GUTTER, rect.bottom()));
    let meter = Rect::from_min_max(pos2(gutter.right(), rect.top()), pos2(rect.right() - SCALE_WIDTH, rect.bottom()));
    let scale = Rect::from_min_max(pos2(meter.right(), rect.top()), rect.max);
    let inner = meter.shrink(3.0);
    let to_y = |db: f32| inner.bottom() - ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0) * inner.height();
    let to_db = |y: f32| METER_MIN_DB + (inner.bottom() - y) / inner.height() * -METER_MIN_DB;

    // Am Rand bei den Pfeilen: Gelb oder Rot. In der Anzeige: die nächste Linie, Limiter eingeschlossen.
    let lines_y = Some((to_y(*yellow), to_y(*red)));
    let limit_line_y = to_y(*limit_db);
    let line_at = |p: Pos2| {
        let in_gutter = p.x < gutter.right();
        let Some((yellow_y, red_y)) = lines_y else {
            return (!in_gutter).then_some(MeterLine::Limit);
        };
        let mut best = (MeterLine::Limit, if in_gutter { f32::MAX } else { (p.y - limit_line_y).abs() });
        for (line, y) in [(MeterLine::Yellow, yellow_y), (MeterLine::Red, red_y)] {
            let distance = (p.y - y).abs();
            if distance < best.1 {
                best = (line, distance);
            }
        }
        Some(best.0)
    };
    let drag_id = response.id.with("linie");
    if response.drag_started()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let code: u8 = match line_at(pos) {
            Some(MeterLine::Yellow) => 0,
            Some(MeterLine::Red) => 1,
            Some(MeterLine::Limit) => 2,
            None => 3,
        };
        ui.data_mut(|d| d.insert_temp(drag_id, code));
    }
    let active_line = if response.dragged() {
        match ui.data(|d| d.get_temp::<u8>(drag_id)).unwrap_or(3) {
            0 => Some(MeterLine::Yellow),
            1 => Some(MeterLine::Red),
            2 => Some(MeterLine::Limit),
            _ => None,
        }
    } else {
        response.hover_pos().and_then(line_at)
    };

    let pointer_db = response.interact_pointer_pos().filter(|_| response.dragged()).map(|p| to_db(p.y));
    let mut changed = false;
    match active_line {
        Some(MeterLine::Limit) => {
            let mut new = pointer_db.unwrap_or(*limit_db);
            if response.double_clicked() {
                new = LIMIT_OFF_DB;
            }
            // + 0.0 macht aus -0 eine 0, sonst steht „-0“ da.
            let new = new.round().clamp(LIMIT_MIN_DB, LIMIT_OFF_DB) + 0.0;
            changed |= new != *limit_db;
            *limit_db = new;
        }
        Some(MeterLine::Yellow) => {
            if let Some(db) = pointer_db {
                // Gelb bleibt unter Rot.
                let new = db.round().clamp(METER_MIN_DB, *red) + 0.0;
                changed |= new != *yellow;
                *yellow = new;
            }
        }
        Some(MeterLine::Red) => {
            if let Some(db) = pointer_db {
                let new = db.round().clamp(*yellow, 0.0) + 0.0;
                changed |= new != *red;
                *red = new;
            }
        }
        None => {}
    }
    if changed {
        response.mark_changed();
    }
    if active_line.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    let painter = ui.painter();
    painter.rect_filled(meter, CornerRadius::same(3), TRACK);

    // dB-Skala rechts neben dem Balken.
    for step in 0..=10 {
        let db = step as f32 * -10.0;
        let y = to_y(db);
        painter.line_segment([pos2(scale.left() + 1.0, y), pos2(scale.left() + 4.0, y)], Stroke::new(1.0, LABEL));
        painter.text(pos2(scale.left() + 6.0, y), Align2::LEFT_CENTER, format!("{db:.0}"), FontId::proportional(9.0), LABEL);
    }
    // Wo das Gate steht: kleines Dreieck an der Skala, grün wenn offen, grau wenn zu.
    if let Some((threshold, open)) = gate {
        let y = to_y(threshold);
        let color = if open { ACCENT } else { Color32::from_gray(150) };
        let x = scale.left() + 1.0;
        painter.add(Shape::convex_polygon(
            vec![pos2(x, y), pos2(x + 7.0, y - 4.5), pos2(x + 7.0, y + 4.5)],
            color,
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        ));
    }

    const SEGMENTS: usize = 40;
    let segment_height = inner.height() / SEGMENTS as f32;
    // Stumm: grau, schlägt aber weiter aus, damit man sieht, dass das Mikrofon hört.
    {
        let column = inner;
        for i in 0..SEGMENTS {
            let db = METER_MIN_DB + (i as f32 + 0.5) / SEGMENTS as f32 * -METER_MIN_DB;
            // Dieselben Schwellen wie die Anzeige auf dem Bildschirm, sonst zeigen beide
            // bei derselben Stimme verschiedene Farben.
            let color = if muted {
                Color32::from_gray(150)
            } else if db >= *red {
                RED
            } else if db >= *yellow {
                YELLOW
            } else {
                ACCENT
            };
            let y1 = inner.bottom() - i as f32 * segment_height;
            let segment = Rect::from_min_max(pos2(column.left(), y1 - segment_height + 1.0), pos2(column.right(), y1));
            painter.rect_filled(segment, CornerRadius::ZERO, if db <= voice_db { color } else { color.gamma_multiply(0.12) });
        }
    }

    let limit_active = *limit_db < LIMIT_OFF_DB;
    if limit_active || active_line == Some(MeterLine::Limit) {
        // Der Limiter liegt über der ganzen Anzeige.
        let column = inner;
        let y = to_y(*limit_db);
        let line = if limit_running { Color32::from_rgb(225, 205, 70) } else { Color32::from_gray(150) };
        let area = Rect::from_min_max(column.min, pos2(column.right(), y));
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(90, 80, 10, 225));
        painter.line_segment([pos2(column.left(), y), pos2(column.right(), y)], Stroke::new(2.0, line));
        let value = if limit_active { format!("{:.0}", *limit_db) } else { t("aus", "off").to_string() };
        let label = format!("Lim\n{value}");
        let font = FontId::proportional(11.0);
        if area.height() >= 30.0 {
            painter.text(pos2(column.center().x, y - 3.0), Align2::CENTER_BOTTOM, label, font, line);
        } else {
            // Zu wenig Platz über der Linie: Beschriftung darunter mit dunklem Hintergrund.
            let text_rect = Rect::from_min_size(pos2(column.left(), y + 2.0), vec2(column.width(), 28.0));
            painter.rect_filled(text_rect, CornerRadius::same(2), Color32::from_black_alpha(200));
            painter.text(pos2(column.center().x, y + 3.0), Align2::CENTER_TOP, label, font, line);
        }
    }

    // Gelb und Rot: Linie über die ganze Anzeige plus Pfeil am Rand; die gegriffene etwas kräftiger.
    {
        for (db, color, line) in [(*yellow, YELLOW, MeterLine::Yellow), (*red, RED, MeterLine::Red)] {
            let y = to_y(db);
            let grabbed = active_line == Some(line);
            painter.line_segment(
                [pos2(inner.left(), y), pos2(inner.right(), y)],
                Stroke::new(if grabbed { 3.0 } else { 2.0 }, color),
            );
            let size = if grabbed { 6.0 } else { 5.0 };
            let tip = pos2(gutter.right() - 1.0, y);
            painter.add(Shape::convex_polygon(
                vec![tip, pos2(tip.x - 2.0 * size, y - size), pos2(tip.x - 2.0 * size, y + size)],
                color,
                Stroke::new(1.0, Color32::from_black_alpha(160)),
            ));
        }
    }

    let hint = match active_line {
        Some(MeterLine::Yellow) => {
            format!("{} {:.0} dB · {}", t("Gelb ab", "Yellow from"), *yellow, t("Pfeil ziehen", "drag the arrow"))
        }
        Some(MeterLine::Red) if beeps => format!(
            "{} {:.0} dB · {}",
            t("Rot und Warnton ab", "Red and beep from"),
            *red,
            t("Pfeil ziehen", "drag the arrow")
        ),
        Some(MeterLine::Red) => {
            format!("{} {:.0} dB · {}", t("Rot ab", "Red from"), *red, t("Pfeil ziehen", "drag the arrow"))
        }
        Some(MeterLine::Limit) if limit_running => {
            t("Limiter: Linie runterziehen, Doppelklick: aus.", "Limiter: drag the line down, double-click: off.").to_string()
        }
        Some(MeterLine::Limit) => t(
            "Limiter: wirkt erst mit einem Ausgang (VB-Cable). Doppelklick: aus.",
            "Limiter: only works with an output (VB-Cable). Double-click: off.",
        )
        .to_string(),
        None => String::new(),
    };
    response.on_hover_text(format!("{} {voice_db:.1} dB\n{hint}", t("Pegel", "Level")))
}

/// Umschalttaste mit eigener Farbe, wenn aktiv.
pub fn toggle_button(ui: &mut Ui, on: &mut bool, text: &str, active: Color32) -> Response {
    let fill = if *on { active } else { Color32::from_rgb(58, 63, 72) };
    let label = egui::RichText::new(text).color(Color32::WHITE).strong();
    let response = ui.add_sized(Vec2::new(56.0, 26.0), egui::Button::new(label).fill(fill));
    if response.clicked() {
        *on = !*on;
    }
    response
}

/// Mute-Taste, rot wenn aktiv.
pub fn mute_button(ui: &mut Ui, muted: &mut bool) -> Response {
    toggle_button(ui, muted, t("Mute", "Mute"), Color32::from_rgb(200, 45, 45))
}

pub const TITLE_BUTTON_WIDTH: f32 = 24.0;

#[derive(Clone, Copy, PartialEq)]
pub enum TitleIcon {
    Gear,
    Minimize,
    Close,
}

/// Kleiner Knopf für die eigene Titelzeile. Schließen wird beim Überfahren rot.
/// Minimieren und Schließen werden gezeichnet, nicht als Schriftzeichen: „✕“ fehlt in der Schrift.
pub fn title_button(ui: &mut Ui, icon: TitleIcon, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(TITLE_BUTTON_WIDTH, 24.0), Sense::click());
    let hovered = response.hovered();
    let background = match (hovered, icon, selected) {
        (true, TitleIcon::Close, _) => Some(Color32::from_rgb(200, 45, 45)),
        (true, _, _) | (false, _, true) => Some(Color32::from_rgb(70, 76, 88)),
        _ => None,
    };
    let painter = ui.painter();
    if let Some(color) = background {
        painter.rect_filled(rect, CornerRadius::same(4), color);
    }
    let color = if hovered || selected { Color32::WHITE } else { LABEL };
    let c = rect.center();
    let stroke = Stroke::new(1.6, color);
    match icon {
        TitleIcon::Gear => {
            painter.text(c, Align2::CENTER_CENTER, "⚙", FontId::proportional(16.0), color);
        }
        TitleIcon::Minimize => {
            painter.line_segment([pos2(c.x - 5.0, c.y + 1.0), pos2(c.x + 5.0, c.y + 1.0)], stroke);
        }
        TitleIcon::Close => {
            painter.line_segment([pos2(c.x - 4.5, c.y - 4.5), pos2(c.x + 4.5, c.y + 4.5)], stroke);
            painter.line_segment([pos2(c.x - 4.5, c.y + 4.5), pos2(c.x + 4.5, c.y - 4.5)], stroke);
        }
    }
    response
}

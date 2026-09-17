//! Bedienelemente im Stil eines Mischpult-Kanalzugs: Drehknopf, Fader, Pegelanzeige.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2,
};

pub const PANEL: Color32 = Color32::from_rgb(40, 44, 52);
pub const ACCENT: Color32 = Color32::from_rgb(90, 200, 120);
const TRACK: Color32 = Color32::from_rgb(22, 24, 28);
const LABEL: Color32 = Color32::from_rgb(170, 176, 186);

/// Drehknopf von 0 bis `max`. Ziehen nach oben/unten oder Mausrad ändert, Doppelklick setzt auf 0.
pub fn knob(ui: &mut Ui, value: &mut f32, max: f32, label: &str, lit: bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(64.0, 92.0), Sense::click_and_drag());

    let mut new = *value;
    if response.dragged() {
        new -= response.drag_delta().y * max / 150.0;
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        new += scroll / 40.0 * max / 20.0;
    }
    if response.double_clicked() {
        new = 0.0;
    }
    let new = new.clamp(0.0, max);
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
    let t = if max > 0.0 { *value / max } else { 0.0 };
    if t > 0.0 {
        painter.add(Shape::line(arc(start, start + sweep * t), Stroke::new(4.0, ACCENT)));
    }
    let pointer = start + sweep * t;
    painter.line_segment(
        [center + vec2(pointer.cos(), pointer.sin()) * 6.0, center + vec2(pointer.cos(), pointer.sin()) * 15.0],
        Stroke::new(2.0, Color32::WHITE),
    );
    painter.text(center + vec2(0.0, radius + 3.0), Align2::CENTER_TOP, format!("{:.1}", *value), FontId::proportional(11.0), Color32::WHITE);

    // Beschriftung, bei Bedarf mit kleiner Lampe (z.B. Gate offen).
    let label_pos = pos2(rect.center().x, rect.bottom() - 2.0);
    painter.text(label_pos, Align2::CENTER_BOTTOM, label, FontId::proportional(12.0), LABEL);
    if lit {
        painter.circle_filled(pos2(rect.right() - 6.0, rect.bottom() - 8.0), 3.5, ACCENT);
    }

    response
}

/// Senkrechter Fader in dB. Ziehen, Mausrad; Doppelklick setzt auf 0 dB.
pub fn fader(ui: &mut Ui, value: &mut f32, min: f32, max: f32, height: f32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(56.0, height), Sense::click_and_drag());
    let top = rect.top() + 18.0;
    let bottom = rect.bottom() - 18.0;
    let to_y = |v: f32| bottom - (v - min) / (max - min) * (bottom - top);

    let mut new = *value;
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        new = min + (bottom - pos.y) / (bottom - top) * (max - min);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        new += scroll / 40.0 * 0.5;
    }
    if response.double_clicked() {
        new = 0.0;
    }
    let new = ((new * 10.0).round() / 10.0).clamp(min, max);
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
    let text = if value.abs() < 0.05 { "0dB".to_string() } else { format!("{:.0}dB", *value) };
    painter.text(knob_center, Align2::CENTER_CENTER, text, FontId::proportional(11.0), Color32::from_rgb(30, 30, 34));

    response
}

pub const METER_MIN_DB: f32 = -60.0;
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

/// Pegelanzeige mit zwei Balken, -60 bis 0 dB.
/// Links deine Stimme, rechts der Ausgang mit der gelben Limiter-Linie.
/// Mit `thresholds` (Gelb, Rot) liegen über dem Stimm-Balken zwei greifbare Linien.
/// Der Limiter ist auf 0 dB aus und erscheint dann nur, wenn die Maus über seinem Balken ist.
pub fn level_meter(
    ui: &mut Ui,
    voice_db: f32,
    out_db: f32,
    out_peak_db: f32,
    limit_db: &mut f32,
    thresholds: Option<(&mut f32, &mut f32)>,
    height: f32,
) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(56.0, height), Sense::click_and_drag());
    let meter = rect;
    let inner = meter.shrink(3.0);
    let gap = 4.0;
    let bar_width = (inner.width() - gap) / 2.0;
    let columns = [
        Rect::from_min_size(inner.min, vec2(bar_width, inner.height())),
        Rect::from_min_size(pos2(inner.left() + bar_width + gap, inner.top()), vec2(bar_width, inner.height())),
    ];
    let to_y = |db: f32| inner.bottom() - ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0) * inner.height();
    let to_db = |y: f32| METER_MIN_DB + (inner.bottom() - y) / inner.height() * -METER_MIN_DB;

    // Links die nähere der beiden Linien greifen, rechts den Limiter.
    let lines_y = thresholds.as_ref().map(|(yellow, red)| (to_y(**yellow), to_y(**red)));
    let line_at = |p: Pos2| {
        if p.x >= inner.center().x {
            return Some(MeterLine::Limit);
        }
        let (yellow_y, red_y) = lines_y?;
        Some(if (p.y - red_y).abs() <= (p.y - yellow_y).abs() { MeterLine::Red } else { MeterLine::Yellow })
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
    let mut thresholds = thresholds;
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
            if let (Some(db), Some((yellow, red))) = (pointer_db, thresholds.as_mut()) {
                // Gelb bleibt unter Rot.
                let new = db.round().clamp(METER_MIN_DB, **red) + 0.0;
                changed |= new != **yellow;
                **yellow = new;
            }
        }
        Some(MeterLine::Red) => {
            if let (Some(db), Some((yellow, red))) = (pointer_db, thresholds.as_mut()) {
                let new = db.round().clamp(**yellow, 0.0) + 0.0;
                changed |= new != **red;
                **red = new;
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

    const SEGMENTS: usize = 40;
    let segment_height = inner.height() / SEGMENTS as f32;
    for (column, level) in columns.iter().zip([voice_db, out_db]) {
        for i in 0..SEGMENTS {
            let db = METER_MIN_DB + (i as f32 + 0.5) / SEGMENTS as f32 * -METER_MIN_DB;
            let color = if db > -3.0 {
                RED
            } else if db > -12.0 {
                YELLOW
            } else {
                ACCENT
            };
            let y1 = inner.bottom() - i as f32 * segment_height;
            let segment = Rect::from_min_max(pos2(column.left(), y1 - segment_height + 1.0), pos2(column.right(), y1));
            painter.rect_filled(segment, CornerRadius::ZERO, if db <= level { color } else { color.gamma_multiply(0.12) });
        }
    }
    if out_peak_db > METER_MIN_DB {
        let y = to_y(out_peak_db);
        painter.line_segment([pos2(columns[1].left(), y), pos2(columns[1].right(), y)], Stroke::new(2.0, Color32::WHITE));
    }

    // Linien für Gelb und Rot über dem Stimm-Balken; die gegriffene etwas dicker.
    if let Some((yellow, red)) = thresholds.as_ref() {
        for (db, color, line) in [(**yellow, YELLOW, MeterLine::Yellow), (**red, RED, MeterLine::Red)] {
            let y = to_y(db);
            let width = if active_line == Some(line) { 4.0 } else { 3.0 };
            painter.line_segment([pos2(columns[0].left(), y), pos2(columns[0].right(), y)], Stroke::new(width, color));
        }
    }

    let limit_active = *limit_db < LIMIT_OFF_DB;
    if limit_active || active_line == Some(MeterLine::Limit) {
        let column = columns[1];
        let y = to_y(*limit_db);
        let line = Color32::from_rgb(225, 205, 70);
        let area = Rect::from_min_max(column.min, pos2(column.right(), y));
        painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(90, 80, 10, 225));
        painter.line_segment([pos2(column.left(), y), pos2(column.right(), y)], Stroke::new(2.0, line));
        let value = if limit_active { format!("{:.0}", *limit_db) } else { "aus".to_string() };
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

    let hint = match (active_line, thresholds.as_ref()) {
        (Some(MeterLine::Yellow), Some((yellow, _))) => format!("Gelb ab {:.0} dB · Linie ziehen", **yellow),
        (Some(MeterLine::Red), Some((_, red))) => format!("Rot und Warnton ab {:.0} dB · Linie ziehen", **red),
        (Some(MeterLine::Limit), _) => {
            "Rechts: was rausgeht. Gelbe Linie runterziehen = Limiter, Doppelklick: aus.".to_string()
        }
        _ => String::new(),
    };
    response.on_hover_text(format!("Stimme {voice_db:.1} dB · Ausgang {out_db:.1} dB\n{hint}"))
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
    toggle_button(ui, muted, "Mute", Color32::from_rgb(200, 45, 45))
}

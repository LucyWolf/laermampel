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
pub const LIMIT_DEFAULT_DB: f32 = -1.0;

/// Senkrechte Pegelanzeige aus Segmenten, -60 bis 0 dB, mit Spitzenwert und greifbarem Limiter.
/// Die Limiter-Linie wird mit der Maus verschoben, Doppelklick setzt sie zurück.
pub fn level_meter(ui: &mut Ui, level_db: f32, peak_db: f32, limit_db: &mut f32, height: f32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(44.0, height), Sense::click_and_drag());
    let inner = rect.shrink(3.0);
    let to_y = |db: f32| inner.bottom() - ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0) * inner.height();

    let mut new = *limit_db;
    if response.dragged()
        && let Some(pos) = response.interact_pointer_pos()
    {
        new = METER_MIN_DB + (inner.bottom() - pos.y) / inner.height() * -METER_MIN_DB;
    }
    if response.double_clicked() {
        new = LIMIT_DEFAULT_DB;
    }
    let new = new.round().clamp(LIMIT_MIN_DB, 0.0);
    if new != *limit_db {
        *limit_db = new;
        response.mark_changed();
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), TRACK);

    // Zwei Balken nebeneinander wie beim Mischpult.
    const SEGMENTS: usize = 40;
    let gap = 3.0;
    let bar_width = (inner.width() - gap) / 2.0;
    let segment_height = inner.height() / SEGMENTS as f32;
    for i in 0..SEGMENTS {
        let db = METER_MIN_DB + (i as f32 + 0.5) / SEGMENTS as f32 * -METER_MIN_DB;
        let color = if db > -3.0 {
            Color32::from_rgb(235, 45, 45)
        } else if db > -12.0 {
            Color32::from_rgb(245, 190, 20)
        } else {
            ACCENT
        };
        let lit = db <= level_db;
        let y1 = inner.bottom() - i as f32 * segment_height;
        for column in 0..2 {
            let x0 = inner.left() + column as f32 * (bar_width + gap);
            let segment = Rect::from_min_max(pos2(x0, y1 - segment_height + 1.0), pos2(x0 + bar_width, y1));
            painter.rect_filled(segment, CornerRadius::ZERO, if lit { color } else { color.gamma_multiply(0.12) });
        }
    }

    if peak_db > METER_MIN_DB {
        let y = to_y(peak_db);
        painter.line_segment([pos2(inner.left(), y), pos2(inner.right(), y)], Stroke::new(2.0, Color32::WHITE));
    }

    // Limiter: alles oberhalb der Linie wird nicht durchgelassen.
    let limit_y = to_y(*limit_db);
    let limit_color = Color32::from_rgb(225, 205, 70);
    let area = Rect::from_min_max(inner.min, pos2(inner.right(), limit_y));
    painter.rect_filled(area, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(90, 80, 10, 225));
    painter.line_segment([pos2(inner.left(), limit_y), pos2(inner.right(), limit_y)], Stroke::new(2.0, limit_color));

    let label = format!("Lim\n{:.0}", *limit_db);
    let font = FontId::proportional(12.0);
    if area.height() >= 32.0 {
        painter.text(pos2(inner.center().x, limit_y - 3.0), Align2::CENTER_BOTTOM, label, font, limit_color);
    } else {
        // Zu wenig Platz über der Linie: Beschriftung darunter mit dunklem Hintergrund.
        let text_rect = Rect::from_min_size(pos2(inner.left(), limit_y + 2.0), vec2(inner.width(), 30.0));
        painter.rect_filled(text_rect, CornerRadius::same(2), Color32::from_black_alpha(200));
        painter.text(pos2(inner.center().x, limit_y + 3.0), Align2::CENTER_TOP, label, font, limit_color);
    }

    response.on_hover_text(format!("Pegel {level_db:.1} dB · Limiter {:.0} dB (ziehen, Doppelklick: {LIMIT_DEFAULT_DB:.0} dB)", *limit_db))
}

/// Mute-Taste, rot wenn aktiv.
pub fn mute_button(ui: &mut Ui, muted: &mut bool) -> Response {
    let fill = if *muted { Color32::from_rgb(200, 45, 45) } else { Color32::from_rgb(58, 63, 72) };
    let text = egui::RichText::new("Mute").color(Color32::WHITE).strong();
    let response = ui.add_sized(Vec2::new(56.0, 26.0), egui::Button::new(text).fill(fill));
    if response.clicked() {
        *muted = !*muted;
    }
    response
}

//! Wo die Anzeige auf dem Bildschirm sitzt: Monitor, Ecke/Mitte, Abstand zum Rand.

use serde::{Deserialize, Serialize};

use crate::lang::t;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    pub const TOP_ROW: [Anchor; 3] = [Anchor::TopLeft, Anchor::TopCenter, Anchor::TopRight];
    pub const BOTTOM_ROW: [Anchor; 3] = [Anchor::BottomLeft, Anchor::BottomCenter, Anchor::BottomRight];

    pub fn label(self) -> &'static str {
        match self {
            Anchor::TopLeft => t("↖ Oben links", "↖ Top left"),
            Anchor::TopCenter => t("↑ Oben Mitte", "↑ Top centre"),
            Anchor::TopRight => t("↗ Oben rechts", "↗ Top right"),
            Anchor::BottomLeft => t("↙ Unten links", "↙ Bottom left"),
            Anchor::BottomCenter => t("↓ Unten Mitte", "↓ Bottom centre"),
            Anchor::BottomRight => t("↘ Unten rechts", "↘ Bottom right"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Monitor {
    /// Arbeitsbereich ohne Taskleiste in physischen Pixeln: links, oben, rechts, unten.
    pub work: [i32; 4],
    /// Windows-Skalierung, 1.0 = 100 %.
    pub scale: f32,
    pub primary: bool,
}

impl Monitor {
    pub fn label(&self, index: usize) -> String {
        let width = self.work[2] - self.work[0];
        let height = self.work[3] - self.work[1];
        let primary = if self.primary { t(", Hauptbildschirm", ", main screen") } else { "" };
        format!("{} {} ({width}×{height}{primary})", t("Monitor", "Monitor"), index + 1)
    }
}

/// Physisches Rechteck x, y, Breite, Höhe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Berechnet die Fensterposition. `size` und `margin` sind in logischen Pixeln (bei 100 %).
pub fn target_rect(monitors: &[Monitor], index: usize, anchor: Anchor, size: [f32; 2], margin: f32) -> Option<PhysicalRect> {
    // Ist der gewählte Monitor gerade nicht angeschlossen, auf den Hauptbildschirm ausweichen.
    let m = monitors.get(index).or_else(|| monitors.first())?;
    let w = (size[0] * m.scale).round() as i32;
    let h = (size[1] * m.scale).round() as i32;
    let margin = (margin * m.scale).round() as i32;
    let [left, top, right, bottom] = m.work;

    let x = match anchor {
        Anchor::TopLeft | Anchor::BottomLeft => left + margin,
        Anchor::TopCenter | Anchor::BottomCenter => left + (right - left - w) / 2,
        Anchor::TopRight | Anchor::BottomRight => right - margin - w,
    };
    let y = match anchor {
        Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => top + margin,
        _ => bottom - margin - h,
    };
    Some(PhysicalRect { x, y, w, h })
}

#[cfg(windows)]
pub use win::{apply, monitors};

#[cfg(windows)]
mod win {
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    use eframe::egui::Context;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, LWA_ALPHA, MONITORINFOF_PRIMARY, SWP_NOACTIVATE,
        SetLayeredWindowAttributes, SetWindowPos, WS_EX_LAYERED,
    };
    use windows_sys::core::BOOL;

    use super::{Monitor, PhysicalRect};

    pub fn monitors() -> Vec<Monitor> {
        let mut list: Vec<Monitor> = Vec::new();
        unsafe {
            EnumDisplayMonitors(null_mut(), null(), Some(collect), &mut list as *mut Vec<Monitor> as LPARAM);
        }
        // Hauptbildschirm ist Monitor 1, der Rest von links nach rechts.
        list.sort_by_key(|m| (!m.primary, m.work[0], m.work[1]));
        list
    }

    unsafe extern "system" fn collect(monitor: HMONITOR, _hdc: HDC, _clip: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let list = &mut *(data as *mut Vec<Monitor>);
            let mut info: MONITORINFO = zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) != 0 {
                let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
                let scale = if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == 0 {
                    dpi_x as f32 / 96.0
                } else {
                    1.0
                };
                let r = info.rcWork;
                list.push(Monitor {
                    work: [r.left, r.top, r.right, r.bottom],
                    scale,
                    primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
        }
        1
    }

    /// Schiebt das Fenster an die Zielposition und holt es wieder nach ganz vorne.
    pub fn apply(frame: &eframe::Frame, rect: PhysicalRect, _ctx: &Context) {
        let Ok(handle) = frame.window_handle() else { return };
        let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
        let hwnd = win.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        unsafe {
            // Für durchklickbare Fenster setzt winit WS_EX_LAYERED, aber keine Deckkraft.
            // Ein solches Fenster zeichnet Windows überhaupt nicht, also volle Deckkraft setzen.
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_LAYERED != 0 {
                SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
            }
            SetWindowPos(hwnd, HWND_TOPMOST, rect.x, rect.y, rect.w, rect.h, SWP_NOACTIVATE);
        }
    }
}

#[cfg(not(windows))]
pub fn monitors() -> Vec<Monitor> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn apply(_frame: &eframe::Frame, rect: PhysicalRect, ctx: &eframe::egui::Context) {
    use eframe::egui::{ViewportCommand, pos2, vec2};
    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos2(rect.x as f32, rect.y as f32)));
    ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(rect.w as f32, rect.h as f32)));
}

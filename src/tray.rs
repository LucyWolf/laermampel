//! Symbol im Infobereich neben der Uhr. Es zeigt die aktuelle Ampelfarbe.

use crate::level::Zone;

#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayAction {
    OpenSettings,
    ToggleMute,
    Quit,
}

#[cfg(windows)]
mod imp {
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{TrayAction, Zone};
    use crate::app::zone_rgb;

    const ICON_SIZE: u32 = 32;

    pub struct Tray {
        icon: TrayIcon,
        settings_id: MenuId,
        mute: CheckMenuItem,
        quit_id: MenuId,
        state: (Zone, bool),
    }

    impl Tray {
        pub fn new() -> Option<Self> {
            let settings = MenuItem::new("Einstellungen", true, None);
            let mute = CheckMenuItem::new("Mikrofon stumm", true, false, None);
            let quit = MenuItem::new("Beenden", true, None);
            let menu = Menu::new();
            menu.append_items(&[&settings, &mute, &PredefinedMenuItem::separator(), &quit]).ok()?;

            let icon = TrayIconBuilder::new()
                .with_tooltip("Lärmampel")
                .with_icon(circle_icon(Zone::Green, false))
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .ok()?;

            Some(Self {
                icon,
                settings_id: settings.id().clone(),
                mute,
                quit_id: quit.id().clone(),
                state: (Zone::Green, false),
            })
        }

        pub fn poll(&self) -> Vec<TrayAction> {
            let mut actions = Vec::new();
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == self.settings_id {
                    actions.push(TrayAction::OpenSettings);
                } else if &event.id == self.mute.id() {
                    actions.push(TrayAction::ToggleMute);
                } else if event.id == self.quit_id {
                    actions.push(TrayAction::Quit);
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                    actions.push(TrayAction::OpenSettings);
                }
            }
            actions
        }

        pub fn set_state(&mut self, zone: Zone, muted: bool) {
            if (zone, muted) != self.state {
                self.state = (zone, muted);
                let _ = self.icon.set_icon(Some(circle_icon(zone, muted)));
                self.mute.set_checked(muted);
            }
        }
    }

    /// Kreis in der Ampelfarbe; stumm: grau mit rotem Ring.
    fn circle_icon(zone: Zone, muted: bool) -> Icon {
        let [r, g, b] = if muted { [120, 120, 126] } else { zone_rgb(zone) };
        let size = ICON_SIZE as f32;
        let radius = size / 2.0 - 2.0;
        let mut rgba = Vec::with_capacity((ICON_SIZE * ICON_SIZE * 4) as usize);
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let dx = x as f32 + 0.5 - size / 2.0;
                let dy = y as f32 + 0.5 - size / 2.0;
                let dist = (dx * dx + dy * dy).sqrt();
                // Weiche Kante, damit der Kreis nicht pixelig wirkt.
                let alpha = (radius + 0.5 - dist).clamp(0.0, 1.0);
                // Dunkler Rand, damit Gelb auch auf heller Taskleiste sichtbar bleibt.
                let edge = ((dist - (radius - 2.0)).clamp(0.0, 1.0) * 0.45).min(1.0);
                let shade = |c: u8| (c as f32 * (1.0 - edge)) as u8;
                let pixel = if muted && dist > radius - 4.0 {
                    [220, 40, 40]
                } else {
                    [shade(r), shade(g), shade(b)]
                };
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], (alpha * 255.0) as u8]);
            }
        }
        Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).expect("Icongröße passt")
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{TrayAction, Zone};

    pub struct Tray;

    impl Tray {
        pub fn new() -> Option<Self> {
            None
        }

        pub fn poll(&self) -> Vec<TrayAction> {
            Vec::new()
        }

        pub fn set_state(&mut self, _zone: Zone, _muted: bool) {}
    }
}

pub use imp::Tray;

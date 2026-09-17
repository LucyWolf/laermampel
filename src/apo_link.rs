#![cfg_attr(not(windows), allow(dead_code))]
//! Verbindung zum laufenden Audio-Filter: Einstellungen hinschicken, Pegel und Lebenszeichen lesen.

use std::time::{Duration, Instant};

/// Bleibt das Lebenszeichen so lange stehen, gilt der Filter als nicht aktiv.
const ALIVE_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

pub struct ApoLink {
    #[cfg(windows)]
    mapping: Option<laermampel_apo::shared::Mapping>,
    last_try: Option<Instant>,
    last_beat: u32,
    last_beat_change: Option<Instant>,
}

impl ApoLink {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            mapping: None,
            last_try: None,
            last_beat: 0,
            last_beat_change: None,
        }
    }

    /// Einmal pro Bild aufrufen.
    pub fn tick(&mut self) {
        #[cfg(windows)]
        {
            use laermampel_apo::shared::{Mapping, VERSION};

            if self.mapping.is_none() && self.last_try.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL) {
                self.last_try = Some(Instant::now());
                self.mapping = Mapping::open().filter(|m| m.params().is_valid() && m.params().version() == VERSION);
                if self.mapping.is_some() {
                    log!("APO: Verbindung zum Filter hergestellt");
                }
            }
            if let Some(mapping) = &self.mapping {
                let beat = mapping.params().heartbeat();
                if beat != self.last_beat {
                    self.last_beat = beat;
                    self.last_beat_change = Some(Instant::now());
                }
            }
        }
    }

    /// Verarbeitet der Filter gerade wirklich Audio?
    pub fn active(&self) -> bool {
        self.last_beat_change.is_some_and(|t| t.elapsed() < ALIVE_TIMEOUT)
    }

    pub fn send(&self, gain_db: f32, muted: bool) {
        #[cfg(windows)]
        if let Some(mapping) = &self.mapping {
            mapping.params().set_gain_db(gain_db);
            mapping.params().set_muted(muted);
        }
        #[cfg(not(windows))]
        let _ = (gain_db, muted);
    }

    /// Pegel vor dem Filter, also deine echte Lautstärke.
    pub fn input_level_db(&self) -> Option<f32> {
        #[cfg(windows)]
        if self.active() {
            return self.mapping.as_ref().map(|m| m.params().input_level_db());
        }
        None
    }
}

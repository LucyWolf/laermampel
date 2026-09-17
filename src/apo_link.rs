#![cfg_attr(not(windows), allow(dead_code))]
//! Verbindung zum laufenden Audio-Filter: Einstellungen hinschicken, Pegel und Lebenszeichen lesen.

use std::time::{Duration, Instant};

use laermampel_apo::dsp::Settings;
use laermampel_apo::shared::Feedback;

/// Bleibt das Lebenszeichen so lange stehen, gilt der Filter als nicht aktiv.
const ALIVE_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

pub struct ApoLink {
    #[cfg(windows)]
    mapping: Option<laermampel_apo::shared::Mapping>,
    last_try: Option<Instant>,
    last_beat: u32,
    last_beat_change: Option<Instant>,
    /// Ein Filter läuft, aber mit altem Aufbau des gemeinsamen Speichers: neu einrichten.
    outdated: bool,
}

impl ApoLink {
    pub fn new() -> Self {
        Self {
            #[cfg(windows)]
            mapping: None,
            last_try: None,
            last_beat: 0,
            last_beat_change: None,
            outdated: false,
        }
    }

    /// Einmal pro Bild aufrufen.
    pub fn tick(&mut self) {
        #[cfg(windows)]
        {
            use laermampel_apo::shared::{Mapping, VERSION};

            if self.mapping.is_none() && self.last_try.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL) {
                self.last_try = Some(Instant::now());
                let opened = Mapping::open().filter(|m| m.params().is_valid());
                self.outdated = opened.as_ref().is_some_and(|m| m.params().version() != VERSION);
                self.mapping = opened.filter(|m| m.params().version() == VERSION);
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

    pub fn outdated(&self) -> bool {
        self.outdated
    }

    pub fn send(&self, settings: &Settings) {
        #[cfg(windows)]
        if let Some(mapping) = &self.mapping {
            mapping.params().set_settings(settings);
        }
        #[cfg(not(windows))]
        let _ = settings;
    }

    /// Rückmeldung des Filters, nur solange er wirklich läuft.
    pub fn feedback(&self) -> Option<Feedback> {
        #[cfg(windows)]
        if self.active() {
            return self.mapping.as_ref().map(|m| m.params().feedback());
        }
        None
    }
}

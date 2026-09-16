//! Aus den rohen Blockpegeln wird eine ruhige, aber schnelle Anzeige:
//! sofort hoch, langsam runter, Gelb/Rot bleiben kurz stehen.

use std::time::{Duration, Instant};

use crate::audio::SILENCE_DB;
use crate::settings::Settings;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Zone {
    Green,
    Yellow,
    Red,
}

pub struct Level {
    pub display_db: f32,
    target_db: f32,
    held: Zone,
    held_until: Option<Instant>,
    pub zone: Zone,
}

impl Level {
    pub fn new() -> Self {
        Self {
            display_db: SILENCE_DB,
            target_db: SILENCE_DB,
            held: Zone::Green,
            held_until: None,
            zone: Zone::Green,
        }
    }

    /// Gibt `true` zurück, wenn die Anzeige gerade neu auf Rot gesprungen ist.
    pub fn update(&mut self, input_db: Option<f32>, dt: f32, now: Instant, s: &Settings) -> bool {
        if let Some(db) = input_db {
            self.target_db = db;
        }

        let tau_ms = if self.target_db > self.display_db { s.attack_ms } else { s.release_ms };
        if tau_ms <= 0.0 {
            self.display_db = self.target_db;
        } else {
            let k = 1.0 - (-dt * 1000.0 / tau_ms).exp();
            self.display_db += (self.target_db - self.display_db) * k;
        }

        let current = if self.display_db >= s.red_db {
            Zone::Red
        } else if self.display_db >= s.yellow_db {
            Zone::Yellow
        } else {
            Zone::Green
        };

        let holding = self.held_until.is_some_and(|t| now < t);
        if current > Zone::Green && (!holding || current >= self.held) {
            self.held = current;
            self.held_until = Some(now + Duration::from_secs_f32(s.hold_ms.max(0.0) / 1000.0));
        }
        let holding = self.held_until.is_some_and(|t| now < t);
        let zone = if holding { current.max(self.held) } else { current };

        let became_red = zone == Zone::Red && self.zone != Zone::Red;
        self.zone = zone;
        became_red
    }
}

pub const CALIBRATION_SECONDS: f32 = 6.0;

/// Nimmt ein paar Sekunden normale Sprache auf und ermittelt deren typischen Pegel.
pub struct Calibration {
    started: Instant,
    samples: Vec<f32>,
}

impl Calibration {
    pub fn new() -> Self {
        Self { started: Instant::now(), samples: Vec::new() }
    }

    pub fn push(&mut self, db: f32) {
        self.samples.push(db);
    }

    pub fn progress(&self) -> f32 {
        (self.started.elapsed().as_secs_f32() / CALIBRATION_SECONDS).min(1.0)
    }

    pub fn done(&self) -> bool {
        self.progress() >= 1.0
    }

    /// Pegel der normalen Stimme in dBFS.
    pub fn result(&self) -> Result<f32, &'static str> {
        let mut sorted = self.samples.clone();
        sorted.sort_by(f32::total_cmp);
        if sorted.len() < 20 {
            return Err("Zu wenig Messwerte, bitte nochmal versuchen.");
        }
        // Die leisesten Momente sind die Pausen zwischen den Wörtern.
        let noise = sorted[sorted.len() / 10];
        let speech: Vec<f32> = sorted.into_iter().filter(|&db| db > noise + 10.0).collect();
        if speech.len() < 15 {
            return Err("Keine Sprache erkannt. Bitte während der Messung normal reden.");
        }
        Ok(speech[speech.len() / 2])
    }
}

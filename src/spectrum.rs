//! Frequenz-Diagramm des Mikrofons und ein Rauschprofil zum Vergleich.
//!
//! Die Aufnahme legt die letzten Samples in einen Ringpuffer; hier wird daraus alle paar
//! Bilder ein Spektrum gerechnet (FFT). Nur zum Anschauen: so ist zu sehen, wo das Rauschen
//! sitzt und wie viel es ist.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::audio::{RawSamples, SAMPLE_RING};

/// Auflösung: bei 48 kHz sind das ~47 Hz pro Balken.
const FFT_SIZE: usize = SAMPLE_RING;
pub const BINS: usize = FFT_SIZE / 2;
/// So oft wird neu gerechnet; öfter bringt fürs Auge nichts.
const INTERVAL: Duration = Duration::from_millis(60);
/// Glättung der Anzeige über die Zeit.
const SMOOTHING: f32 = 0.3;
/// So lange wird für ein Rauschprofil gemittelt.
const PROFILE_SECONDS: f32 = 2.0;

pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex32>,
    /// Geglättete Pegel pro Frequenzband in dB.
    bands_db: Vec<f32>,
    ready: bool,
    last: Instant,
    sample_rate: f32,

    profile_db: Option<Vec<f32>>,
    profile_sum: Vec<f32>,
    profile_count: u32,
    profile_until: Option<Instant>,
}

impl Spectrum {
    pub fn new() -> Self {
        Self {
            fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
            // Hann-Fenster, sonst „schmiert“ jede Frequenz über das ganze Bild.
            window: (0..FFT_SIZE)
                .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos())
                .collect(),
            scratch: vec![Complex32::new(0.0, 0.0); FFT_SIZE],
            bands_db: vec![-120.0; BINS],
            ready: false,
            last: Instant::now(),
            sample_rate: 48_000.0,
            profile_db: None,
            profile_sum: vec![0.0; BINS],
            profile_count: 0,
            profile_until: None,
        }
    }

    pub fn bands_db(&self) -> Option<&[f32]> {
        self.ready.then_some(&self.bands_db[..])
    }

    pub fn profile_db(&self) -> Option<&[f32]> {
        self.profile_db.as_deref()
    }

    pub fn profile_running(&self) -> bool {
        self.profile_until.is_some()
    }

    /// Frequenz in der Mitte eines Bandes.
    pub fn frequency(&self, bin: usize) -> f32 {
        bin as f32 * self.sample_rate / FFT_SIZE as f32
    }

    pub fn start_profile(&mut self) {
        self.profile_sum.fill(0.0);
        self.profile_count = 0;
        self.profile_until = Some(Instant::now() + Duration::from_secs_f32(PROFILE_SECONDS));
    }

    pub fn clear_profile(&mut self) {
        self.profile_db = None;
        self.profile_until = None;
    }

    /// Einmal pro Bild aufrufen; rechnet höchstens alle `INTERVAL`.
    pub fn update(&mut self, raw: Option<&RawSamples>, sample_rate: f32) {
        let Some(raw) = raw else { return };
        if self.last.elapsed() < INTERVAL {
            return;
        }
        self.last = Instant::now();
        self.sample_rate = sample_rate.max(8000.0);

        let Some(samples) = raw.snapshot() else { return };
        for (slot, (&x, &w)) in self.scratch.iter_mut().zip(samples.iter().zip(&self.window)) {
            *slot = Complex32::new(x * w, 0.0);
        }
        self.fft.process(&mut self.scratch);

        // Betrag in dBFS, auf das Fenster normiert.
        let scale = 2.0 / FFT_SIZE as f32;
        for (band, value) in self.bands_db.iter_mut().zip(&self.scratch[..BINS]) {
            let db = 20.0 * (value.norm() * scale).max(1e-7).log10();
            *band = if self.ready { *band + (db - *band) * SMOOTHING } else { db };
        }
        self.ready = true;

        if let Some(until) = self.profile_until {
            for (sum, &db) in self.profile_sum.iter_mut().zip(&self.bands_db) {
                *sum += db;
            }
            self.profile_count += 1;
            if Instant::now() >= until && self.profile_count > 0 {
                let count = self.profile_count as f32;
                self.profile_db = Some(self.profile_sum.iter().map(|sum| sum / count).collect());
                self.profile_until = None;
            }
        }
    }
}

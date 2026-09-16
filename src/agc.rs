//! Automatische Lautstärke: gleicht das Mikrofon an und gibt es auf ein virtuelles
//! Audiogerät (VB-Cable) aus, das andere Programme als Mikrofon benutzen.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use ringbuf::HeapCons;
use ringbuf::traits::{Consumer, Observer};

use crate::audio::InputDevice;

pub const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";
/// Name des Wiedergabegeräts von VB-Cable. Programme nehmen dann „CABLE Output“ als Mikrofon.
const VB_CABLE_HINT: &str = "cable input";

/// Wie viel Puffer zwischen Mikrofon und Ausgabe angestrebt wird.
const TARGET_BUFFER_SECONDS: f32 = 0.03;
const MAX_BUFFER_SECONDS: f32 = 0.2;
/// Die beiden Geräte laufen nie exakt gleich schnell, das wird hier sanft ausgeglichen.
const MAX_DRIFT_CORRECTION: f64 = 0.005;

/// Neue Werte werden nur alle so viele Samples übernommen.
const PARAM_RELOAD_SAMPLES: usize = 256;
const LEVEL_WINDOW_MS: f32 = 50.0;
const LIMITER_RELEASE_MS: f32 = 100.0;
/// Liegt der Pegel so weit unter dem letzten Höchstwert, klingt gerade ein Wort aus.
const TAIL_DB: f32 = 10.0;
const PEAK_DECAY_DB_PER_SECOND: f32 = 10.0;

/// Ein f32, das sich ohne Sperre zwischen UI und Audio-Thread teilen lässt.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// Einstellungen, die live aus der Oberfläche übernommen werden.
#[derive(Default)]
pub struct AgcParams {
    pub target_db: AtomicF32,
    pub max_gain_db: AtomicF32,
    pub max_cut_db: AtomicF32,
    pub attack_ms: AtomicF32,
    pub release_ms: AtomicF32,
    pub gate_db: AtomicF32,
    pub ceiling_db: AtomicF32,
    /// Rückmeldung an die Oberfläche: aktuelle Anpassung in dB.
    pub current_gain_db: AtomicF32,
}

pub fn list_output_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| id.clone());
            Some(InputDevice { id, name })
        })
        .collect()
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn smoothing(ms: f32, sample_rate: f32) -> f32 {
    if ms <= 0.0 { 1.0 } else { 1.0 - (-1000.0 / (ms * sample_rate)).exp() }
}

/// Regelt Sample für Sample: leise hoch, laut runter, Spitzen hart begrenzen.
pub struct Agc {
    sample_rate: f32,
    params: Arc<AgcParams>,
    countdown: usize,

    level_k: f32,
    attack_k: f32,
    release_k: f32,
    limiter_k: f32,
    target_db: f32,
    max_gain_db: f32,
    max_cut_db: f32,
    gate_db: f32,
    ceiling: f32,

    power: f32,
    recent_peak_db: f32,
    gain_db: f32,
    limiter_env: f32,
}

impl Agc {
    pub fn new(sample_rate: f32, params: Arc<AgcParams>) -> Self {
        Self {
            sample_rate,
            params,
            countdown: 0,
            level_k: smoothing(LEVEL_WINDOW_MS, sample_rate),
            attack_k: 0.0,
            release_k: 0.0,
            limiter_k: smoothing(LIMITER_RELEASE_MS, sample_rate),
            target_db: 0.0,
            max_gain_db: 0.0,
            max_cut_db: 0.0,
            gate_db: 0.0,
            ceiling: 1.0,
            power: 0.0,
            recent_peak_db: -120.0,
            gain_db: 0.0,
            limiter_env: 0.0,
        }
    }

    fn reload(&mut self) {
        let p = &self.params;
        self.attack_k = smoothing(p.attack_ms.get(), self.sample_rate);
        self.release_k = smoothing(p.release_ms.get(), self.sample_rate);
        self.target_db = p.target_db.get();
        self.max_gain_db = p.max_gain_db.get().max(0.0);
        self.max_cut_db = p.max_cut_db.get().max(0.0);
        self.gate_db = p.gate_db.get();
        self.ceiling = db_to_gain(p.ceiling_db.get().min(0.0));
        p.current_gain_db.set(self.gain_db);
    }

    pub fn process(&mut self, x: f32) -> f32 {
        if self.countdown == 0 {
            self.reload();
            self.countdown = PARAM_RELOAD_SAMPLES;
        }
        self.countdown -= 1;

        self.power += (x * x - self.power) * self.level_k;
        let level_db = 10.0 * self.power.max(1e-12).log10();

        self.recent_peak_db = (self.recent_peak_db - PEAK_DECAY_DB_PER_SECOND / self.sample_rate).max(level_db);

        // In Sprechpausen die Verstärkung halten, sonst wird das Rauschen hochgezogen.
        if level_db > self.gate_db {
            let wanted = (self.target_db - level_db).clamp(-self.max_cut_db, self.max_gain_db);
            if wanted < self.gain_db {
                self.gain_db += (wanted - self.gain_db) * self.attack_k;
            } else if level_db > self.recent_peak_db - TAIL_DB {
                // Nicht hochregeln, während ein Wort ausklingt.
                self.gain_db += (wanted - self.gain_db) * self.release_k;
            }
        }
        let mut y = x * db_to_gain(self.gain_db);

        // Limiter: sofort zupacken, langsam loslassen.
        let peak = y.abs();
        self.limiter_env = if peak > self.limiter_env {
            peak
        } else {
            self.limiter_env + (peak - self.limiter_env) * self.limiter_k
        };
        if self.limiter_env > self.ceiling {
            y *= self.ceiling / self.limiter_env;
        }
        y.clamp(-self.ceiling, self.ceiling)
    }
}

/// Startet die Ausgabe auf das virtuelle Gerät. Ohne gewähltes Gerät wird VB-Cable gesucht.
pub fn start_output(
    device_id: Option<&str>,
    samples: HeapCons<f32>,
    input_rate: u32,
    failed: Arc<AtomicBool>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    let device = match device_id.and_then(|s| s.parse::<cpal::DeviceId>().ok()) {
        Some(id) => host.device_by_id(&id),
        None => host.output_devices().ok().and_then(|mut devices| {
            devices.find(|d| {
                d.description()
                    .map(|desc| desc.name().to_lowercase().contains(VB_CABLE_HINT))
                    .unwrap_or(false)
            })
        }),
    }
    .ok_or_else(|| "VB-Cable nicht gefunden. Bitte installieren oder ein Ausgabegerät wählen.".to_string())?;

    let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Ausgabe".to_string());
    let config = device
        .default_output_config()
        .map_err(|e| format!("Ausgabe lässt sich nicht öffnen: {e}"))?;

    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, samples, input_rate, failed),
        SampleFormat::I16 => build::<i16>(&device, &config, samples, input_rate, failed),
        SampleFormat::I32 => build::<i32>(&device, &config, samples, input_rate, failed),
        SampleFormat::U16 => build::<u16>(&device, &config, samples, input_rate, failed),
        other => return Err(format!("Nicht unterstütztes Audioformat: {other}")),
    }?;
    stream.play().map_err(|e| format!("Ausgabe lässt sich nicht starten: {e}"))?;
    Ok((stream, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut samples: HeapCons<f32>,
    input_rate: u32,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = config.channels().max(1) as usize;
    let mut state = OutputState::new(input_rate, config.sample_rate());
    let mut mono = Vec::new();

    device
        .build_output_stream(
            config.clone().into(),
            move |data: &mut [T], _: &_| {
                mono.resize(data.len() / channels, 0.0);
                state.fill(&mut samples, &mut mono);
                for (frame, &value) in data.chunks_mut(channels).zip(&mono) {
                    frame.fill(T::from_sample(value));
                }
            },
            move |_err| failed.store(true, Ordering::Relaxed),
            None,
        )
        .map_err(|e| format!("Ausgabe lässt sich nicht öffnen: {e}"))
}

/// Holt die Samples aus dem Puffer, rechnet die Abtastrate um und gleicht Gangunterschiede aus.
struct OutputState {
    target: usize,
    max: usize,
    base_step: f64,
    primed: bool,
    resampler: Resampler,
}

impl OutputState {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            target: ((input_rate as f32 * TARGET_BUFFER_SECONDS) as usize).max(1),
            max: (input_rate as f32 * MAX_BUFFER_SECONDS) as usize,
            base_step: input_rate as f64 / output_rate as f64,
            primed: false,
            resampler: Resampler { frac: 0.0, previous: 0.0, current: 0.0 },
        }
    }

    fn fill(&mut self, samples: &mut HeapCons<f32>, out: &mut [f32]) {
        let filled = samples.occupied_len();
        // Nach einem Hänger nicht immer mehr Verzögerung ansammeln.
        if filled > self.max {
            samples.skip(filled - self.target);
        }
        // Erst loslegen, wenn etwas Puffer da ist, sonst knackt es am Anfang.
        if !self.primed {
            if filled < self.target {
                out.fill(0.0);
                return;
            }
            self.primed = true;
        }
        let error = (samples.occupied_len() as f64 - self.target as f64) / self.target as f64;
        let step = self.base_step * (1.0 + (error * 0.01).clamp(-MAX_DRIFT_CORRECTION, MAX_DRIFT_CORRECTION));
        for value in out {
            *value = self.resampler.next(samples, step);
        }
    }
}

/// Einfache lineare Umrechnung der Abtastrate, reicht für Sprache.
struct Resampler {
    frac: f64,
    previous: f32,
    current: f32,
}

impl Resampler {
    fn next(&mut self, samples: &mut HeapCons<f32>, step: f64) -> f32 {
        self.frac += step;
        while self.frac >= 1.0 {
            self.frac -= 1.0;
            self.previous = self.current;
            // Leerer Puffer: sanft gegen Stille laufen statt hart abzuschneiden.
            self.current = samples.try_pop().unwrap_or(self.current * 0.95);
        }
        self.previous + (self.current - self.previous) * self.frac as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn params() -> Arc<AgcParams> {
        let p = AgcParams::default();
        p.target_db.set(-20.0);
        p.max_gain_db.set(18.0);
        p.max_cut_db.set(18.0);
        p.attack_ms.set(40.0);
        p.release_ms.set(1500.0);
        p.gate_db.set(-50.0);
        p.ceiling_db.set(-1.0);
        Arc::new(p)
    }

    /// Sinus mit gegebenem RMS-Pegel durch die Regelung schicken, RMS am Ende messen.
    fn run(agc: &mut Agc, rms_db: f32, seconds: f32) -> (f32, f32) {
        let amplitude = db_to_gain(rms_db) * std::f32::consts::SQRT_2;
        let n = (RATE * seconds) as usize;
        let tail = (RATE * 0.5) as usize;
        let (mut sum, mut peak) = (0.0f64, 0.0f32);
        for i in 0..n {
            let x = amplitude * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin();
            let y = agc.process(x);
            peak = peak.max(y.abs());
            if i >= n - tail {
                sum += (y * y) as f64;
            }
        }
        (10.0 * (sum / tail as f64).log10() as f32, peak)
    }

    #[test]
    fn leise_wird_angehoben() {
        let mut agc = Agc::new(RATE, params());
        let (out, _) = run(&mut agc, -32.0, 10.0);
        assert!((out - -20.0).abs() < 1.0, "Ausgang {out} dB");
    }

    #[test]
    fn laut_wird_abgesenkt() {
        let mut agc = Agc::new(RATE, params());
        let (out, _) = run(&mut agc, -8.0, 3.0);
        assert!((out - -20.0).abs() < 1.0, "Ausgang {out} dB");
    }

    #[test]
    fn verstaerkung_ist_begrenzt() {
        let mut agc = Agc::new(RATE, params());
        let (out, _) = run(&mut agc, -45.0, 15.0);
        assert!((out - -27.0).abs() < 1.0, "Ausgang {out} dB, erwartet -45 + 18");
    }

    #[test]
    fn pause_zieht_rauschen_nicht_hoch() {
        let mut agc = Agc::new(RATE, params());
        run(&mut agc, -20.0, 3.0);
        let before = agc.gain_db;
        run(&mut agc, -65.0, 5.0);
        assert!((agc.gain_db - before).abs() < 0.5, "Verstärkung {before} -> {}", agc.gain_db);
    }

    #[test]
    fn puffer_bleibt_stabil_bei_gangunterschied() {
        use ringbuf::traits::{Producer, Split};

        // Mikrofon 48 kHz läuft 0,2 % zu schnell bzw. zu langsam, Ausgabe 44,1 kHz, 10-ms-Blöcke.
        for input_per_block in [48.096 * 10.0, 47.904 * 10.0] {
        let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(48_000).split();
        let mut state = OutputState::new(48_000, 44_100);
        let mut out = vec![0.0f32; 441];
        let (mut owed, mut underruns, mut max_fill) = (0.0f64, 0usize, 0usize);

        for block in 0..60_000 {
            owed += input_per_block;
            while owed >= 1.0 {
                let _ = producer.try_push(0.5);
                owed -= 1.0;
            }
            state.fill(&mut consumer, &mut out);
            if block > 100 {
                if consumer.occupied_len() == 0 {
                    underruns += 1;
                }
                max_fill = max_fill.max(consumer.occupied_len());
            }
        }
        // 10 Minuten simuliert: keine Aussetzer, Verzögerung bleibt unter 100 ms.
        assert_eq!(underruns, 0, "Aussetzer bei {input_per_block}");
        assert!(max_fill < 4_800, "Puffer {max_fill} Samples bei {input_per_block}");
        }
    }

    #[test]
    fn limiter_haelt_obergrenze() {
        let mut agc = Agc::new(RATE, params());
        run(&mut agc, -45.0, 15.0);
        // Plötzlicher Schrei bei voll aufgedrehter Verstärkung.
        let (_, peak) = run(&mut agc, -3.0, 1.0);
        assert!(peak <= db_to_gain(-1.0) + 1e-6, "Spitze {peak}");
    }
}

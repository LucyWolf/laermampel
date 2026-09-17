//! Mikrofon für andere Programme: Noise Gate und automatische Lautstärke, ausgegeben
//! auf ein virtuelles Audiogerät (VB-Cable), das andere Programme als Mikrofon benutzen.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::audio::{Fault, InputDevice, describe};

pub const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";
/// Meldung, wenn gar kein VB-Cable da ist. Das ist kein Fehler, nur nicht eingerichtet.
pub const VB_CABLE_MISSING: &str = "VB-Cable ist nicht installiert.";
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
const GATE_LEVEL_WINDOW_MS: f32 = 10.0;
/// Fader und Mute weich überblenden, sonst knackt es.
const FADER_SMOOTHING_MS: f32 = 10.0;
const METER_WINDOW_MS: f32 = 50.0;
const METER_PEAK_DECAY_DB_PER_SECOND: f32 = 20.0;
/// So weit muss der Pegel unter die Schwelle fallen, bevor das Gate zu zählen beginnt.
const GATE_HYSTERESIS_DB: f32 = 3.0;
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
    pub gate_enabled: AtomicBool,
    pub gate_threshold_db: AtomicF32,
    /// Wie viel leiser im geschlossenen Zustand.
    pub gate_range_db: AtomicF32,
    pub gate_attack_ms: AtomicF32,
    pub gate_hold_ms: AtomicF32,
    pub gate_release_ms: AtomicF32,
    /// Rückmeldung an die Oberfläche.
    pub gate_level_db: AtomicF32,
    pub gate_open: AtomicBool,

    pub agc_enabled: AtomicBool,

    /// Fader in dB und Stummschaltung, wirken nach Gate und Kompressor.
    pub fader_db: AtomicF32,
    pub muted: AtomicBool,
    /// Rückmeldung an die Oberfläche: Pegel am Ausgang, so wie andere dich hören.
    pub out_level_db: AtomicF32,
    pub out_peak_db: AtomicF32,
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

/// VB-Cable, Voicemeeter und ähnliche virtuelle Geräte.
pub fn is_virtual_device(name: &str) -> bool {
    let name = name.to_lowercase();
    ["vb-audio", "cable", "voicemeeter", "virtual"].iter().any(|hint| name.contains(hint))
}

/// Nur virtuelle Geräte: auf Kopfhörer oder Lautsprecher würde das Mikrofon zurückgespielt.
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
        .filter(|d| is_virtual_device(&d.name))
        .collect()
}

/// Mikrofon → Gate → Kompressor → Fader/Mute → Limiter → Puffer zur Ausgabe.
pub struct VoiceChain {
    sample_rate: f32,
    gate: Gate,
    agc: Agc,
    limiter: Limiter,
    params: Arc<AgcParams>,
    output: HeapProd<f32>,
    countdown: usize,

    fader_k: f32,
    fader_target: f32,
    fader_gain: f32,

    meter_k: f32,
    out_power: f32,
    out_peak_db: f32,
}

impl VoiceChain {
    pub fn new(sample_rate: u32, params: Arc<AgcParams>, output: HeapProd<f32>) -> Self {
        let rate = sample_rate as f32;
        Self {
            sample_rate: rate,
            gate: Gate::new(rate, Arc::clone(&params)),
            agc: Agc::new(rate, Arc::clone(&params)),
            limiter: Limiter::new(rate),
            params,
            output,
            countdown: 0,
            fader_k: smoothing(FADER_SMOOTHING_MS, rate),
            fader_target: 1.0,
            fader_gain: 1.0,
            meter_k: smoothing(METER_WINDOW_MS, rate),
            out_power: 0.0,
            out_peak_db: -120.0,
        }
    }

    fn reload(&mut self) {
        let p = &self.params;
        self.fader_target = if p.muted.load(Ordering::Relaxed) { 0.0 } else { db_to_gain(p.fader_db.get()) };
        self.limiter.ceiling = db_to_gain(p.ceiling_db.get().min(0.0));
        p.out_level_db.set(10.0 * self.out_power.max(1e-12).log10());
        p.out_peak_db.set(self.out_peak_db);
    }

    pub fn push(&mut self, x: f32) {
        if self.countdown == 0 {
            self.reload();
            self.countdown = PARAM_RELOAD_SAMPLES;
        }
        self.countdown -= 1;

        let mut y = x;
        if self.params.gate_enabled.load(Ordering::Relaxed) {
            y = self.gate.process(y);
        }
        if self.params.agc_enabled.load(Ordering::Relaxed) {
            y = self.agc.process(y);
        }
        self.fader_gain += (self.fader_target - self.fader_gain) * self.fader_k;
        y = self.limiter.process(y * self.fader_gain);

        self.out_power += (y * y - self.out_power) * self.meter_k;
        let peak_db = 20.0 * y.abs().max(1e-6).log10();
        self.out_peak_db = (self.out_peak_db - METER_PEAK_DECAY_DB_PER_SECOND / self.sample_rate).max(peak_db);

        // Ist der Puffer voll, hängt die Ausgabe; dann lieber verwerfen als blockieren.
        let _ = self.output.try_push(y);
    }
}

/// Lässt keine Spitze über die Obergrenze: sofort zupacken, langsam loslassen.
struct Limiter {
    release_k: f32,
    envelope: f32,
    ceiling: f32,
}

impl Limiter {
    fn new(sample_rate: f32) -> Self {
        Self { release_k: smoothing(LIMITER_RELEASE_MS, sample_rate), envelope: 0.0, ceiling: 1.0 }
    }

    fn process(&mut self, x: f32) -> f32 {
        let peak = x.abs();
        self.envelope = if peak > self.envelope {
            peak
        } else {
            self.envelope + (peak - self.envelope) * self.release_k
        };
        let y = if self.envelope > self.ceiling { x * self.ceiling / self.envelope } else { x };
        y.clamp(-self.ceiling, self.ceiling)
    }
}

/// Unter der Schwelle wird das Mikrofon abgesenkt, darüber geht es auf.
pub struct Gate {
    sample_rate: f32,
    params: Arc<AgcParams>,
    countdown: usize,

    level_k: f32,
    attack_k: f32,
    release_k: f32,
    threshold_db: f32,
    closed_gain: f32,
    hold_samples: usize,

    power: f32,
    gain: f32,
    open: bool,
    hold_left: usize,
}

impl Gate {
    pub fn new(sample_rate: f32, params: Arc<AgcParams>) -> Self {
        Self {
            sample_rate,
            params,
            countdown: 0,
            level_k: smoothing(GATE_LEVEL_WINDOW_MS, sample_rate),
            attack_k: 1.0,
            release_k: 1.0,
            threshold_db: 0.0,
            closed_gain: 0.0,
            hold_samples: 0,
            power: 0.0,
            gain: 0.0,
            open: false,
            hold_left: 0,
        }
    }

    fn reload(&mut self, level_db: f32) {
        let p = &self.params;
        self.attack_k = smoothing(p.gate_attack_ms.get(), self.sample_rate);
        self.release_k = smoothing(p.gate_release_ms.get(), self.sample_rate);
        self.threshold_db = p.gate_threshold_db.get();
        self.closed_gain = db_to_gain(-p.gate_range_db.get().max(0.0));
        self.hold_samples = (p.gate_hold_ms.get().max(0.0) / 1000.0 * self.sample_rate) as usize;
        p.gate_level_db.set(level_db);
        p.gate_open.store(self.open, Ordering::Relaxed);
    }

    pub fn process(&mut self, x: f32) -> f32 {
        self.power += (x * x - self.power) * self.level_k;
        let level_db = 10.0 * self.power.max(1e-12).log10();

        if self.countdown == 0 {
            self.reload(level_db);
            self.countdown = PARAM_RELOAD_SAMPLES;
        }
        self.countdown -= 1;

        if level_db > self.threshold_db {
            self.open = true;
            self.hold_left = self.hold_samples;
        } else if self.open && level_db < self.threshold_db - GATE_HYSTERESIS_DB {
            // Knapp unter der Schwelle nicht zählen, sonst flattert es am Wortende.
            if self.hold_left > 0 {
                self.hold_left -= 1;
            } else {
                self.open = false;
            }
        }

        let target = if self.open { 1.0 } else { self.closed_gain };
        let k = if target > self.gain { self.attack_k } else { self.release_k };
        self.gain += (target - self.gain) * k;
        x * self.gain
    }
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
    target_db: f32,
    max_gain_db: f32,
    max_cut_db: f32,
    gate_db: f32,

    power: f32,
    recent_peak_db: f32,
    gain_db: f32,
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
            target_db: 0.0,
            max_gain_db: 0.0,
            max_cut_db: 0.0,
            gate_db: 0.0,
            power: 0.0,
            recent_peak_db: -120.0,
            gain_db: 0.0,
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
        x * db_to_gain(self.gain_db)
    }
}

/// Startet die Ausgabe auf das virtuelle Gerät. Ohne gewähltes Gerät wird VB-Cable gesucht.
pub fn start_output(
    device_id: Option<&str>,
    samples: HeapCons<f32>,
    input_rate: u32,
    fault: Arc<Fault>,
) -> Result<(cpal::Stream, String), String> {
    let host = cpal::default_host();
    // Ein gewähltes Gerät nur nehmen, wenn es da und virtuell ist; sonst VB-Cable suchen.
    let chosen = device_id
        .and_then(|s| s.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .filter(|d| d.description().is_ok_and(|desc| is_virtual_device(desc.name())));
    let device = chosen
        .or_else(|| {
            host.output_devices().ok().and_then(|mut devices| {
                devices.find(|d| d.description().is_ok_and(|desc| desc.name().to_lowercase().contains(VB_CABLE_HINT)))
            })
        })
        .ok_or_else(|| VB_CABLE_MISSING.to_string())?;

    let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Ausgabe".to_string());
    // Auf Kopfhörer oder Lautsprecher würde das Mikrofon direkt zurückgespielt.
    if !is_virtual_device(&name) {
        return Err(format!(
            "„{name}“ ist kein virtuelles Gerät. Dort würdest du dich selbst hören \
             (bei Lautsprechern gibt es eine Rückkopplung). Bitte VB-Cable als Ausgabe wählen."
        ));
    }
    let config = device
        .default_output_config()
        .map_err(|e| format!("Ausgabe lässt sich nicht öffnen: {}", describe(&e)))?;

    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, samples, input_rate, fault),
        SampleFormat::I16 => build::<i16>(&device, &config, samples, input_rate, fault),
        SampleFormat::I32 => build::<i32>(&device, &config, samples, input_rate, fault),
        SampleFormat::U16 => build::<u16>(&device, &config, samples, input_rate, fault),
        other => return Err(format!("Nicht unterstütztes Audioformat: {other}")),
    }?;
    stream.play().map_err(|e| format!("Ausgabe lässt sich nicht starten: {}", describe(&e)))?;
    Ok((stream, name))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut samples: HeapCons<f32>,
    input_rate: u32,
    fault: Arc<Fault>,
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
            move |err| fault.report(err),
            None,
        )
        .map_err(|e| format!("Ausgabe lässt sich nicht öffnen: {}", describe(&e)))
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

    fn gate_params() -> Arc<AgcParams> {
        let p = params();
        p.gate_enabled.store(true, Ordering::Relaxed);
        p.agc_enabled.store(false, Ordering::Relaxed);
        p.gate_threshold_db.set(-40.0);
        p.gate_range_db.set(40.0);
        p.gate_attack_ms.set(2.0);
        p.gate_hold_ms.set(200.0);
        p.gate_release_ms.set(50.0);
        p
    }

    fn sine(n: usize, rms_db: f32) -> Vec<f32> {
        let amplitude = db_to_gain(rms_db) * std::f32::consts::SQRT_2;
        (0..n).map(|i| amplitude * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin()).collect()
    }

    fn rms_db(samples: &[f32]) -> f32 {
        let power = samples.iter().map(|&x| (x * x) as f64).sum::<f64>() / samples.len() as f64;
        10.0 * power.log10() as f32
    }

    fn through_gate(gate: &mut Gate, input: &[f32]) -> Vec<f32> {
        input.iter().map(|&x| gate.process(x)).collect()
    }

    #[test]
    fn virtuelle_geraete_werden_erkannt() {
        assert!(is_virtual_device("CABLE Output (VB-Audio Virtual Cable)"));
        assert!(is_virtual_device("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(is_virtual_device("VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"));
        assert!(!is_virtual_device("Headset Microphone (Arctis 7 Chat)"));
        assert!(!is_virtual_device("Lautsprecher (Realtek(R) Audio)"));
    }

    #[test]
    fn gate_senkt_leises_ab() {
        let mut gate = Gate::new(RATE, gate_params());
        let input = sine(RATE as usize, -55.0);
        let output = through_gate(&mut gate, &input);
        let tail = RATE as usize / 2;
        let reduction = rms_db(&input[tail..]) - rms_db(&output[tail..]);
        assert!((reduction - 40.0).abs() < 1.0, "{reduction:.1} dB abgesenkt");
    }

    #[test]
    fn gate_laesst_sprache_durch() {
        let mut gate = Gate::new(RATE, gate_params());
        let input = sine(RATE as usize, -20.0);
        let output = through_gate(&mut gate, &input);
        let tail = RATE as usize / 2;
        let difference = rms_db(&input[tail..]) - rms_db(&output[tail..]);
        assert!(difference.abs() < 0.1, "{difference:.2} dB Unterschied");
    }

    #[test]
    fn gate_haelt_nach_wortende() {
        let mut gate = Gate::new(RATE, gate_params());
        through_gate(&mut gate, &sine(RATE as usize, -20.0));
        // 100 ms nach dem Wort: noch offen (Halten 200 ms). 500 ms danach: zu.
        through_gate(&mut gate, &sine(RATE as usize / 10, -60.0));
        assert!(gate.open, "schon nach 100 ms zu");
        through_gate(&mut gate, &sine(RATE as usize * 4 / 10, -60.0));
        assert!(!gate.open, "nach 500 ms noch offen");
    }

    #[test]
    fn ausgeschaltet_bleibt_signal_gleich() {
        let p = params();
        p.gate_enabled.store(false, Ordering::Relaxed);
        p.agc_enabled.store(false, Ordering::Relaxed);
        let (mut chain, mut consumer) = chain(p);
        let input = sine(RATE as usize / 2, -20.0);
        for &x in &input {
            chain.push(x);
        }
        let output: Vec<f32> = consumer.pop_iter().collect();
        assert_eq!(output, input);
    }

    fn chain(p: Arc<AgcParams>) -> (VoiceChain, HeapCons<f32>) {
        use ringbuf::traits::Split;
        let (producer, consumer) = ringbuf::HeapRb::<f32>::new(RATE as usize * 20).split();
        (VoiceChain::new(RATE as u32, p, producer), consumer)
    }

    #[test]
    fn limiter_haelt_obergrenze() {
        let p = params();
        p.agc_enabled.store(true, Ordering::Relaxed);
        p.fader_db.set(12.0);
        let (mut chain, mut consumer) = chain(p);
        for x in sine(RATE as usize * 15, -45.0) {
            chain.push(x);
        }
        // Plötzlicher Schrei bei voll aufgedrehter Verstärkung und Fader.
        for x in sine(RATE as usize, -3.0) {
            chain.push(x);
        }
        let peak = consumer.pop_iter().fold(0.0f32, |m, y| m.max(y.abs()));
        assert!(peak <= db_to_gain(-1.0) + 1e-6, "Spitze {peak}");
    }

    #[test]
    fn fader_und_mute_wirken() {
        let p = params();
        p.fader_db.set(-6.0);
        let (mut chain, mut consumer) = chain(Arc::clone(&p));
        let input = sine(RATE as usize, -20.0);
        for &x in &input {
            chain.push(x);
        }
        let output: Vec<f32> = consumer.pop_iter().collect();
        let tail = RATE as usize / 2;
        let difference = rms_db(&input[tail..]) - rms_db(&output[tail..]);
        assert!((difference - 6.0).abs() < 0.1, "{difference:.2} dB leiser");

        p.muted.store(true, Ordering::Relaxed);
        for &x in &input {
            chain.push(x);
        }
        let muted: Vec<f32> = consumer.pop_iter().collect();
        assert!(rms_db(&muted[tail..]) < -100.0, "stumm ist {} dB", rms_db(&muted[tail..]));
    }
}

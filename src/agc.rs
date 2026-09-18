//! Mikrofon für andere Programme über VB-Cable: dieselbe Bearbeitung wie im Audio-Filter
//! (Gate, Comp., Fader, Limiter), ausgegeben auf ein virtuelles Audiogerät.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use ringbuf::traits::{Consumer, Observer, Producer};
use ringbuf::{HeapCons, HeapProd};

use crate::audio::{Fault, InputDevice, describe};
use crate::dsp::{Chain, Settings};
use crate::lang::t;

/// Rückmeldung der Kette für die Anzeige.
#[derive(Clone, Copy, Debug, Default)]
pub struct Feedback {
    pub out_level_db: f32,
    pub gate_open: bool,
    pub gate_level_db: f32,
    pub comp_gain_db: f32,
}

pub const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";
/// Meldung, wenn gar kein VB-Cable da ist. Das ist kein Fehler, nur nicht eingerichtet.
pub fn vb_cable_missing() -> &'static str {
    t("VB-Cable ist nicht installiert.", "VB-Cable is not installed.")
}
/// Name des Wiedergabegeräts von VB-Cable. Programme nehmen dann „CABLE Output“ als Mikrofon.
const VB_CABLE_HINT: &str = "cable input";

/// Notfallwert, falls nichts eingestellt ist.
pub const DEFAULT_BUFFER_MS: f32 = 30.0;
const MAX_BUFFER_SECONDS: f32 = 0.2;
/// Die beiden Geräte laufen nie exakt gleich schnell, das wird hier sanft ausgeglichen.
const MAX_DRIFT_CORRECTION: f64 = 0.005;

/// Neue Werte werden nur alle so viele Samples übernommen.
const PARAM_RELOAD_SAMPLES: usize = 256;

/// Austausch zwischen Oberfläche und Audio-Thread. Der Audio-Thread wartet nie auf die Sperre.
#[derive(Default)]
pub struct ChainControl {
    settings: Mutex<Settings>,
    feedback: Mutex<Feedback>,
    /// Woraus sich die Verzögerung zusammensetzt, in Samples bzw. Hz.
    input_frames: AtomicU32,
    input_rate: AtomicU32,
    buffer_frames: AtomicU32,
    output_frames: AtomicU32,
    output_rate: AtomicU32,
}

/// Verzögerung in Millisekunden, aufgeschlüsselt.
#[derive(Clone, Copy, Debug, Default)]
pub struct Latency {
    /// Block, den Windows der Lärmampel vom Mikrofon gibt.
    pub input_ms: f32,
    /// Puffer zwischen Aufnahme und Ausgabe.
    pub buffer_ms: f32,
    /// Block, den das Ausgabegerät abholt.
    pub output_ms: f32,
}

impl Latency {
    pub fn total_ms(&self) -> f32 {
        self.input_ms + self.buffer_ms + self.output_ms
    }
}

impl ChainControl {
    pub fn set(&self, settings: Settings) {
        if let Ok(mut slot) = self.settings.lock() {
            *slot = settings;
        }
    }

    pub fn feedback(&self) -> Feedback {
        self.feedback.lock().map(|f| *f).unwrap_or_default()
    }

    fn report_input(&self, frames: u32, rate: u32) {
        self.input_frames.store(frames, Ordering::Relaxed);
        self.input_rate.store(rate, Ordering::Relaxed);
    }

    fn report_output(&self, buffer_frames: u32, frames: u32, rate: u32) {
        self.buffer_frames.store(buffer_frames, Ordering::Relaxed);
        self.output_frames.store(frames, Ordering::Relaxed);
        self.output_rate.store(rate, Ordering::Relaxed);
    }

    pub fn latency(&self) -> Latency {
        let ms = |frames: &AtomicU32, rate: &AtomicU32| {
            let rate = rate.load(Ordering::Relaxed);
            if rate == 0 { 0.0 } else { frames.load(Ordering::Relaxed) as f32 * 1000.0 / rate as f32 }
        };
        Latency {
            input_ms: ms(&self.input_frames, &self.input_rate),
            buffer_ms: ms(&self.buffer_frames, &self.input_rate),
            output_ms: ms(&self.output_frames, &self.output_rate),
        }
    }
}

/// VB-Cable, Voicemeeter und ähnliche virtuelle Geräte.
pub fn is_virtual_device(name: &str) -> bool {
    let name = name.to_lowercase();
    ["vb-audio", "cable", "voicemeeter", "virtual"].iter().any(|hint| name.contains(hint))
}

/// Name eines Geräts zu einer gespeicherten ID, auch wenn es kein virtuelles ist.
/// `None` heißt „gerade nicht da“ – direkt nach dem Windows-Start kann das noch dauern.
pub fn output_device_name(device_id: &str) -> Option<String> {
    let host = cpal::default_host();
    let id = device_id.parse::<cpal::DeviceId>().ok()?;
    let device = host.device_by_id(&id)?;
    device.description().ok().map(|d| d.name().to_string())
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

/// Mikrofon → gemeinsame Kette → Puffer zur Ausgabe auf VB-Cable.
pub struct VoiceChain {
    chain: Chain,
    sample_rate: u32,
    control: Arc<ChainControl>,
    output: HeapProd<f32>,
    countdown: usize,
}

impl VoiceChain {
    pub fn new(sample_rate: u32, control: Arc<ChainControl>, output: HeapProd<f32>) -> Self {
        Self { chain: Chain::new(sample_rate as f32), sample_rate, control, output, countdown: 0 }
    }

    /// Blockgröße der Aufnahme melden, für die Verzögerungs-Anzeige.
    pub fn report_input(&self, frames: u32) {
        self.control.report_input(frames, self.sample_rate);
    }

    pub fn push(&mut self, x: f32) {
        if self.countdown == 0 {
            self.countdown = PARAM_RELOAD_SAMPLES;
            if let Ok(settings) = self.control.settings.try_lock() {
                self.chain.set(*settings);
            }
            if let Ok(mut feedback) = self.control.feedback.try_lock() {
                *feedback = Feedback {
                    out_level_db: self.chain.out_level_db(),
                    gate_open: self.chain.gate_open(),
                    gate_level_db: self.chain.gate_level_db(),
                    comp_gain_db: self.chain.comp_gain_db(),
                };
            }
        }
        self.countdown -= 1;

        let mut frame = [x];
        self.chain.process_frame(&mut frame);
        // Ist der Puffer voll, hängt die Ausgabe; dann lieber verwerfen als blockieren.
        let _ = self.output.try_push(frame[0]);
    }
}

/// Startet die Ausgabe auf das virtuelle Gerät. Ohne gewähltes Gerät wird VB-Cable gesucht.
pub fn start_output(
    device_id: Option<&str>,
    samples: HeapCons<f32>,
    input_rate: u32,
    buffer_ms: f32,
    control: Arc<ChainControl>,
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
        .ok_or_else(|| vb_cable_missing().to_string())?;

    let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| t("Ausgabe", "Output").to_string());
    // Auf Kopfhörer oder Lautsprecher würde das Mikrofon direkt zurückgespielt.
    if !is_virtual_device(&name) {
        return Err(format!(
            "„{name}“ {}",
            t(
                "ist kein virtuelles Gerät. Dort würdest du dich selbst hören (bei Lautsprechern \
                 gibt es eine Rückkopplung). Bitte VB-Cable als Ausgabe wählen.",
                "is not a virtual device. You would hear yourself there (speakers would feed back). \
                 Please choose VB-Cable as the output.",
            )
        ));
    }
    let config = device
        .default_output_config()
        .map_err(|e| format!("{}: {}", t("Ausgabe lässt sich nicht öffnen", "Cannot open output"), describe(&e)))?;

    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, samples, input_rate, buffer_ms, control, fault),
        SampleFormat::I16 => build::<i16>(&device, &config, samples, input_rate, buffer_ms, control, fault),
        SampleFormat::I32 => build::<i32>(&device, &config, samples, input_rate, buffer_ms, control, fault),
        SampleFormat::U16 => build::<u16>(&device, &config, samples, input_rate, buffer_ms, control, fault),
        other => return Err(format!("{}: {other}", t("Nicht unterstütztes Audioformat", "Unsupported audio format"))),
    }?;
    stream.play().map_err(|e| format!("{}: {}", t("Ausgabe lässt sich nicht starten", "Cannot start output"), describe(&e)))?;
    Ok((stream, name))
}

#[allow(clippy::too_many_arguments)]
fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut samples: HeapCons<f32>,
    input_rate: u32,
    buffer_ms: f32,
    control: Arc<ChainControl>,
    fault: Arc<Fault>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = config.channels().max(1) as usize;
    let output_rate = config.sample_rate();
    let mut state = OutputState::new(input_rate, output_rate, buffer_ms);
    let mut mono = Vec::new();

    device
        .build_output_stream(
            config.clone().into(),
            move |data: &mut [T], _: &_| {
                mono.resize(data.len() / channels, 0.0);
                control.report_output(samples.occupied_len() as u32, mono.len() as u32, output_rate);
                state.fill(&mut samples, &mut mono);
                for (frame, &value) in data.chunks_mut(channels).zip(&mono) {
                    frame.fill(T::from_sample(value));
                }
            },
            move |err| fault.report(err),
            None,
        )
        .map_err(|e| format!("{}: {}", t("Ausgabe lässt sich nicht öffnen", "Cannot open output"), describe(&e)))
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
    fn new(input_rate: u32, output_rate: u32, buffer_ms: f32) -> Self {
        Self {
            target: ((input_rate as f32 * buffer_ms.clamp(3.0, 200.0) / 1000.0) as usize).max(1),
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
        // Puffer ganz leergelaufen (Aussetzer): von vorne auffüllen, sonst bleibt er am Boden
        // und es knistert dauerhaft weiter.
        if self.primed && filled == 0 {
            self.primed = false;
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

    #[test]
    fn puffer_bleibt_stabil_bei_gangunterschied() {
        use ringbuf::traits::{Producer, Split};

        // Mikrofon 48 kHz läuft 0,2 % zu schnell bzw. zu langsam, Ausgabe 44,1 kHz, 10-ms-Blöcke.
        for input_per_block in [48.096 * 10.0, 47.904 * 10.0] {
        let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(48_000).split();
        let mut state = OutputState::new(48_000, 44_100, DEFAULT_BUFFER_MS);
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
    fn ausgabe_fuellt_sich_nach_aussetzer_neu_auf() {
        use ringbuf::traits::{Producer, Split};

        let (mut producer, mut consumer) = ringbuf::HeapRb::<f32>::new(48_000).split();
        let mut state = OutputState::new(48_000, 48_000, DEFAULT_BUFFER_MS);
        let target = state.target;
        let mut out = vec![0.0f32; 480];

        // Normal anlaufen: Puffer voll genug, Ausgabe läuft.
        for _ in 0..target {
            let _ = producer.try_push(0.5);
        }
        state.fill(&mut consumer, &mut out);
        assert!(out.iter().any(|&s| s != 0.0), "Ausgabe sollte laufen");

        // Aussetzer: es kommt nichts nach, der Puffer läuft leer.
        while consumer.occupied_len() > 0 {
            state.fill(&mut consumer, &mut out);
        }
        state.fill(&mut consumer, &mut out);

        // Danach kommt wieder etwas, aber noch zu wenig: erst auffüllen, nichts verbrauchen.
        for _ in 0..target / 2 {
            let _ = producer.try_push(0.5);
        }
        let before = consumer.occupied_len();
        state.fill(&mut consumer, &mut out);
        assert_eq!(consumer.occupied_len(), before, "Puffer muss sich erst wieder füllen");
        assert!(out.iter().all(|&s| s == 0.0), "Solange Stille statt dauerndem Knistern");
    }

    #[test]
    fn virtuelle_geraete_werden_erkannt() {
        assert!(is_virtual_device("CABLE Output (VB-Audio Virtual Cable)"));
        assert!(is_virtual_device("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(is_virtual_device("VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"));
        assert!(!is_virtual_device("Headset Microphone (Arctis 7 Chat)"));
        assert!(!is_virtual_device("Lautsprecher (Realtek(R) Audio)"));
    }
}

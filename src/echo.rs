//! Echounterdrückung: rechnet aus dem Mikrofon heraus, was aus den Kopfhörern kommt.
//!
//! Ein Rauschfilter kann das nicht: was aus den Kopfhörern kommt, *ist* Sprache, also lässt
//! er es stehen. Herausrechnen geht nur, wenn man weiß, was gerade gespielt wird. Windows
//! gibt das über „Loopback“ her: dasselbe Wiedergabegerät lässt sich als Aufnahmegerät
//! öffnen und liefert dann genau das, was in den Kopfhörern läuft.
//!
//! ```text
//! Kopfhörer (Loopback) ─┐
//!                       ├─→ AEC3 ─→ sauberes Mikrofon
//! Mikrofon ─────────────┘
//! ```
//!
//! Gerechnet wird in Blöcken von 10 ms (so arbeitet AEC3), das Mikrofon kommt also um
//! 10 ms verzögert heraus. Die Verzögerung zwischen Wiedergabe und Mikrofon sucht AEC3
//! selbst; deshalb muss hier nichts ausgemessen werden.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapRb};
use sonora::config::{EchoCanceller, TransparentModeType};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::audio::{Fault, describe};

/// Ein Block von 10 ms; darauf arbeitet AEC3.
const BLOCK_MS: usize = 10;
/// So viel Vorlauf darf die Wiedergabe haben, bevor Ältestes verworfen wird (eine halbe Sekunde).
const MAX_REFERENCE_SECONDS: f32 = 0.5;

/// Läuft mit, was aus den Kopfhörern kommt.
pub struct Reference {
    _stream: cpal::Stream,
    pub name: String,
    pub rate: u32,
    samples: HeapCons<f32>,
}

impl Reference {
    /// Ein Block Wiedergabe, oder Stille, wenn gerade nichts läuft.
    fn take_block(&mut self, out: &mut [f32]) {
        for slot in out.iter_mut() {
            *slot = self.samples.try_pop().unwrap_or(0.0);
        }
    }

    /// Hat sich zu viel angestaut (z.B. nach einem Hänger), das Älteste wegwerfen.
    fn trim(&mut self) {
        let max = (self.rate as f32 * MAX_REFERENCE_SECONDS) as usize;
        let filled = self.samples.occupied_len();
        if filled > max {
            self.samples.skip(filled - max);
        }
    }
}

/// Öffnet das Wiedergabegerät als Aufnahme. Unter Windows schaltet WASAPI dabei von selbst
/// in den Loopback-Betrieb; ohne Gerätewahl wird das Standardgerät genommen.
pub fn start_reference(device_id: Option<&str>, fault: Arc<Fault>) -> Result<Reference, String> {
    let host = cpal::default_host();
    let device = device_id
        .and_then(|s| s.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .or_else(|| host.default_output_device())
        .ok_or_else(|| "Kein Wiedergabegerät für die Echounterdrückung gefunden".to_string())?;
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_default();

    // Für Loopback gilt das Format der Wiedergabe, nicht das einer Aufnahme.
    let config = device
        .default_output_config()
        .map_err(|e| format!("Mithören lässt sich nicht öffnen: {}", describe(&e)))?;
    let rate = config.sample_rate();
    let (producer, samples) = HeapRb::<f32>::new(rate as usize).split();

    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, producer, &fault),
        SampleFormat::I16 => build::<i16>(&device, &config, producer, &fault),
        SampleFormat::I32 => build::<i32>(&device, &config, producer, &fault),
        SampleFormat::U16 => build::<u16>(&device, &config, producer, &fault),
        SampleFormat::U8 => build::<u8>(&device, &config, producer, &fault),
        other => return Err(format!("Nicht unterstütztes Audioformat: {other}")),
    }?;
    stream.play().map_err(|e| format!("Mithören lässt sich nicht starten: {}", describe(&e)))?;
    Ok(Reference { _stream: stream, name, rate, samples })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mut producer: ringbuf::HeapProd<f32>,
    fault: &Arc<Fault>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = config.channels().max(1) as usize;
    let fault = Arc::clone(fault);
    device
        .build_input_stream(
            config.clone().into(),
            move |data: &[T], _: &_| {
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
                    let _ = producer.try_push(sum / channels as f32);
                }
            },
            move |err| fault.report(err),
            None,
        )
        .map_err(|e| format!("Mithören lässt sich nicht öffnen: {}", describe(&e)))
}

/// Mikrofon rein, Mikrofon ohne Kopfhörer-Anteil raus.
pub struct Echo {
    apm: AudioProcessing,
    reference: Reference,
    /// Block, der gerade gefüllt wird, und das Ergebnis des vorigen.
    mic_in: Vec<f32>,
    mic_out: Vec<f32>,
    ref_in: Vec<f32>,
    ref_out: Vec<f32>,
    /// Fertige Samples, die noch abgeholt werden.
    ready: std::collections::VecDeque<f32>,
}

impl Echo {
    pub fn new(mic_rate: u32, reference: Reference) -> Self {
        let config = Config {
            // Nur das Echo: Rauschen und Lautstärke macht die eigene Kette.
            echo_canceller: Some(EchoCanceller {
                enforce_high_pass_filtering: true,
                // Erkennt selbst, wenn gar kein Echo da ist, und hält sich dann zurück.
                transparent_mode: TransparentModeType::default(),
            }),
            ..Default::default()
        };
        let capture = StreamConfig::new(mic_rate, 1);
        let render = StreamConfig::new(reference.rate, 1);
        let apm = AudioProcessing::builder().config(config).capture_config(capture).render_config(render).build();

        let mic_frames = mic_rate as usize * BLOCK_MS / 1000;
        let ref_frames = reference.rate as usize * BLOCK_MS / 1000;
        Self {
            apm,
            reference,
            mic_in: Vec::with_capacity(mic_frames),
            mic_out: vec![0.0; mic_frames],
            ref_in: vec![0.0; ref_frames],
            ref_out: vec![0.0; ref_frames],
            ready: std::collections::VecDeque::with_capacity(mic_frames * 2),
        }
    }

    pub fn reference_name(&self) -> &str {
        &self.reference.name
    }

    /// Ein Sample hinein, ein um 10 ms verzögertes, sauberes Sample heraus.
    pub fn process(&mut self, x: f32) -> f32 {
        self.mic_in.push(x);
        if self.mic_in.len() == self.mic_out.len() {
            // Erst die Wiedergabe: AEC3 braucht sie, bevor es das Mikrofon sieht.
            self.reference.trim();
            self.reference.take_block(&mut self.ref_in);
            let _ = self.apm.process_render_f32(&[&self.ref_in], &mut [&mut self.ref_out]);
            if self.apm.process_capture_f32(&[&self.mic_in], &mut [&mut self.mic_out]).is_ok() {
                self.ready.extend(self.mic_out.iter().copied());
            } else {
                // Geht etwas schief, lieber unbearbeitet weiterreichen als Stille.
                self.ready.extend(self.mic_in.iter().copied());
            }
            self.mic_in.clear();
        }
        self.ready.pop_front().unwrap_or(0.0)
    }
}

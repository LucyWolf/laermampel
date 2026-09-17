//! Mikrofon einlesen und kurzzeitige Pegel (dBFS) bereitstellen.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use ringbuf::HeapRb;
use ringbuf::traits::Split;

use crate::agc::{self, ChainControl, VoiceChain};
use crate::dsp::{Biquad, METER_HIGHPASS_HZ, METER_LOWPASS_HZ};

/// Länge eines Messblocks. Kurz, damit die Anzeige sofort reagiert.
const BLOCK_SECONDS: f32 = 0.02;

pub const SILENCE_DB: f32 = -100.0;

pub const NO_DEVICE: &str = "Kein Gerät ausgewählt";
pub const MISSING_DEVICE: &str = "Mikrofon nicht gefunden";

/// Merkt sich einen Fehler aus einem Audio-Stream, der einen Neustart nötig macht.
#[derive(Default)]
pub struct Fault(Mutex<Option<String>>);

impl Fault {
    /// Unter Windows kommen über den Fehlerkanal auch reine Hinweise (kein Echtzeit-Vorrang,
    /// kurzer Knackser, Gerät automatisch umgeleitet). Die Aufnahme läuft dabei weiter.
    pub fn report(&self, err: cpal::Error) {
        use cpal::ErrorKind::{DeviceChanged, RealtimeDenied, Xrun};
        if matches!(err.kind(), DeviceChanged | RealtimeDenied | Xrun) {
            return;
        }
        if let Ok(mut slot) = self.0.lock() {
            slot.get_or_insert_with(|| describe(&err));
        }
    }

    pub fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|s| s.clone())
    }
}

pub fn device_name(device: &cpal::Device) -> String {
    device.description().map(|d| d.name().to_string()).unwrap_or_default()
}

/// Verständliche Fehlermeldung, bei den typischen Windows-Ursachen mit Lösung.
pub fn describe(err: &cpal::Error) -> String {
    match err.kind() {
        cpal::ErrorKind::PermissionDenied => "Zugriff verweigert. Windows-Einstellungen → Datenschutz → Mikrofon → \
             „Desktop-Apps Zugriff auf das Mikrofon erlauben“ einschalten."
            .to_string(),
        cpal::ErrorKind::DeviceBusy => {
            "Gerät ist belegt. Ein anderes Programm nutzt es exklusiv \
             (Windows-Soundeinstellungen → Gerät → Erweitert → exklusive Nutzung abschalten)."
                .to_string()
        }
        cpal::ErrorKind::DeviceNotAvailable => "Gerät nicht verfügbar, vermutlich abgesteckt.".to_string(),
        _ => err.to_string(),
    }
}

#[derive(Clone)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
}

pub fn list_input_devices() -> Vec<InputDevice> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d
                .description()
                .map(|desc| desc.name().to_string())
                .unwrap_or_else(|_| id.clone());
            Some(InputDevice { id, name })
        })
        .collect()
}

/// So viele letzte Samples stehen fürs Frequenz-Diagramm bereit.
pub const SAMPLE_RING: usize = 2048;
/// So viele Samples werden gesammelt, bevor sie in den Ringpuffer wandern.
const RAW_BLOCK: usize = 256;

/// Ringpuffer mit den letzten Samples, vom Audio-Thread gefüllt, von der Anzeige gelesen.
pub struct RawSamples {
    ring: Mutex<(Vec<f32>, usize)>,
}

impl Default for RawSamples {
    fn default() -> Self {
        Self { ring: Mutex::new((vec![0.0; SAMPLE_RING], 0)) }
    }
}

impl RawSamples {
    /// Blockweise schreiben: ein Mutex pro Sample wäre im Audio-Thread reine Last, und bei
    /// jedem belegten Schloss ginge ein einzelnes Sample verloren (Lücken im Diagramm).
    fn push_block(&self, samples: &[f32]) {
        let Ok(mut guard) = self.ring.try_lock() else { return };
        let (ring, write) = &mut *guard;
        for &x in samples {
            ring[*write] = x;
            *write = (*write + 1) % SAMPLE_RING;
        }
    }

    /// Die letzten Samples in richtiger Reihenfolge, oder `None`, wenn gerade geschrieben wird.
    pub fn snapshot(&self) -> Option<Vec<f32>> {
        let guard = self.ring.try_lock().ok()?;
        let (ring, write) = &*guard;
        let mut samples = Vec::with_capacity(SAMPLE_RING);
        samples.extend_from_slice(&ring[*write..]);
        samples.extend_from_slice(&ring[..*write]);
        Some(samples)
    }
}

struct Shared {
    /// Höchster Blockpegel seit dem letzten Abholen.
    peak_db: f32,
    fresh: bool,
}

/// Mikrofon für andere Programme einschalten: wohin ausgeben und mit welchen Werten.
pub struct VoiceSetup {
    /// Puffer zwischen Aufnahme und Ausgabe in ms; kleiner heißt weniger Verzögerung.
    pub buffer_ms: f32,
    pub output_id: Option<String>,
    pub control: Arc<ChainControl>,
}

pub struct Meter {
    _stream: cpal::Stream,
    _output: Option<cpal::Stream>,
    shared: Arc<Mutex<Shared>>,
    fault: Arc<Fault>,
    /// Letzte Samples fürs Frequenz-Diagramm.
    pub raw: Arc<RawSamples>,
    pub input_name: String,
    /// Abtastrate des Mikrofons; der Rauschfilter braucht 48 kHz.
    pub input_rate: u32,
    pub agc_output_name: Option<String>,
    pub agc_error: Option<String>,
}

impl Meter {
    /// Startet die Aufnahme. Ist `device_id` unbekannt, wird das Standardmikrofon genommen.
    /// Startet die Aufnahme vom ausgewählten Mikrofon. Ein Standardgerät gibt es bewusst nicht:
    /// Ist in Windows VB-Cable als Standard eingestellt, gäbe das eine Rückkopplung.
    pub fn start(device_id: &str, voice_setup: Option<VoiceSetup>) -> Result<Meter, String> {
        let host = cpal::default_host();
        let device = device_id
            .parse::<cpal::DeviceId>()
            .ok()
            .and_then(|id| host.device_by_id(&id))
            .ok_or_else(|| MISSING_DEVICE.to_string())?;
        let input_is_virtual = agc::is_virtual_device(&device_name(&device));

        let config = device
            .default_input_config()
            .map_err(|e| format!("Mikrofon lässt sich nicht öffnen: {}", describe(&e)))?;

        let shared = Arc::new(Mutex::new(Shared { peak_db: SILENCE_DB, fresh: false }));
        let fault = Arc::new(Fault::default());
        let raw = Arc::new(RawSamples::default());

        // Erst die Ausgabe öffnen: klappt das nicht, läuft die Ampel trotzdem weiter.
        let input_rate = config.sample_rate();
        let mut output = None;
        let mut agc_output_name = None;
        let mut agc_error = None;
        let mut chain = None;
        if voice_setup.is_some() && input_is_virtual {
            agc_error = Some(format!(
                "Als Mikrofon ist „{}“ gewählt, also VB-Cable selbst. Das gäbe eine Rückkopplung, \
                 deshalb ist der Kanalzug aus. Bitte oben dein echtes Mikrofon auswählen.",
                device_name(&device)
            ));
        } else if let Some(setup) = voice_setup {
            let (producer, consumer) = HeapRb::<f32>::new(input_rate as usize).split();
            let control = Arc::clone(&setup.control);
            match agc::start_output(
                setup.output_id.as_deref(),
                consumer,
                input_rate,
                setup.buffer_ms,
                Arc::clone(&control),
                Arc::clone(&fault),
            ) {
                Ok((stream, name)) => {
                    output = Some(stream);
                    agc_output_name = Some(name);
                    chain = Some(VoiceChain::new(input_rate, control, producer));
                }
                Err(e) => agc_error = Some(e),
            }
        }

        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, &shared, &fault, &raw, chain),
            SampleFormat::I16 => build::<i16>(&device, &config, &shared, &fault, &raw, chain),
            SampleFormat::I32 => build::<i32>(&device, &config, &shared, &fault, &raw, chain),
            SampleFormat::U16 => build::<u16>(&device, &config, &shared, &fault, &raw, chain),
            SampleFormat::U8 => build::<u8>(&device, &config, &shared, &fault, &raw, chain),
            other => return Err(format!("Nicht unterstütztes Audioformat: {other}")),
        }?;
        stream
            .play()
            .map_err(|e| format!("Aufnahme lässt sich nicht starten: {}", describe(&e)))?;

        Ok(Meter {
            _stream: stream,
            _output: output,
            shared,
            fault,
            raw,
            input_name: device_name(&device),
            input_rate,
            agc_output_name,
            agc_error,
        })
    }

    /// Lauteste Blockmessung seit dem letzten Aufruf, `None` wenn keine neuen Daten da sind.
    pub fn take_peak_db(&self) -> Option<f32> {
        let mut s = self.shared.lock().ok()?;
        if !s.fresh {
            return None;
        }
        let peak = s.peak_db;
        s.peak_db = SILENCE_DB;
        s.fresh = false;
        Some(peak)
    }

    /// Fehler, der einen Neustart der Aufnahme nötig macht.
    pub fn fault(&self) -> Option<String> {
        self.fault.get()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    shared: &Arc<Mutex<Shared>>,
    fault: &Arc<Fault>,
    raw: &Arc<RawSamples>,
    chain: Option<VoiceChain>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let sample_rate = config.sample_rate() as f32;
    let channels = config.channels().max(1) as usize;
    let mut proc = Processor::new(sample_rate, Arc::clone(shared), Arc::clone(raw), chain);
    let fault = Arc::clone(fault);

    device
        .build_input_stream(
            config.clone().into(),
            move |data: &[T], _: &_| {
                proc.report_block((data.len() / channels) as u32);
                // Nur der erste Kanal, beim Headset-Mikrofon sind die anderen identisch oder leer.
                for frame in data.chunks(channels) {
                    proc.push(f32::from_sample(frame[0]));
                }
            },
            move |err| fault.report(err),
            None,
        )
        .map_err(|e| format!("Aufnahme lässt sich nicht öffnen: {}", describe(&e)))
}

struct Processor {
    highpass: Biquad,
    lowpass: Biquad,
    sum_sq: f64,
    count: usize,
    block_len: usize,
    /// Wert, der noch nicht abgegeben werden konnte, weil der Mutex gerade belegt war.
    pending_db: f32,
    shared: Arc<Mutex<Shared>>,
    raw: Arc<RawSamples>,
    /// Sammelt Samples fürs Frequenz-Diagramm, damit der Ringpuffer blockweise gefüllt wird.
    raw_block: Vec<f32>,
    /// Rauschfilter, automatische Lautstärke und Ausgabe, falls eingeschaltet.
    chain: Option<VoiceChain>,
}

impl Processor {
    fn new(sample_rate: f32, shared: Arc<Mutex<Shared>>, raw: Arc<RawSamples>, chain: Option<VoiceChain>) -> Self {
        Self {
            highpass: Biquad::highpass(sample_rate, METER_HIGHPASS_HZ),
            lowpass: Biquad::lowpass(sample_rate, METER_LOWPASS_HZ.min(sample_rate * 0.45)),
            sum_sq: 0.0,
            count: 0,
            block_len: ((sample_rate * BLOCK_SECONDS) as usize).max(1),
            pending_db: SILENCE_DB,
            shared,
            raw,
            raw_block: Vec::with_capacity(RAW_BLOCK),
            chain,
        }
    }

    /// Wie groß der Block ist, den Windows uns gibt: der erste Teil der Verzögerung.
    fn report_block(&self, frames: u32) {
        if let Some(chain) = &self.chain {
            chain.report_input(frames);
        }
    }

    fn push(&mut self, x: f32) {
        self.raw_block.push(x);
        if self.raw_block.len() == RAW_BLOCK {
            self.raw.push_block(&self.raw_block);
            self.raw_block.clear();
        }
        if let Some(chain) = &mut self.chain {
            chain.push(x);
        }

        let y = self.lowpass.run(self.highpass.run(x));
        self.sum_sq += (y * y) as f64;
        self.count += 1;
        if self.count < self.block_len {
            return;
        }
        let rms = (self.sum_sq / self.count as f64).sqrt() as f32;
        self.sum_sq = 0.0;
        self.count = 0;
        let db = (20.0 * rms.max(1e-6).log10()).max(SILENCE_DB);
        self.pending_db = self.pending_db.max(db);

        // Im Audio-Thread nie blockieren.
        if let Ok(mut s) = self.shared.try_lock() {
            s.peak_db = s.peak_db.max(self.pending_db);
            s.fresh = true;
            self.pending_db = SILENCE_DB;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Anzeige soll echte dBFS zeigen: ein Ton mit RMS -63 dBFS muss als -63 dB ankommen.
    fn measure(freq: f32, rms_db: f32) -> f32 {
        let rate = 48_000.0;
        let shared = Arc::new(Mutex::new(Shared { peak_db: SILENCE_DB, fresh: false }));
        let mut proc = Processor::new(rate, Arc::clone(&shared), Arc::new(RawSamples::default()), None);
        let amplitude = 10f32.powf(rms_db / 20.0) * std::f32::consts::SQRT_2;
        // Eine Sekunde einschwingen lassen, dann den letzten Block ablesen.
        for i in 0..rate as usize {
            proc.push(amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / rate).sin());
            if i < rate as usize - 960 {
                shared.lock().unwrap().peak_db = SILENCE_DB;
            }
        }
        shared.lock().unwrap().peak_db
    }

    #[test]
    fn pegel_stimmt_in_dbfs() {
        for level in [-63.0, -40.0, -20.0] {
            let measured = measure(1000.0, level);
            assert!((measured - level).abs() < 1.0, "{level} dB gemessen als {measured:.1} dB");
        }
    }
}

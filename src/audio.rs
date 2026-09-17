//! Mikrofon einlesen und kurzzeitige Pegel (dBFS) bereitstellen.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use ringbuf::HeapRb;
use ringbuf::traits::Split;

use crate::agc::{self, AgcParams, VoiceChain};

/// Länge eines Messblocks. Kurz, damit die Anzeige sofort reagiert.
const BLOCK_SECONDS: f32 = 0.02;
/// Sprachbereich, alles außerhalb wird weggefiltert (Trittschall, Lüfter, Zischen).
const HIGHPASS_HZ: f32 = 100.0;
const LOWPASS_HZ: f32 = 4000.0;

pub const SILENCE_DB: f32 = -100.0;

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

struct Shared {
    /// Höchster Blockpegel seit dem letzten Abholen.
    peak_db: f32,
    fresh: bool,
}

/// Mikrofon für andere Programme einschalten: wohin ausgeben und mit welchen Werten.
pub struct VoiceSetup {
    pub output_id: Option<String>,
    pub params: Arc<AgcParams>,
}

pub struct Meter {
    _stream: cpal::Stream,
    _output: Option<cpal::Stream>,
    shared: Arc<Mutex<Shared>>,
    fault: Arc<Fault>,
    pub input_name: String,
    pub agc_output_name: Option<String>,
    pub agc_error: Option<String>,
}

impl Meter {
    /// Startet die Aufnahme. Ist `device_id` unbekannt, wird das Standardmikrofon genommen.
    pub fn start(device_id: Option<&str>, voice_setup: Option<VoiceSetup>) -> Result<Meter, String> {
        let host = cpal::default_host();
        let chosen = device_id.and_then(|s| s.parse::<cpal::DeviceId>().ok()).and_then(|id| host.device_by_id(&id));
        let explicitly_chosen = chosen.is_some();
        let mut device = chosen
            .or_else(|| host.default_input_device())
            .ok_or_else(|| "Kein Mikrofon gefunden".to_string())?;

        // Ist in Windows „CABLE Output“ das Standard-Mikrofon, würde die Lärmampel ihre eigene
        // Ausgabe wieder aufnehmen: Rückkopplung. Dann lieber das erste echte Mikrofon nehmen.
        if !explicitly_chosen && agc::is_virtual_device(&device_name(&device)) {
            if let Some(real) = host
                .input_devices()
                .ok()
                .and_then(|mut all| all.find(|d| !agc::is_virtual_device(&device_name(d))))
            {
                device = real;
            }
        }
        let input_is_virtual = agc::is_virtual_device(&device_name(&device));

        let config = device
            .default_input_config()
            .map_err(|e| format!("Mikrofon lässt sich nicht öffnen: {}", describe(&e)))?;

        let shared = Arc::new(Mutex::new(Shared { peak_db: SILENCE_DB, fresh: false }));
        let fault = Arc::new(Fault::default());

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
            match agc::start_output(setup.output_id.as_deref(), consumer, input_rate, Arc::clone(&fault)) {
                Ok((stream, name)) => {
                    output = Some(stream);
                    agc_output_name = Some(name);
                    chain = Some(VoiceChain::new(input_rate, setup.params, producer));
                }
                Err(e) => agc_error = Some(e),
            }
        }

        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, &shared, &fault, chain),
            SampleFormat::I16 => build::<i16>(&device, &config, &shared, &fault, chain),
            SampleFormat::I32 => build::<i32>(&device, &config, &shared, &fault, chain),
            SampleFormat::U16 => build::<u16>(&device, &config, &shared, &fault, chain),
            SampleFormat::U8 => build::<u8>(&device, &config, &shared, &fault, chain),
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
            input_name: device_name(&device),
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
    chain: Option<VoiceChain>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let sample_rate = config.sample_rate() as f32;
    let channels = config.channels().max(1) as usize;
    let mut proc = Processor::new(sample_rate, Arc::clone(shared), chain);
    let fault = Arc::clone(fault);

    device
        .build_input_stream(
            config.clone().into(),
            move |data: &[T], _: &_| {
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
    /// Rauschfilter, automatische Lautstärke und Ausgabe, falls eingeschaltet.
    chain: Option<VoiceChain>,
}

impl Processor {
    fn new(sample_rate: f32, shared: Arc<Mutex<Shared>>, chain: Option<VoiceChain>) -> Self {
        Self {
            highpass: Biquad::highpass(sample_rate, HIGHPASS_HZ),
            lowpass: Biquad::lowpass(sample_rate, LOWPASS_HZ.min(sample_rate * 0.45)),
            sum_sq: 0.0,
            count: 0,
            block_len: ((sample_rate * BLOCK_SECONDS) as usize).max(1),
            pending_db: SILENCE_DB,
            shared,
            chain,
        }
    }

    fn push(&mut self, x: f32) {
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

/// Biquad-Filter nach dem Audio EQ Cookbook (R. Bristow-Johnson).
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn highpass(sample_rate: f32, freq: f32) -> Self {
        let (cos, alpha) = Self::prewarp(sample_rate, freq);
        Self::normalized((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, cos, alpha)
    }

    fn lowpass(sample_rate: f32, freq: f32) -> Self {
        let (cos, alpha) = Self::prewarp(sample_rate, freq);
        Self::normalized((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0, cos, alpha)
    }

    fn prewarp(sample_rate: f32, freq: f32) -> (f32, f32) {
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let q = std::f32::consts::FRAC_1_SQRT_2;
        (w0.cos(), w0.sin() / (2.0 * q))
    }

    fn normalized(b0: f32, b1: f32, b2: f32, cos: f32, alpha: f32) -> Self {
        let a0 = 1.0 + alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

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
//! 10 ms verzögert heraus. Die *akustische* Verzögerung (Kopfhörer → Luft → Mikrofon) sucht
//! AEC3 selbst, aber nur in einem Fenster von ein paar hundert Millisekunden. Alles, was hier
//! an Vorlauf im Puffer steht, kommt oben drauf: staut sich die Wiedergabe an, liegt der Bezug
//! außerhalb dieses Fensters und der Filter findet ihn nie. Deshalb wird die Wiedergabe kurz
//! gehalten (siehe `MAX_REFERENCE_BLOCKS`).

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapRb};
use sonora::config::{EchoCanceller, MaxProcessingRate, Pipeline, TransparentModeType};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::audio::{Fault, describe};

/// Ein Block von 10 ms; darauf arbeitet AEC3.
const BLOCK_MS: usize = 10;
/// So viel Vorlauf darf die Wiedergabe haben, bevor Ältestes verworfen wird.
///
/// Zwei Blöcke (20 ms) reichen als Polster gegen den Ruckler zwischen zwei Audio-Threads und
/// bleiben weit innerhalb des Fensters, in dem AEC3 den Bezug noch findet. Vorher stand hier
/// eine halbe Sekunde – damit lag die Wiedergabe so weit hinter dem Mikrofon, dass gar nichts
/// mehr herausgerechnet wurde.
const MAX_REFERENCE_BLOCKS: usize = 2;
/// Wie oft nach den Kennzahlen des Filters gesehen wird (alle 50 Blöcke = zweimal pro Sekunde).
const STATS_EVERY_BLOCKS: u32 = 50;

/// Was die Echounterdrückung gerade tut – aus dem Audio-Thread für die Oberfläche.
#[derive(Debug)]
pub struct Status {
    /// Um wie viel dB das Echo leiser wird (ERLE), in Zehntel-dB; `i32::MIN` = noch nichts.
    erle_dbx10: AtomicI32,
    /// Verzögerung zwischen Wiedergabe und Mikrofon, die AEC3 gefunden hat; −1 = unbekannt.
    delay_ms: AtomicI32,
    /// Wie oft die Wiedergabe zu spät kam und mit Stille aufgefüllt werden musste.
    underruns: AtomicU32,
}

impl Status {
    fn report(&self, erle_db: Option<f64>, delay_ms: Option<i32>) {
        if let Some(db) = erle_db {
            self.erle_dbx10.store((db * 10.0) as i32, Ordering::Relaxed);
        }
        self.delay_ms.store(delay_ms.unwrap_or(-1), Ordering::Relaxed);
    }

    fn count_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Um wie viel dB das Echo gerade gedämpft wird.
    pub fn erle_db(&self) -> Option<f32> {
        match self.erle_dbx10.load(Ordering::Relaxed) {
            i32::MIN => None,
            x => Some(x as f32 / 10.0),
        }
    }

    /// Gefundene Verzögerung in ms.
    pub fn delay_ms(&self) -> Option<i32> {
        match self.delay_ms.load(Ordering::Relaxed) {
            -1 => None,
            ms => Some(ms),
        }
    }

    pub fn underruns(&self) -> u32 {
        self.underruns.load(Ordering::Relaxed)
    }

}

impl Default for Status {
    /// Frisch heißt: noch nichts gemessen (nicht „0 dB Dämpfung“).
    fn default() -> Self {
        Status { erle_dbx10: AtomicI32::new(i32::MIN), delay_ms: AtomicI32::new(-1), underruns: AtomicU32::new(0) }
    }
}

/// Läuft mit, was aus den Kopfhörern kommt.
pub struct Reference {
    _stream: cpal::Stream,
    pub name: String,
    pub rate: u32,
    samples: HeapCons<f32>,
}

impl Reference {
    /// Ein Block Wiedergabe, oder Stille, wenn gerade nichts läuft.
    ///
    /// Gibt `true` zurück, wenn aufgefüllt werden musste: dann ist die Wiedergabe gegenüber
    /// dem Mikrofon um genau diesen Block verrutscht, und AEC3 muss den Bezug neu suchen.
    fn take_block(&mut self, out: &mut [f32]) -> bool {
        let short = self.samples.occupied_len() < out.len();
        for slot in out.iter_mut() {
            *slot = self.samples.try_pop().unwrap_or(0.0);
        }
        short
    }

    /// Hat sich zu viel angestaut (z.B. nach einem Hänger), das Älteste wegwerfen.
    ///
    /// `block` ist ein Block in Samples dieses Geräts. Stehen mehr als
    /// `MAX_REFERENCE_BLOCKS` davon an, ist alles darüber alter Ton, der nur noch den
    /// Abstand zum Mikrofon vergrößert.
    fn trim(&mut self, block: usize) {
        let max = block * MAX_REFERENCE_BLOCKS;
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

/// Höchstens so viel zusätzlich dämpfen. Mehr würde man der Stimme anhören.
const EXTRA_DB: f32 = 8.0;
/// Darunter läuft praktisch kein Ton, dann gibt es auch kein Echo zu dämpfen.
const FAR_ACTIVE_DB: f32 = -60.0;
/// So weit über dem geschätzten Restecho muss der Pegel liegen, damit er als Stimme gilt.
const TALK_MARGIN_DB: f32 = 8.0;
/// Wie schnell die Dämpfung aufgebaut (gemächlich) und wieder aufgegeben wird (zügig,
/// damit der Anfang eines Wortes nicht abgeschnitten wird). Pro 10-ms-Block.
const DUCK_ATTACK_K: f32 = 0.10;
const DUCK_RELEASE_K: f32 = 0.50;
/// Wie schnell gelernt wird, wie laut das Restecho im Verhältnis zur Wiedergabe ist.
/// Nach unten schnell (findet den Boden), nach oben träge (sonst zieht die eigene
/// Stimme die Schätzung hoch und alles klingt wie Echo).
const COUPLING_DOWN_K: f32 = 0.20;
const COUPLING_UP_K: f32 = 0.002;
/// Solange jemand spricht, wird nach oben gar nicht gelernt – sonst hält die eigene
/// Stimme die Schätzung hoch, bis sie selbst als Echo gilt. Nach vier Sekunden
/// ununterbrochenem Sprechen wird die Sperre aufgegeben: dann hat sich vermutlich der
/// Abstand zwischen Kopfhörer und Mikrofon geändert und die Schätzung muss nachziehen.
const TALK_FREEZE_BLOCKS: u32 = 400;
/// Tempo, mit dem die Schätzung nach dieser Sperre nachzieht. Schneller als das normale
/// Lernen nach oben (sonst dauert es eine halbe Minute), aber langsam genug, dass echtes
/// langes Reden längst vorbei ist, bevor es die Stimme erreicht.
const COUPLING_RECOVER_K: f32 = 0.004;

fn rms_db(block: &[f32]) -> f32 {
    if block.is_empty() {
        return -120.0;
    }
    let power = block.iter().map(|&x| (x * x) as f64).sum::<f64>() / block.len() as f64;
    (10.0 * power.max(1e-12).log10()) as f32
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Nachbrenner für das, was AEC3 stehen lässt.
///
/// AEC3 rechnet den *linearen* Anteil des Echos heraus. Was ein kleiner Kopfhörer-Treiber
/// daraus verzerrt und was vom Tisch zurückkommt, bleibt stehen – der Filter schont im
/// Zweifel lieber die Stimme. Hier kommt darauf noch etwas Dämpfung, aber nur solange es
/// wirklich nur Restecho ist: wer redet, während Ton läuft, soll nicht mit abgesenkt werden.
///
/// Dafür wird laufend gelernt, wie laut der Rest im Verhältnis zur Wiedergabe ist. Liegt
/// der Pegel auf diesem Niveau, ist es Echo; liegt er deutlich darüber, redet jemand.
#[derive(Debug)]
struct Residual {
    /// Rest zu Wiedergabe in dB, wie es sich einpendelt, wenn niemand spricht.
    coupling_db: f32,
    /// Dämpfung, die gerade anliegt (dB, positiv = leiser).
    duck_db: f32,
    /// Blöcke am Stück, in denen es nach Stimme aussah.
    talking_blocks: u32,
}

impl Default for Residual {
    fn default() -> Self {
        // 0 dB heißt „Rest so laut wie die Wiedergabe“ – der Anfangswert wird in den ersten
        // Blöcken nach unten korrigiert, sobald echte Zahlen da sind.
        Residual { coupling_db: 0.0, duck_db: 0.0, talking_blocks: 0 }
    }
}

impl Residual {
    /// Verstärkung für diesen Block. `far_db` ist die Wiedergabe, `near_db` das, was nach
    /// AEC3 übrig ist (beide vor dieser Dämpfung gemessen).
    fn gain(&mut self, far_db: f32, near_db: f32) -> f32 {
        let target = if far_db < FAR_ACTIVE_DB {
            // Kein Ton, kein Echo: loslassen, aber das Gelernte behalten.
            self.talking_blocks = 0;
            0.0
        } else {
            let ratio = (near_db - far_db).clamp(-90.0, 6.0);
            // Erst mit dem Stand von eben einschätzen, ob das hier Stimme ist …
            let residual_db = far_db + self.coupling_db;
            let speech = ((near_db - residual_db) / TALK_MARGIN_DB).clamp(0.0, 1.0);

            // … und erst dann lernen. Nach unten immer, nach oben nur, wenn es gerade
            // nicht nach Stimme aussieht oder schon sehr lange am Stück danach aussieht.
            if speech > 0.5 {
                self.talking_blocks = self.talking_blocks.saturating_add(1);
            } else {
                self.talking_blocks = 0;
            }
            let talking = speech > 0.5;
            let k = match (ratio < self.coupling_db, talking, self.talking_blocks >= TALK_FREEZE_BLOCKS) {
                // Nach unten immer sofort: da findet sich der Boden.
                (true, _, _) => COUPLING_DOWN_K,
                // Sieht nach Stimme aus und das noch nicht lange: gar nicht lernen.
                (false, true, false) => 0.0,
                // Schon sehr lange „Stimme“: dann stimmt eher die Schätzung nicht mehr.
                (false, true, true) => COUPLING_RECOVER_K,
                (false, false, _) => COUPLING_UP_K,
            };
            self.coupling_db += (ratio - self.coupling_db) * k;

            EXTRA_DB * (1.0 - speech)
        };
        let k = if target > self.duck_db { DUCK_ATTACK_K } else { DUCK_RELEASE_K };
        self.duck_db += (target - self.duck_db) * k;
        db_to_gain(-self.duck_db)
    }
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
    /// Nimmt weg, was AEC3 stehen lässt.
    residual: Residual,
    /// Kennzahlen für die Oberfläche.
    status: Arc<Status>,
    /// Blöcke seit dem letzten Blick auf die Kennzahlen.
    since_stats: u32,
}

impl Echo {
    pub fn new(mic_rate: u32, reference: Reference, status: Arc<Status>) -> Self {
        let config = Config {
            pipeline: Pipeline {
                // Standard wären 32 kHz: dann rechnet AEC3 nur bis 16 kHz, alles darüber
                // bleibt unangetastet – und genau da sitzt viel von dem, was aus kleinen
                // Kopfhörer-Treibern ins Mikrofon zischt. Dazu spart es zwei Umrechnungen
                // (48 → 32 → 48 kHz) auf der Stimme.
                maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
                // Hier läuft beides einkanalig, also gar nicht erst den Mehrkanal-Weg öffnen.
                multi_channel_render: false,
                multi_channel_capture: false,
                ..Default::default()
            },
            // Nur das Echo: Rauschen und Lautstärke macht die eigene Kette.
            echo_canceller: Some(EchoCanceller {
                enforce_high_pass_filtering: true,
                // Erkennt selbst, wenn gar kein Echo da ist, und hält sich dann zurück.
                // Der Zähler-Ansatz (Legacy) braucht dafür lange: nimmt man das Headset ab,
                // hält er noch eine ganze Weile „kein Echo da“ fest und lässt es durch.
                // Das Markov-Modell schaltet in beide Richtungen schneller um.
                transparent_mode: TransparentModeType::Hmm,
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
            residual: Residual::default(),
            status,
            since_stats: 0,
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
            let block = self.ref_in.len();
            self.reference.trim(block);
            if self.reference.take_block(&mut self.ref_in) {
                self.status.count_underrun();
            }
            let _ = self.apm.process_render_f32(&[&self.ref_in], &mut [&mut self.ref_out]);
            if self.apm.process_capture_f32(&[&self.mic_in], &mut [&mut self.mic_out]).is_ok() {
                // Gemessen wird vor der eigenen Dämpfung, sonst regelt sie sich selbst nach.
                let gain = self.residual.gain(rms_db(&self.ref_in), rms_db(&self.mic_out));
                self.ready.extend(self.mic_out.iter().map(|&x| x * gain));
            } else {
                // Geht etwas schief, lieber unbearbeitet weiterreichen als Stille.
                self.ready.extend(self.mic_in.iter().copied());
            }
            self.mic_in.clear();

            self.since_stats += 1;
            if self.since_stats >= STATS_EVERY_BLOCKS {
                self.since_stats = 0;
                let stats = self.apm.statistics();
                // Was der Nachbrenner gerade zusätzlich wegnimmt, gehört zur Zahl dazu:
                // gefragt ist „wie viel leiser ist das Echo“, nicht „was tut AEC3“.
                let erle = stats.echo_return_loss_enhancement.map(|db| db + self.residual.duck_db as f64);
                self.status.report(erle, stats.delay_ms);
            }
        }
        self.ready.pop_front().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dämpfung in dB nach `blocks` Blöcken mit immer denselben Pegeln.
    fn settle(far_db: f32, near_db: f32, blocks: usize) -> f32 {
        let mut residual = Residual::default();
        let mut gain = 1.0;
        for _ in 0..blocks {
            gain = residual.gain(far_db, near_db);
        }
        -20.0 * gain.log10()
    }

    /// Läuft Ton und bleibt nur leises Restecho übrig, wird das zusätzlich gedämpft.
    #[test]
    fn restecho_wird_zusaetzlich_gedaempft() {
        let damping = settle(-20.0, -45.0, 200);
        assert!(damping > EXTRA_DB - 0.5, "nur {damping:.1} dB gedämpft");
    }

    /// Wer redet, während Ton läuft, bleibt laut: der Pegel liegt deutlich über dem Rest.
    #[test]
    fn stimme_bleibt_stehen() {
        let mut residual = Residual::default();
        // Erst eine Sekunde nur Echo: der Nachbrenner lernt den Rest und dämpft.
        for _ in 0..100 {
            residual.gain(-20.0, -45.0);
        }
        // Dann spricht jemand dazwischen.
        let mut gain = 1.0;
        for _ in 0..20 {
            gain = residual.gain(-20.0, -25.0);
        }
        let damping = -20.0 * gain.log10();
        assert!(damping < 1.0, "Stimme um {damping:.1} dB abgesenkt");
    }

    /// Ohne Wiedergabe wird gar nichts gedämpft.
    #[test]
    fn ohne_ton_keine_daempfung() {
        let damping = settle(-90.0, -30.0, 100);
        assert!(damping.abs() < 0.1, "{damping:.1} dB ohne Grund gedämpft");
    }

    /// Langes Durchreden über laufendem Ton darf die Stimme nicht doch noch abwürgen.
    #[test]
    fn langes_reden_wird_nicht_zu_echo() {
        let mut residual = Residual::default();
        for _ in 0..100 {
            residual.gain(-20.0, -45.0);
        }
        // Fünf Sekunden am Stück sprechen, ohne eine einzige Pause.
        let mut gain = 1.0;
        for _ in 0..500 {
            gain = residual.gain(-20.0, -25.0);
        }
        let damping = -20.0 * gain.log10();
        assert!(damping < 1.0, "Stimme nach langem Reden um {damping:.1} dB abgesenkt");
    }

    /// Rückt der Kopfhörer näher ans Mikrofon, muss die Schätzung nachziehen.
    #[test]
    fn schaetzung_zieht_nach() {
        let mut residual = Residual::default();
        for _ in 0..200 {
            residual.gain(-20.0, -45.0);
        }
        // Ab jetzt ist das Echo dauerhaft 20 dB lauter – das sieht erst wie Stimme aus.
        let mut gain = 1.0;
        for _ in 0..2000 {
            gain = residual.gain(-20.0, -25.0);
        }
        let damping = -20.0 * gain.log10();
        assert!(damping > EXTRA_DB - 1.0, "nach dem Nachziehen nur {damping:.1} dB gedämpft");
    }

    /// Ein lauter Kopfhörer dicht am Mikrofon: auch dann gilt der Pegel als Echo.
    #[test]
    fn lautes_echo_wird_erkannt() {
        let damping = settle(-15.0, -18.0, 200);
        assert!(damping > EXTRA_DB - 0.5, "nur {damping:.1} dB gedämpft");
    }
}

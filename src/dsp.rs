//! Die eigentliche Bearbeitung des Mikrofons: Gate → Comp. → Fader/Mute → Limiter.
//!
//! Läuft auf dem Weg über VB-Cable im Audio-Thread: kein Anlegen von Speicher im laufenden Betrieb.

const GATE_LEVEL_WINDOW_MS: f32 = 10.0;
const COMP_LEVEL_WINDOW_MS: f32 = 50.0;
/// Liegt der Pegel so weit unter dem letzten Höchstwert, klingt gerade ein Wort aus.
const TAIL_DB: f32 = 10.0;
const PEAK_DECAY_DB_PER_SECOND: f32 = 10.0;
const LIMITER_RELEASE_MS: f32 = 100.0;
/// Fader und Mute weich überblenden, sonst knackt es.
const FADER_SMOOTHING_MS: f32 = 10.0;
/// Darunter ist es sicher keine Stimme und die volle Absenkung greift.
const VAD_NOISE: f32 = 0.1;
/// Zurück auf volle Lautstärke schnell (kein abgeschnittenes erstes Wort), absenken langsam.
const DUCK_OPEN_MS: f32 = 4.0;
const DUCK_CLOSE_MS: f32 = 150.0;
const METER_WINDOW_MS: f32 = 50.0;
/// Sprachbereich für die Pegelmessung. Beide Anzeigen (Ampel und Kanalzug) messen damit
/// dasselbe: ohne diesen Filter zählt Brummen unter 100 Hz mit und der Kanalzug stünde
/// deutlich höher als die Ampel, obwohl gar nichts bearbeitet wird.
pub const METER_HIGHPASS_HZ: f32 = 100.0;
pub const METER_LOWPASS_HZ: f32 = 4000.0;

/// Biquad-Filter nach dem Audio EQ Cookbook (R. Bristow-Johnson).
pub struct Biquad {
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
    pub fn highpass(sample_rate: f32, freq: f32) -> Self {
        let (cos, alpha) = Self::prewarp(sample_rate, freq);
        Self::normalized((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, cos, alpha)
    }

    pub fn lowpass(sample_rate: f32, freq: f32) -> Self {
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

    pub fn run(&mut self, x: f32) -> f32 {
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


pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn smoothing(ms: f32, sample_rate: f32) -> f32 {
    if ms <= 0.0 { 1.0 } else { 1.0 - (-1000.0 / (ms * sample_rate)).exp() }
}

fn power_db(power: f32) -> f32 {
    10.0 * power.max(1e-12).log10()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    /// Wie viel leiser im geschlossenen Zustand.
    pub range_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompSettings {
    pub enabled: bool,
    /// Ziel-Sprachpegel in dBFS (RMS).
    pub target_db: f32,
    pub max_gain_db: f32,
    pub max_cut_db: f32,
    /// Wie schnell runtergeregelt wird.
    pub attack_ms: f32,
    /// Wie schnell hochgeregelt wird.
    pub release_ms: f32,
    /// Darunter gilt es als Sprechpause, die Verstärkung wird gehalten.
    pub pause_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Rauschfilter (RNNoise). Braucht 48 kHz, sonst wird er übersprungen.
    pub denoise: bool,
    /// Ab dieser Sprachwahrscheinlichkeit gilt es als Stimme und es wird nicht abgesenkt.
    /// Höher heißt strenger: dann muss sich das Netz sicher sein, sonst wird abgesenkt.
    pub denoise_speech_vad: f32,
    /// Zusätzliche Absenkung in Sprechpausen, gesteuert vom Netz selbst (0 = aus).
    /// Dafür sorgt der Filter beim Sprechen weiter allein; erst wenn er keine Stimme
    /// erkennt, wird zusätzlich leiser gemacht. So verschwindet auch lauter Krach
    /// (Bohrmaschine, Lüfter), während die Stimme unangetastet bleibt.
    pub denoise_duck_db: f32,
    /// Wie viel vom ungefilterten Ton stehen bleibt (0 = voll gefiltert, 0.32 = höchstens
    /// 10 dB leiser). Damit gibt es Stufen: etwas Restrauschen klingt natürlicher als ein
    /// Filter, der in Sprechpausen alles totmacht.
    pub denoise_dry: f32,
    pub gate: GateSettings,
    pub comp: CompSettings,
    pub fader_db: f32,
    pub muted: bool,
    /// 0 dB = Limiter aus.
    pub ceiling_db: f32,
}

impl Default for Settings {
    /// Alles aus: das Signal geht unverändert durch.
    fn default() -> Self {
        Self {
            denoise: false,
            denoise_speech_vad: 0.6,
            denoise_duck_db: 0.0,
            denoise_dry: 0.0,
            gate: GateSettings { enabled: false, threshold_db: -45.0, range_db: 40.0, attack_ms: 2.0, hold_ms: 250.0, release_ms: 150.0 },
            comp: CompSettings {
                enabled: false,
                target_db: -20.0,
                max_gain_db: 0.0,
                max_cut_db: 0.0,
                attack_ms: 40.0,
                release_ms: 1500.0,
                pause_db: -50.0,
            },
            fader_db: 0.0,
            muted: false,
            ceiling_db: 0.0,
        }
    }
}

/// Wie stark das Gate unter der Schwelle absenkt: pro dB darunter so viele dB zusätzlich.
/// Dadurch gleitend statt hart: je höher das Gate, desto niedriger der Pegel (wie bei Voicemeeter).
pub const GATE_RATIO: f32 = 4.0;

/// Absenkung in dB für einen Pegel, gleitend unter der Schwelle, höchstens `range_db`.
pub fn gate_reduction_db(level_db: f32, threshold_db: f32, range_db: f32) -> f32 {
    if level_db >= threshold_db {
        0.0
    } else {
        ((threshold_db - level_db) * (GATE_RATIO - 1.0)).min(range_db.max(0.0))
    }
}

/// Gilt als offen, solange kaum abgesenkt wird.
pub const GATE_OPEN_BELOW_DB: f32 = 1.0;

/// Gleitendes Gate (Expander): unter der Schwelle umso leiser, je weiter darunter.
pub struct Gate {
    sample_rate: f32,
    level_k: f32,
    attack_k: f32,
    release_k: f32,
    hold_samples: usize,
    threshold_db: f32,
    range_db: f32,

    power: f32,
    reduction_db: f32,
    hold_left: usize,
}

impl Gate {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            level_k: smoothing(GATE_LEVEL_WINDOW_MS, sample_rate),
            attack_k: 1.0,
            release_k: 1.0,
            hold_samples: 0,
            threshold_db: -100.0,
            range_db: 0.0,
            power: 0.0,
            reduction_db: 0.0,
            hold_left: 0,
        }
    }

    pub fn configure(&mut self, s: &GateSettings) {
        self.attack_k = smoothing(s.attack_ms, self.sample_rate);
        self.release_k = smoothing(s.release_ms, self.sample_rate);
        self.threshold_db = s.threshold_db;
        self.range_db = s.range_db.max(0.0);
        self.hold_samples = (s.hold_ms.max(0.0) / 1000.0 * self.sample_rate) as usize;
    }

    /// Verstärkung für dieses Sample, gemessen am unbearbeiteten Signal.
    pub fn gain(&mut self, x: f32) -> f32 {
        self.power += (x * x - self.power) * self.level_k;
        let level_db = power_db(self.power);

        let mut target = gate_reduction_db(level_db, self.threshold_db, self.range_db);
        if level_db >= self.threshold_db {
            self.hold_left = self.hold_samples;
        } else if self.hold_left > 0 {
            // Nach dem letzten Wort kurz nicht stärker absenken, damit Wortenden nicht abreißen.
            self.hold_left -= 1;
            target = target.min(self.reduction_db);
        }

        // Aufgehen (weniger Absenkung) mit „Öffnen“, Zugehen mit „Schließen“.
        let k = if target < self.reduction_db { self.attack_k } else { self.release_k };
        self.reduction_db += (target - self.reduction_db) * k;
        db_to_gain(-self.reduction_db)
    }

    pub fn is_open(&self) -> bool {
        self.reduction_db < GATE_OPEN_BELOW_DB
    }

    pub fn level_db(&self) -> f32 {
        power_db(self.power)
    }
}

/// Automatische Lautstärke: leise hoch, laut runter, in Pausen nichts hochziehen.
pub struct Comp {
    sample_rate: f32,
    level_k: f32,
    attack_k: f32,
    release_k: f32,
    settings: CompSettings,

    power: f32,
    recent_peak_db: f32,
    gain_db: f32,
}

impl Comp {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            level_k: smoothing(COMP_LEVEL_WINDOW_MS, sample_rate),
            attack_k: 1.0,
            release_k: 1.0,
            settings: Settings::default().comp,
            power: 0.0,
            recent_peak_db: -120.0,
            gain_db: 0.0,
        }
    }

    pub fn configure(&mut self, s: &CompSettings) {
        self.attack_k = smoothing(s.attack_ms, self.sample_rate);
        self.release_k = smoothing(s.release_ms, self.sample_rate);
        self.settings = *s;
    }

    /// Verstärkung für dieses Sample (linear).
    pub fn gain(&mut self, x: f32) -> f32 {
        let s = &self.settings;
        self.power += (x * x - self.power) * self.level_k;
        let level_db = power_db(self.power);
        self.recent_peak_db = (self.recent_peak_db - PEAK_DECAY_DB_PER_SECOND / self.sample_rate).max(level_db);

        // In Sprechpausen die Verstärkung halten, sonst wird das Rauschen hochgezogen.
        if level_db > s.pause_db {
            let wanted = (s.target_db - level_db).clamp(-s.max_cut_db.max(0.0), s.max_gain_db.max(0.0));
            if wanted < self.gain_db {
                self.gain_db += (wanted - self.gain_db) * self.attack_k;
            } else if level_db > self.recent_peak_db - TAIL_DB {
                // Nicht hochregeln, während ein Wort ausklingt.
                self.gain_db += (wanted - self.gain_db) * self.release_k;
            }
        }
        db_to_gain(self.gain_db)
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_db
    }
}

/// Lässt keine Spitze über die Obergrenze: sofort zupacken, langsam loslassen.
struct Limiter {
    release_k: f32,
    envelope: f32,
}

impl Limiter {
    fn new(sample_rate: f32) -> Self {
        Self { release_k: smoothing(LIMITER_RELEASE_MS, sample_rate), envelope: 0.0 }
    }

    fn gain(&mut self, peak: f32, ceiling: f32) -> f32 {
        self.envelope = if peak > self.envelope { peak } else { self.envelope + (peak - self.envelope) * self.release_k };
        if self.envelope > ceiling { ceiling / self.envelope } else { 1.0 }
    }
}

/// Rauschfilter auf Sprache trainiert (RNNoise). Arbeitet in Blöcken von 10 ms bei 48 kHz.
///
/// Heraus kommt das Signal um **zwei** Blöcke verzögert (20 ms): einen sammelt diese Stufe,
/// bevor überhaupt gerechnet wird, und einen hält RNNoise selbst zurück, weil es seine
/// Fenster überlappend zusammensetzt. Nachgemessen mit einem Impuls (siehe Tests).
/// Um so viele Samples muss der Trockenweg warten, damit er zum gefilterten Signal passt.
/// Ein Block sammelt diese Stufe selbst, einen hält RNNoise zurück – zusammen 20 ms.
/// Stand hier nur ein Block, lagen beim Beimischen zwei um 10 ms versetzte Signale
/// übereinander: die Stimme verlor rund 5 dB und klang verschmiert.
const DRY_DELAY: usize = 2 * nnnoiseless::DenoiseState::FRAME_SIZE;

struct Denoiser {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    input: Vec<f32>,
    output: Vec<f32>,
    /// Fertige, noch nicht abgeholte Samples.
    ready: std::collections::VecDeque<f32>,
    /// Das unbearbeitete Signal, um dieselbe Zeit verzögert wie das gefilterte.
    /// Ohne diese Verzögerung würde das Beimischen den Ton verschmieren.
    dry: std::collections::VecDeque<f32>,
    /// Wie sicher das Netz beim letzten Block Sprache gehört hat (0 bis 1).
    vad: f32,
}

impl Denoiser {
    fn new() -> Self {
        let frame = nnnoiseless::DenoiseState::FRAME_SIZE;
        Self {
            state: nnnoiseless::DenoiseState::new(),
            input: Vec::with_capacity(frame),
            output: vec![0.0; frame],
            ready: std::collections::VecDeque::with_capacity(frame * 2),
            dry: std::collections::VecDeque::with_capacity(DRY_DELAY + 1),
            vad: 1.0,
        }
    }

    /// Halbfertigen und fertigen Block wegwerfen. Sonst käme beim nächsten Einschalten
    /// zuerst der 10-ms-Block von vorhin heraus.
    fn reset(&mut self) {
        self.input.clear();
        self.ready.clear();
        self.dry.clear();
    }

    /// Ein Sample hinein, ein (verzögertes) Sample heraus.
    /// `dry_mix` ist der Anteil, der ungefiltert stehen bleibt (Stärke des Filters).
    fn process(&mut self, x: f32, dry_mix: f32) -> f32 {
        self.input.push(x * 32768.0);
        if self.input.len() == nnnoiseless::DenoiseState::FRAME_SIZE {
            self.vad = self.state.process_frame(&mut self.output, &self.input);
            self.input.clear();
            for &y in &self.output {
                self.ready.push_back(y / 32768.0);
            }
        }
        self.dry.push_back(x);
        // Bis der erste Block fertig ist, kommt Stille heraus.
        let wet = self.ready.pop_front().unwrap_or(0.0);
        let dry = if self.dry.len() >= DRY_DELAY { self.dry.pop_front().unwrap_or(0.0) } else { 0.0 };
        // Überblendung statt Addition: bei Sprache sind beide gleich, die Lautstärke
        // bleibt also stehen; in Pausen bestimmt `dry_mix`, wie viel Rauschen übrig ist.
        wet + (dry - wet) * dry_mix.clamp(0.0, 1.0)
    }
}

/// Die ganze Kette für ein Mikrofon mit beliebig vielen Kanälen.
pub struct Chain {
    settings: Settings,
    /// Nur bei 48 kHz vorhanden: RNNoise arbeitet nur mit dieser Abtastrate.
    denoiser: Option<Denoiser>,
    configured: bool,
    gate: Gate,
    comp: Comp,
    limiter: Limiter,
    ceiling: f32,

    fader_k: f32,
    fader_gain: f32,

    /// Aktuelle Absenkung in Sprechpausen und wie schnell sie sich bewegt.
    duck_db: f32,
    duck_open_k: f32,
    duck_close_k: f32,

    meter_k: f32,
    meter_hp: Biquad,
    meter_lp: Biquad,
    out_power: f32,
}

impl Chain {
    pub fn new(sample_rate: f32) -> Self {
        let mut chain = Self {
            settings: Settings::default(),
            denoiser: (sample_rate as u32 == 48_000).then(Denoiser::new),
            configured: false,
            gate: Gate::new(sample_rate),
            comp: Comp::new(sample_rate),
            limiter: Limiter::new(sample_rate),
            ceiling: 1.0,
            fader_k: smoothing(FADER_SMOOTHING_MS, sample_rate),
            fader_gain: 1.0,
            duck_db: 0.0,
            duck_open_k: smoothing(DUCK_OPEN_MS, sample_rate),
            duck_close_k: smoothing(DUCK_CLOSE_MS, sample_rate),
            meter_k: smoothing(METER_WINDOW_MS, sample_rate),
            meter_hp: Biquad::highpass(sample_rate, METER_HIGHPASS_HZ),
            meter_lp: Biquad::lowpass(sample_rate, METER_LOWPASS_HZ.min(sample_rate * 0.45)),
            out_power: 0.0,
        };
        chain.set(Settings::default());
        chain
    }

    /// Neue Einstellungen übernehmen; billig, wenn sich nichts geändert hat.
    pub fn set(&mut self, settings: Settings) {
        if self.configured && settings == self.settings {
            return;
        }
        if settings.denoise != self.settings.denoise
            && let Some(denoiser) = &mut self.denoiser
        {
            denoiser.reset();
        }
        self.configured = true;
        self.gate.configure(&settings.gate);
        self.comp.configure(&settings.comp);
        self.ceiling = db_to_gain(settings.ceiling_db.min(0.0));
        self.settings = settings;
    }

    /// Bearbeitet ein Frame (ein Sample pro Kanal) direkt im Puffer.
    pub fn process_frame(&mut self, frame: &mut [f32]) {
        if frame.is_empty() {
            return;
        }
        // Gemessen wird am Mittel aller Kanäle, alle Kanäle bekommen dieselbe Verstärkung.
        let mut mono = frame.iter().sum::<f32>() / frame.len() as f32;

        // Rauschfilter ersetzt das Signal; danach zählt nur noch der gefilterte Ton.
        if self.settings.denoise
            && let Some(denoiser) = &mut self.denoiser
        {
            mono = denoiser.process(mono, self.settings.denoise_dry);
            // Erkennt das Netz keine Stimme, zusätzlich absenken. Zwischen den beiden
            // Schwellen wird gleitend übergeblendet, sonst pumpt es hörbar.
            let speech_vad = self.settings.denoise_speech_vad.max(VAD_NOISE + 0.05);
            let speech = ((denoiser.vad - VAD_NOISE) / (speech_vad - VAD_NOISE)).clamp(0.0, 1.0);
            let target = self.settings.denoise_duck_db.max(0.0) * (1.0 - speech);
            let k = if target < self.duck_db { self.duck_open_k } else { self.duck_close_k };
            self.duck_db += (target - self.duck_db) * k;
            mono *= db_to_gain(-self.duck_db);
            frame.fill(mono);
        }

        let mut gain = 1.0;
        if self.settings.gate.enabled {
            gain *= self.gate.gain(mono);
        }
        if self.settings.comp.enabled {
            gain *= self.comp.gain(mono * gain);
        }
        let fader_target = if self.settings.muted { 0.0 } else { db_to_gain(self.settings.fader_db.clamp(-60.0, 24.0)) };
        self.fader_gain += (fader_target - self.fader_gain) * self.fader_k;
        gain *= self.fader_gain;

        let peak = frame.iter().fold(0.0f32, |m, s| m.max((s * gain).abs()));
        gain *= self.limiter.gain(peak, self.ceiling);

        for sample in frame.iter_mut() {
            *sample = (*sample * gain).clamp(-self.ceiling, self.ceiling);
        }

        // Genauso gemessen wie die Ampel: erst in den Sprachbereich filtern.
        let out = self.meter_lp.run(self.meter_hp.run(mono * gain));
        self.out_power += (out * out - self.out_power) * self.meter_k;
    }

    pub fn out_level_db(&self) -> f32 {
        power_db(self.out_power)
    }

    pub fn gate_open(&self) -> bool {
        !self.settings.gate.enabled || self.gate.is_open()
    }

    pub fn gate_level_db(&self) -> f32 {
        self.gate.level_db()
    }

    /// Wie sicher der Rauschfilter gerade Sprache hört. Ohne Filter gilt alles als Sprache.
    pub fn speech(&self) -> f32 {
        if self.settings.denoise { self.denoiser.as_ref().map_or(1.0, |d| d.vad) } else { 1.0 }
    }

    /// Absenkung, die der Filter gerade zusätzlich fährt, weil er keine Stimme hört.
    pub fn duck_db(&self) -> f32 {
        if self.settings.denoise { self.duck_db } else { 0.0 }
    }

    pub fn comp_gain_db(&self) -> f32 {
        if self.settings.comp.enabled { self.comp.gain_db() } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn comp_settings() -> Settings {
        let mut s = Settings::default();
        s.comp = CompSettings {
            enabled: true,
            target_db: -20.0,
            max_gain_db: 18.0,
            max_cut_db: 18.0,
            attack_ms: 40.0,
            release_ms: 1500.0,
            pause_db: -50.0,
        };
        s.ceiling_db = -1.0;
        s
    }

    fn gate_settings() -> Settings {
        let mut s = Settings::default();
        s.gate = GateSettings { enabled: true, threshold_db: -40.0, range_db: 40.0, attack_ms: 2.0, hold_ms: 200.0, release_ms: 50.0 };
        s
    }

    fn sine(n: usize, rms_db: f32) -> Vec<f32> {
        let amplitude = db_to_gain(rms_db) * std::f32::consts::SQRT_2;
        (0..n).map(|i| amplitude * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin()).collect()
    }

    fn rms_db(samples: &[f32]) -> f32 {
        let power = samples.iter().map(|&x| (x * x) as f64).sum::<f64>() / samples.len() as f64;
        10.0 * power.log10() as f32
    }

    /// Mono durch die Kette schicken.
    fn run(chain: &mut Chain, input: &[f32]) -> Vec<f32> {
        input
            .iter()
            .map(|&x| {
                let mut frame = [x];
                chain.process_frame(&mut frame);
                frame[0]
            })
            .collect()
    }

    fn tail_rms(samples: &[f32]) -> f32 {
        rms_db(&samples[samples.len() - RATE as usize / 2..])
    }

    #[test]
    fn leise_wird_angehoben() {
        let mut chain = Chain::new(RATE);
        chain.set(comp_settings());
        let out = run(&mut chain, &sine(RATE as usize * 10, -32.0));
        assert!((tail_rms(&out) - -20.0).abs() < 1.0, "Ausgang {} dB", tail_rms(&out));
    }

    #[test]
    fn laut_wird_abgesenkt() {
        let mut chain = Chain::new(RATE);
        chain.set(comp_settings());
        let out = run(&mut chain, &sine(RATE as usize * 3, -8.0));
        assert!((tail_rms(&out) - -20.0).abs() < 1.0, "Ausgang {} dB", tail_rms(&out));
    }

    #[test]
    fn verstaerkung_ist_begrenzt() {
        let mut chain = Chain::new(RATE);
        chain.set(comp_settings());
        let out = run(&mut chain, &sine(RATE as usize * 15, -45.0));
        assert!((tail_rms(&out) - -27.0).abs() < 1.0, "Ausgang {} dB, erwartet -45 + 18", tail_rms(&out));
    }

    #[test]
    fn pause_zieht_rauschen_nicht_hoch() {
        let mut chain = Chain::new(RATE);
        chain.set(comp_settings());
        run(&mut chain, &sine(RATE as usize * 3, -20.0));
        let before = chain.comp_gain_db();
        run(&mut chain, &sine(RATE as usize * 5, -65.0));
        assert!((chain.comp_gain_db() - before).abs() < 0.5, "Verstärkung {before} -> {}", chain.comp_gain_db());
    }

    /// Gleichmäßiges Rauschen, reproduzierbar ohne Zufallsbibliothek.
    fn noise(n: usize, level: f32) -> Vec<f32> {
        let mut state = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * level
            })
            .collect()
    }

    /// Zwei-Pol-Resonator, reicht als Formant für ein künstliches „ah“.
    struct Formant {
        cos: f32,
        r: f32,
        y1: f32,
        y2: f32,
    }

    impl Formant {
        fn new(freq: f32, bandwidth: f32) -> Self {
            let r = (-std::f32::consts::PI * bandwidth / RATE).exp();
            Formant { cos: (2.0 * std::f32::consts::PI * freq / RATE).cos(), r, y1: 0.0, y2: 0.0 }
        }

        fn run(&mut self, x: f32) -> f32 {
            let y = x + 2.0 * self.r * self.cos * self.y1 - self.r * self.r * self.y2;
            self.y2 = self.y1;
            self.y1 = y;
            y
        }
    }

    /// Grob wie ein gesprochener Vokal: Impulsfolge auf Grundfrequenz, durch drei Formanten
    /// gefiltert, dazu eine Silbenhüllkurve. Kein echtes Sprachsignal, aber nah genug dran,
    /// dass die Sprach-Erkennung von RNNoise darauf anspringen sollte.
    fn vowel(n: usize, ziel_db: f32) -> Vec<f32> {
        let f0 = 110.0;
        let period = (RATE / f0) as usize;
        let mut formants = [Formant::new(700.0, 80.0), Formant::new(1220.0, 90.0), Formant::new(2600.0, 120.0)];
        let mut raw: Vec<f32> = (0..n)
            .map(|i| {
                let pulse = if i % period == 0 { 1.0 } else { 0.0 };
                let mut sum = 0.0;
                for (weight, f) in [1.0, 0.5, 0.25].iter().zip(formants.iter_mut()) {
                    sum += weight * f.run(pulse);
                }
                // Silben: viermal pro Sekunde lauter und leiser, aber nie ganz weg.
                let syllable = 0.55 + 0.45 * (2.0 * std::f32::consts::PI * 4.0 * i as f32 / RATE).sin();
                sum * syllable
            })
            .collect();
        let current = rms_db(&raw);
        let gain = db_to_gain(ziel_db - current);
        for x in &mut raw {
            *x *= gain;
        }
        raw
    }

    /// Der Trockenweg muss genau so lange warten wie das gefilterte Signal. Lag er davor,
    /// mischten sich beim Beimischen zwei versetzte Signale und die Stimme verlor rund 5 dB.
    #[test]
    fn trockenweg_liegt_auf_dem_gefilterten() {
        let frame = nnnoiseless::DenoiseState::FRAME_SIZE;
        // Impuls hinein, schauen wo er wieder herauskommt: einmal gefiltert, einmal trocken.
        let spitze = |dry_mix: f32| {
            let mut d = Denoiser::new();
            let out: Vec<f32> = (0..frame * 6).map(|i| d.process(if i == 0 { 1.0 } else { 0.0 }, dry_mix)).collect();
            out.iter()
                .enumerate()
                .fold((0, 0.0f32), |(bi, bv), (i, &v)| if v.abs() > bv { (i, v.abs()) } else { (bi, bv) })
                .0
        };
        assert_eq!(spitze(0.0), spitze(1.0), "gefiltert und trocken kommen zu verschiedenen Zeiten heraus");
    }

    /// Der Filter darf Rauschen wegnehmen, aber nicht die Stimme. Ohne diesen Test fiel
    /// nicht auf, dass „Leicht“ rund 5 dB der Stimme verschluckte.
    #[test]
    fn stimme_ueberlebt_den_filter() {
        // Werte aus settings::DenoiseLevel.
        for (stufe, dry, duck, vad) in [("Leicht", 0.32, 0.0, 0.6), ("Medium", 0.0, 40.0, 0.6), ("Stark", 0.0, 60.0, 0.85)] {
            let mut chain = Chain::new(RATE);
            let mut s = Settings::default();
            s.denoise = true;
            s.denoise_dry = dry;
            s.denoise_duck_db = duck;
            s.denoise_speech_vad = vad;
            chain.set(s);
            let input = vowel(RATE as usize * 3, -20.0);
            let out = run(&mut chain, &input);
            let verlust = tail_rms(&input) - tail_rms(&out);
            assert!(verlust < 1.5, "„{stufe}“ nimmt der Stimme {verlust:.1} dB weg");
        }
    }

    /// Auf „aus“ muss das Gate wirklich nichts tun – wer sich auf die Störfilter verlässt,
    /// stellt es ganz nach links und darf dann keinen Unterschied hören.
    /// Der Gain-Fader muss genau das tun, was draufsteht – am Ausgang und in der Anzeige.
    /// Mit VB-Cable rechnet die Lärmampel selbst; dann hängt nichts mehr an Windows.
    #[test]
    fn fader_kommt_am_ausgang_an() {
        for fader in [-20.0, 0.0, 6.0, 11.0] {
            let mut chain = Chain::new(RATE);
            let mut s = Settings::default();
            s.fader_db = fader;
            chain.set(s);
            let out = run(&mut chain, &sine(RATE as usize * 2, -30.0));
            let erwartet = -30.0 + fader;
            assert!((tail_rms(&out) - erwartet).abs() < 0.5, "Fader {fader:+.0}: Ausgang {:.1} statt {erwartet:.1} dB", tail_rms(&out));
            assert!((chain.out_level_db() - erwartet).abs() < 0.5, "Fader {fader:+.0}: Anzeige {:.1} statt {erwartet:.1} dB", chain.out_level_db());
        }
    }

    /// Die automatische Lautstärke regelt auf ihr Ziel, der Fader kommt danach: beides
    /// zusammen darf sich nicht gegenseitig aufheben.
    #[test]
    fn comp_frisst_den_fader_nicht() {
        let mit = |fader: f32| {
            let mut chain = Chain::new(RATE);
            let mut s = comp_settings();
            s.fader_db = fader;
            chain.set(s);
            tail_rms(&run(&mut chain, &sine(RATE as usize * 10, -30.0)))
        };
        let unterschied = mit(11.0) - mit(0.0);
        assert!((unterschied - 11.0).abs() < 0.5, "Fader bringt mit Comp. nur {unterschied:.1} dB");
    }

    /// Grob wie ein Räuspern: kurzer, rauer Ausbruch aus tiefem Rauschen, kaum harmonisch.
    fn raeuspern(n: usize, ziel_db: f32) -> Vec<f32> {
        let mut tief = Formant::new(180.0, 300.0);
        let mut mitte = Formant::new(550.0, 500.0);
        let roh = noise(n, 1.0);
        let mut raw: Vec<f32> = roh
            .iter()
            .enumerate()
            .map(|(i, &x)| {
                let t = i as f32 / RATE;
                // Zwei Stöße von je ~250 ms, dazwischen fast nichts.
                let stoss = (-((t % 0.6 - 0.12) / 0.09).powi(2)).exp();
                (tief.run(x) + 0.6 * mitte.run(x)) * stoss
            })
            .collect();
        let gain = db_to_gain(ziel_db - rms_db(&raw));
        for x in &mut raw {
            *x *= gain;
        }
        raw
    }

    /// Was der Filter *nicht* für Stimme hält, wirft er bei „Medium“ und „Stark“ heraus –
    /// „Leicht“ filtert nur. Damit ist gezeigt: die Mechanik greift, es hängt allein daran,
    /// wofür das Netz einen Laut hält.
    #[test]
    fn nicht_stimme_fliegt_raus() {
        let weg = |dry: f32, duck: f32, vad: f32| {
            let mut chain = Chain::new(RATE);
            let mut s = Settings::default();
            s.denoise = true;
            s.denoise_dry = dry;
            s.denoise_duck_db = duck;
            s.denoise_speech_vad = vad;
            chain.set(s);
            let input = raeuspern(RATE as usize * 3, -20.0);
            let out = run(&mut chain, &input);
            tail_rms(&input) - tail_rms(&out)
        };
        // Werte aus settings::DenoiseLevel.
        assert!(weg(0.32, 0.0, 0.6) < 11.0, "„Leicht“ nimmt zu viel weg");
        assert!(weg(0.0, 40.0, 0.6) > 35.0, "„Medium“ lässt Nicht-Stimme stehen");
        assert!(weg(0.0, 60.0, 0.85) > 50.0, "„Stark“ lässt Nicht-Stimme stehen");
    }

    #[test]
    fn gate_aus_laesst_alles_durch() {
        let input = vowel(RATE as usize, -20.0);
        let mut chain = Chain::new(RATE);
        chain.set(Settings::default());
        let out = run(&mut chain, &input);
        assert_eq!(out, input, "„aus“ verändert den Ton trotzdem");
    }

    /// Ein Wortanfang nach Stille darf höchstens ganz kurz gedämpft sein.
    #[test]
    fn gate_frisst_keinen_wortanfang() {
        let mut chain = Chain::new(RATE);
        chain.set(gate_settings());
        let ruhe = noise(RATE as usize, 0.0005);
        let wort = vowel(RATE as usize, -20.0);
        let mut input = ruhe.clone();
        input.extend_from_slice(&wort);
        let out = run(&mut chain, &input);

        let start = ruhe.len();
        let fehlt = |ms: usize| {
            let n = RATE as usize * ms / 1000;
            rms_db(&input[start..start + n]) - rms_db(&out[start..start + n])
        };
        assert!(fehlt(20) < 3.0, "nach 20 ms fehlen noch {:.1} dB", fehlt(20));
        assert!(fehlt(50) < 1.0, "nach 50 ms fehlen noch {:.1} dB", fehlt(50));
    }

    #[test]
    fn rauschfilter_entfernt_rauschen() {
        let mut chain = Chain::new(RATE);
        let mut s = Settings::default();
        s.denoise = true;
        chain.set(s);
        let input = noise(RATE as usize * 3, 0.05);
        let out = run(&mut chain, &input);
        let reduction = tail_rms(&input) - tail_rms(&out);
        assert!(reduction > 10.0, "nur {reduction:.1} dB leiser");
    }

    /// Ampel und Kanalzug müssen bei unbearbeitetem Ton denselben Pegel anzeigen.
    /// Ohne den gemeinsamen Sprachfilter stand der Kanalzug deutlich höher (Brummen zählte mit).
    #[test]
    fn kanalzug_misst_wie_die_ampel() {
        let mut chain = Chain::new(RATE);
        chain.set(Settings::default());
        let amplitude = db_to_gain(-20.0) * std::f32::consts::SQRT_2;
        let input: Vec<f32> = (0..RATE as usize)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / RATE).sin())
            .collect();
        let _ = run(&mut chain, &input);
        let measured = chain.out_level_db();
        assert!((measured + 20.0).abs() < 1.0, "Kanalzug misst {measured:.1} dB statt -20 dB");
    }

    #[test]
    fn rauschfilter_hat_drei_stufen() {
        let weggenommen = |dry: f32, duck_db: f32| {
            let mut chain = Chain::new(RATE);
            let mut s = Settings::default();
            s.denoise = true;
            s.denoise_dry = dry;
            s.denoise_duck_db = duck_db;
            chain.set(s);
            let input = noise(RATE as usize * 3, 0.05);
            let out = run(&mut chain, &input);
            tail_rms(&input) - tail_rms(&out)
        };
        // Werte aus settings::DenoiseLevel.
        let leicht = weggenommen(0.32, 0.0);
        let medium = weggenommen(0.0, 40.0);
        let stark = weggenommen(0.0, 60.0);
        assert!(leicht < medium && medium < stark, "Stufen: {leicht:.1} / {medium:.1} / {stark:.1} dB");
        assert!(leicht < 11.0, "„Leicht“ soll höchstens 10 dB wegnehmen, nimmt aber {leicht:.1} dB");
        // Medium und Stark senken Dauerkrach zusätzlich ab, sobald keine Stimme erkannt wird.
        assert!(medium > 35.0, "„Medium“ nimmt nur {medium:.1} dB weg");
        assert!(stark > 50.0, "„Stark“ nimmt nur {stark:.1} dB weg");
    }

    #[test]
    fn gate_senkt_leises_ab() {
        let mut chain = Chain::new(RATE);
        chain.set(gate_settings());
        let input = sine(RATE as usize, -55.0);
        let out = run(&mut chain, &input);
        let reduction = tail_rms(&input) - tail_rms(&out);
        assert!((reduction - 40.0).abs() < 1.0, "{reduction:.1} dB abgesenkt");
    }

    #[test]
    fn gate_senkt_gleitend_ab() {
        // Knapp unter der Schwelle nur wenig, weiter darunter mehr: je höher das Gate, desto leiser.
        let input = sine(RATE as usize, -48.0);
        let mut previous = f32::MAX;
        for threshold in [-50.0, -47.0, -45.0, -42.0] {
            let mut chain = Chain::new(RATE);
            let mut s = gate_settings();
            s.gate.threshold_db = threshold;
            chain.set(s);
            let out = tail_rms(&run(&mut chain, &input));
            assert!(out < previous || threshold == -50.0, "Schwelle {threshold}: {out:.1} dB, vorher {previous:.1}");
            previous = out;
        }
        assert!((previous - (-48.0 - 3.0 * 6.0)).abs() < 1.5, "bei -42 erwartet -66 dB, war {previous:.1}");
    }

    #[test]
    fn gate_laesst_sprache_durch() {
        let mut chain = Chain::new(RATE);
        chain.set(gate_settings());
        let input = sine(RATE as usize, -20.0);
        let out = run(&mut chain, &input);
        let difference = tail_rms(&input) - tail_rms(&out);
        assert!(difference.abs() < 0.1, "{difference:.2} dB Unterschied");
    }

    #[test]
    fn gate_haelt_nach_wortende() {
        let mut chain = Chain::new(RATE);
        chain.set(gate_settings());
        run(&mut chain, &sine(RATE as usize, -20.0));
        // 100 ms nach dem Wort: noch offen (Halten 200 ms). 500 ms danach: zu.
        run(&mut chain, &sine(RATE as usize / 10, -60.0));
        assert!(chain.gate_open(), "schon nach 100 ms zu");
        run(&mut chain, &sine(RATE as usize * 4 / 10, -60.0));
        assert!(!chain.gate_open(), "nach 500 ms noch offen");
    }

    #[test]
    fn ausgeschaltet_bleibt_signal_gleich() {
        let mut chain = Chain::new(RATE);
        let input = sine(RATE as usize / 2, -20.0);
        assert_eq!(run(&mut chain, &input), input);
    }

    #[test]
    fn limiter_haelt_obergrenze() {
        let mut chain = Chain::new(RATE);
        let mut s = comp_settings();
        s.fader_db = 12.0;
        chain.set(s);
        run(&mut chain, &sine(RATE as usize * 15, -45.0));
        // Plötzlicher Schrei bei voll aufgedrehter Verstärkung und Fader.
        let out = run(&mut chain, &sine(RATE as usize, -3.0));
        let peak = out.iter().fold(0.0f32, |m, y| m.max(y.abs()));
        assert!(peak <= db_to_gain(-1.0) + 1e-6, "Spitze {peak}");
    }

    #[test]
    fn fader_und_mute_wirken() {
        let mut chain = Chain::new(RATE);
        let mut s = Settings::default();
        s.fader_db = -6.0;
        chain.set(s);
        let input = sine(RATE as usize, -20.0);
        let out = run(&mut chain, &input);
        let difference = tail_rms(&input) - tail_rms(&out);
        assert!((difference - 6.0).abs() < 0.1, "{difference:.2} dB leiser");

        s.muted = true;
        chain.set(s);
        let muted = run(&mut chain, &input);
        assert!(tail_rms(&muted) < -100.0, "stumm ist {} dB", tail_rms(&muted));
    }

    #[test]
    fn stereo_bekommt_dieselbe_verstaerkung() {
        let mut chain = Chain::new(RATE);
        chain.set(gate_settings());
        let quiet = sine(RATE as usize, -55.0);
        let mut left = Vec::new();
        let mut right = Vec::new();
        for &x in &quiet {
            let mut frame = [x, x * 0.5];
            chain.process_frame(&mut frame);
            left.push(frame[0]);
            right.push(frame[1]);
        }
        let reduction_left = tail_rms(&quiet) - tail_rms(&left);
        let reduction_right = tail_rms(&quiet) - 6.02 - tail_rms(&right);
        assert!((reduction_left - reduction_right).abs() < 0.1, "links {reduction_left:.1}, rechts {reduction_right:.1}");
    }
}

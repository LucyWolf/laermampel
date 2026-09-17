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
const METER_WINDOW_MS: f32 = 50.0;

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

/// Die ganze Kette für ein Mikrofon mit beliebig vielen Kanälen.
pub struct Chain {
    settings: Settings,
    configured: bool,
    gate: Gate,
    comp: Comp,
    limiter: Limiter,
    ceiling: f32,

    fader_k: f32,
    fader_gain: f32,

    meter_k: f32,
    out_power: f32,
}

impl Chain {
    pub fn new(sample_rate: f32) -> Self {
        let mut chain = Self {
            settings: Settings::default(),
            configured: false,
            gate: Gate::new(sample_rate),
            comp: Comp::new(sample_rate),
            limiter: Limiter::new(sample_rate),
            ceiling: 1.0,
            fader_k: smoothing(FADER_SMOOTHING_MS, sample_rate),
            fader_gain: 1.0,
            meter_k: smoothing(METER_WINDOW_MS, sample_rate),
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
        let mono = frame.iter().sum::<f32>() / frame.len() as f32;
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

        let out = mono * gain;
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

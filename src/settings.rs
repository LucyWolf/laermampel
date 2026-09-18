use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::placement::Anchor;

/// Gate-Knopf ganz links: lässt alles durch.
pub const GATE_OFF_DB: f32 = -100.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DisplayMode {
    Dot,
    Bar,
}

/// Wie stark der Rauschfilter eingreifen darf.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DenoiseLevel {
    Leicht,
    Medium,
    Stark,
}

impl DenoiseLevel {
    pub const ALL: [DenoiseLevel; 3] = [DenoiseLevel::Leicht, DenoiseLevel::Medium, DenoiseLevel::Stark];

    pub fn label(self) -> &'static str {
        match self {
            DenoiseLevel::Leicht => "Leicht",
            DenoiseLevel::Medium => "Medium",
            DenoiseLevel::Stark => "Stark",
        }
    }

    /// Anteil des ungefilterten Tons, der stehen bleibt. Damit ist gedeckelt, wie viel
    /// der Filter beim Sprechen höchstens wegnehmen darf: 0,32 sind 10 dB, 0,1 sind 20 dB.
    pub fn dry(self) -> f32 {
        match self {
            DenoiseLevel::Leicht => 0.32,
            DenoiseLevel::Medium => 0.10,
            DenoiseLevel::Stark => 0.0,
        }
    }

    /// Wie viel in Sprechpausen zusätzlich abgesenkt wird. Dafür sagt das Netz selbst, ob es
    /// gerade Sprache hört – so verschwindet auch lauter Krach, den der Filter allein nicht
    /// schafft (Bohrmaschine, Akkuschrauber), ohne die Stimme anzutasten.
    pub fn duck_db(self) -> f32 {
        match self {
            DenoiseLevel::Leicht => 0.0,
            DenoiseLevel::Medium => 18.0,
            DenoiseLevel::Stark => 40.0,
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            DenoiseLevel::Leicht => "Filtert nur, senkt höchstens 10 dB ab. Klingt am natürlichsten, \
                                     lässt aber Geräusche stehen.",
            DenoiseLevel::Medium => "Filtert bis 20 dB und macht Pausen zusätzlich 18 dB leiser. \
                                     Guter Mittelweg für Tastatur, Lüfter und Werkzeug.",
            DenoiseLevel::Stark => "Filtert voll und macht Pausen praktisch still (40 dB). Auch lauter \
                                    Krach verschwindet zwischen den Wörtern; kann bei sehr leiser Stimme \
                                    den Anfang eines Wortes streifen.",
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub device_id: Option<String>,

    /// Schwellen in dBFS.
    pub yellow_db: f32,
    pub red_db: f32,

    pub attack_ms: f32,
    pub release_ms: f32,
    pub hold_ms: f32,

    /// 0.0 bis 1.0
    pub brightness: f32,
    pub green_brightness: f32,
    /// Deckkraft der Anzeige auf dem Bildschirm: 1.0 voll deckend, kleiner = durchsichtiger.
    pub opacity: f32,

    pub display: DisplayMode,
    /// 0 = Hauptbildschirm, danach von links nach rechts.
    pub monitor: usize,
    pub anchor: Anchor,
    /// Abstand zum Bildschirmrand in Pixeln (bei 100 % Skalierung).
    pub margin: f32,
    pub dot_size: f32,
    pub bar_width: f32,

    pub beep_enabled: bool,
    pub beep_volume: f32,

    /// Kanalzug fürs Mikrofon (über VB-Cable). Knöpfe 0 bis 10, 0 = aus.
    /// Schwelle des Gates in dB, -100 = aus.
    pub gate_threshold_db: f32,
    pub comp_knob: f32,
    pub fader_db: f32,
    pub mic_muted: bool,
    /// Wie viel leiser im geschlossenen Zustand, 80 = praktisch stumm.
    pub gate_range_db: f32,
    pub gate_attack_ms: f32,
    pub gate_hold_ms: f32,
    pub gate_release_ms: f32,
    pub agc_output_id: Option<String>,
    /// Ausgang ausgeschaltet: nichts wird auf ein virtuelles Gerät ausgegeben.
    pub output_off: bool,
    /// Rauschfilter (RNNoise) im Kanalzug.
    pub denoise: bool,
    /// Wie stark der Rauschfilter eingreift.
    pub denoise_level: DenoiseLevel,
    /// Puffer zwischen Aufnahme und Ausgabe in ms. Kleiner = weniger Verzögerung, mehr Aussetzer-Risiko.
    pub buffer_ms: f32,
    /// Gemessenes Rauschprofil (Pegel je Frequenzband in dB), damit es Neustarts übersteht.
    pub noise_profile: Option<Vec<f32>>,
    /// Ziel-Sprachpegel in dBFS (RMS).
    pub agc_target_db: f32,
    /// Wie schnell runtergeregelt wird.
    pub agc_attack_ms: f32,
    /// Wie schnell hochgeregelt wird.
    pub agc_release_ms: f32,
    /// Darunter gilt es als Pause, die Verstärkung wird gehalten.
    pub agc_gate_db: f32,
    pub agc_ceiling_db: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_id: None,
            yellow_db: -24.0,
            red_db: -18.0,
            attack_ms: 20.0,
            release_ms: 400.0,
            hold_ms: 1500.0,
            brightness: 0.9,
            green_brightness: 0.25,
            opacity: 1.0,
            display: DisplayMode::Dot,
            monitor: 0,
            anchor: Anchor::TopRight,
            margin: 12.0,
            dot_size: 20.0,
            bar_width: 240.0,
            beep_enabled: false,
            beep_volume: 0.3,
            gate_threshold_db: GATE_OFF_DB,
            comp_knob: 0.0,
            fader_db: 0.0,
            mic_muted: false,
            gate_range_db: 25.0,
            gate_attack_ms: 2.0,
            gate_hold_ms: 250.0,
            gate_release_ms: 150.0,
            agc_output_id: None,
            output_off: false,
            denoise: false,
            denoise_level: DenoiseLevel::Medium,
            buffer_ms: crate::agc::DEFAULT_BUFFER_MS,
            noise_profile: None,
            agc_target_db: -20.0,
            agc_attack_ms: 40.0,
            agc_release_ms: 1500.0,
            agc_gate_db: -50.0,
            agc_ceiling_db: 0.0,
        }
    }
}

/// Setzt jedes Feld zurück, das keine echte Zahl mehr ist.
macro_rules! nur_echte_zahlen {
    ($settings:ident, $default:ident, $($field:ident),* $(,)?) => {
        $(if !$settings.$field.is_finite() {
            $settings.$field = $default.$field;
        })*
    };
}

impl Settings {
    /// NaN oder unendlich schreibt serde_json als `null`. Beim nächsten Start scheitert dann
    /// das Einlesen der ganzen Datei – und alle Einstellungen wären weg. Also vorher gerade-
    /// biegen, sowohl beim Speichern als auch beim Laden.
    pub fn repair(&mut self) {
        let d = Settings::default();
        nur_echte_zahlen!(
            self,
            d,
            yellow_db,
            red_db,
            attack_ms,
            release_ms,
            hold_ms,
            brightness,
            green_brightness,
            opacity,
            margin,
            dot_size,
            bar_width,
            beep_volume,
            gate_threshold_db,
            comp_knob,
            fader_db,
            gate_range_db,
            gate_attack_ms,
            gate_hold_ms,
            gate_release_ms,
            buffer_ms,
            agc_target_db,
            agc_attack_ms,
            agc_release_ms,
            agc_gate_db,
            agc_ceiling_db,
        );
        if self.noise_profile.as_ref().is_some_and(|p| p.iter().any(|v| !v.is_finite())) {
            self.noise_profile = None;
        }
    }
}

fn path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("de", "LucyWolf", "Laermampel")?;
    Some(dirs.config_dir().join("settings.json"))
}

pub fn load() -> Settings {
    let mut settings: Settings = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    settings.repair();
    settings
}

pub fn save(settings: &Settings) {
    let mut settings = settings.clone();
    settings.repair();
    let settings = &settings;
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kaputte_zahlen_kosten_nicht_alle_einstellungen() {
        let mut settings = Settings { dot_size: f32::NAN, fader_db: f32::INFINITY, margin: 42.0, ..Settings::default() };
        settings.noise_profile = Some(vec![-80.0, f32::NAN]);

        settings.repair();

        assert_eq!(settings.dot_size, Settings::default().dot_size);
        assert_eq!(settings.fader_db, Settings::default().fader_db);
        assert_eq!(settings.margin, 42.0, "gute Werte bleiben stehen");
        assert!(settings.noise_profile.is_none(), "kaputtes Rauschprofil wird verworfen");

        // Und die Datei lässt sich danach wieder einlesen.
        let json = serde_json::to_string(&settings).unwrap();
        let wieder: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(wieder.margin, 42.0);
    }
}

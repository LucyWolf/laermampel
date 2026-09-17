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
            gate_range_db: 40.0,
            gate_attack_ms: 2.0,
            gate_hold_ms: 250.0,
            gate_release_ms: 150.0,
            agc_output_id: None,
            output_off: false,
            denoise: false,
            agc_target_db: -20.0,
            agc_attack_ms: 40.0,
            agc_release_ms: 1500.0,
            agc_gate_db: -50.0,
            agc_ceiling_db: 0.0,
        }
    }
}

fn path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("de", "LucyWolf", "Laermampel")?;
    Some(dirs.config_dir().join("settings.json"))
}

pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(settings: &Settings) {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = std::fs::write(path, json);
    }
}

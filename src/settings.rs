use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::placement::Anchor;

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
    /// Abstand zur eingelernten normalen Stimme.
    pub yellow_offset_db: f32,
    pub red_offset_db: f32,

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

    /// Mikrofon für andere Programme über VB-Cable.
    pub noise_filter_enabled: bool,
    pub agc_enabled: bool,
    pub agc_output_id: Option<String>,
    /// Ziel-Sprachpegel in dBFS (RMS).
    pub agc_target_db: f32,
    pub agc_max_gain_db: f32,
    pub agc_max_cut_db: f32,
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
            yellow_offset_db: 6.0,
            red_offset_db: 10.0,
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
            noise_filter_enabled: false,
            agc_enabled: false,
            agc_output_id: None,
            agc_target_db: -20.0,
            agc_max_gain_db: 18.0,
            agc_max_cut_db: 18.0,
            agc_attack_ms: 40.0,
            agc_release_ms: 1500.0,
            agc_gate_db: -50.0,
            agc_ceiling_db: -1.0,
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

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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

    pub always_on_top: bool,
    pub beep_enabled: bool,
    pub beep_volume: f32,

    pub window_pos: Option<[f32; 2]>,
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
            always_on_top: true,
            beep_enabled: false,
            beep_volume: 0.3,
            window_pos: None,
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

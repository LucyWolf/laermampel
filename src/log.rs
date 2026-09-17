//! Einfache Log-Datei, damit sich Probleme auf fremden Rechnern nachvollziehen lassen.
//! Liegt unter %LOCALAPPDATA%\LucyWolf\Laermampel\data\laermampel.log.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Wird die Datei größer, fängt sie beim nächsten Start neu an.
const MAX_BYTES: u64 = 512 * 1024;

static FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("de", "LucyWolf", "Laermampel")?;
    Some(dirs.data_local_dir().join("laermampel.log"))
}

pub fn init() {
    let Some(path) = path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let too_big = std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false);
    let file = OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(&path);
    if let (Ok(file), Ok(mut slot)) = (file, FILE.lock()) {
        *slot = Some(file);
    }

    // Abstürze mit Ort und Meldung festhalten, sonst verschwindet das Programm einfach.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&format!("ABSTURZ: {info}"));
        previous(info);
    }));
}

pub fn write(message: &str) {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // UTC-Uhrzeit reicht zum Zuordnen.
    let (h, m, s) = ((seconds / 3600) % 24, (seconds / 60) % 60, seconds % 60);
    if let Ok(mut slot) = FILE.lock()
        && let Some(file) = slot.as_mut()
    {
        let _ = writeln!(file, "{h:02}:{m:02}:{s:02} UTC  {message}");
        let _ = file.flush();
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::log::write(&format!($($arg)*))
    };
}

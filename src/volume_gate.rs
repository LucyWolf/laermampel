//! Gate über den Mikrofon-Regler von Windows: ohne Filter, ohne VB-Cable, ohne Adminrechte.
//!
//! Unter der Schwelle wird der Windows-Regler des Mikrofons heruntergezogen, das gilt für alle
//! Programme. Ganz stumm geht nicht, sonst hört die Lärmampel selbst nicht mehr, wann man wieder
//! spricht. Die Absenkung wird bei der Messung wieder herausgerechnet.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// So weit muss der Pegel unter die Schwelle fallen, bevor das Gate zu zählen beginnt.
const HYSTERESIS_DB: f32 = 3.0;
/// So oft wird nachgesehen, ob jemand den Windows-Regler von Hand verstellt hat.
const RESYNC_INTERVAL: Duration = Duration::from_millis(500);
/// Nach dem Zugehen erst kurz warten, dann messen, wie stark der Regler wirklich absenkt.
const CALIBRATION_SETTLE: Duration = Duration::from_millis(80);
const CALIBRATION_END: Duration = Duration::from_millis(350);
/// Senkt das Gerät mehr ab als gewollt, wird der Regler um so viel weniger weit heruntergezogen.
const MAX_BACKOFF_DB: f32 = 60.0;
/// Glättung des Pegels, aus dem der Wert kurz vor dem Zugehen stammt.
const SMOOTHING: f32 = 0.2;

pub struct GateParams {
    pub threshold_db: f32,
    pub range_db: f32,
    pub hold_ms: f32,
}

/// Merkt sich die ursprüngliche Lautstärke auf der Platte, falls die Lärmampel abstürzt,
/// während das Gate zu ist. Beim nächsten Start wird sie dann zurückgesetzt.
#[derive(Serialize, Deserialize)]
struct Leftover {
    endpoint_id: String,
    original_db: f32,
}

fn leftover_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("de", "LucyWolf", "Laermampel")?;
    Some(dirs.data_local_dir().join("gate_lautstaerke.json"))
}

pub struct VolumeGate {
    endpoint_id: Option<String>,
    #[cfg(windows)]
    volume: Option<win::Volume>,
    open: bool,
    below_since: Option<Instant>,
    /// Stand des Windows-Reglers, bevor das Gate ihn angefasst hat.
    original_db: f32,
    /// Was das Gate zuletzt eingestellt hat.
    applied_db: f32,
    last_resync: Instant,

    /// Geglätteter echter Pegel, solange offen.
    smoothed_db: Option<f32>,
    /// Pegel kurz vor dem Zugehen; daran wird die tatsächliche Absenkung gemessen.
    before_close_db: f32,
    closed_at: Option<Instant>,
    calibration_sum: f32,
    calibration_count: u32,
    /// Gemessene Absenkung. Viele Headsets senken deutlich stärker ab, als der Regler in dB angibt.
    measured_attenuation_db: Option<f32>,
    /// Um so viel wird der Regler weniger weit heruntergezogen, damit die Lärmampel noch hört.
    backoff_db: f32,
}

impl VolumeGate {
    pub fn new() -> Self {
        let gate = Self {
            endpoint_id: None,
            #[cfg(windows)]
            volume: None,
            open: true,
            below_since: None,
            original_db: 0.0,
            applied_db: 0.0,
            last_resync: Instant::now(),
            smoothed_db: None,
            before_close_db: -120.0,
            closed_at: None,
            calibration_sum: 0.0,
            calibration_count: 0,
            measured_attenuation_db: None,
            backoff_db: 0.0,
        };
        gate.restore_leftover();
        gate
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Wie viel leiser das Mikrofon gerade bei der Lärmampel ankommt als ohne Gate.
    pub fn attenuation_db(&self) -> f32 {
        if !self.has_volume() || (self.original_db - self.applied_db).abs() < 0.05 {
            return 0.0;
        }
        self.measured_attenuation_db.unwrap_or(self.original_db - self.applied_db).max(0.0)
    }

    fn has_volume(&self) -> bool {
        #[cfg(windows)]
        return self.volume.is_some();
        #[cfg(not(windows))]
        false
    }

    /// Einmal pro Bild. `device_id` ist die cpal-ID, `measured_db` der gerade gemessene Pegel
    /// (mit abgesenktem Regler). Gibt den echten Pegel zurück, also mit herausgerechneter Absenkung.
    pub fn update(&mut self, device_id: Option<&str>, enabled: bool, measured_db: Option<f32>, params: &GateParams) -> Option<f32> {
        let real_db = measured_db.map(|db| db + self.attenuation_db());

        let endpoint_id = device_id.and_then(|id| id.strip_prefix("wasapi:")).map(str::to_string);
        if !enabled || endpoint_id != self.endpoint_id {
            self.release();
            self.endpoint_id = endpoint_id;
        }
        if !enabled {
            return real_db;
        }
        if !self.has_volume() && !self.attach() {
            return real_db;
        }

        // Hat jemand den Regler von Hand verstellt, solange das Gate offen war: neuer Ausgangswert.
        if self.open && self.last_resync.elapsed() >= RESYNC_INTERVAL {
            self.last_resync = Instant::now();
            if let Some(current) = self.read_db()
                && (current - self.applied_db).abs() > 0.5
            {
                self.original_db = current;
                self.applied_db = current;
            }
        }

        if self.open
            && let Some(level) = real_db
        {
            let previous = self.smoothed_db.unwrap_or(level);
            self.smoothed_db = Some(previous + (level - previous) * SMOOTHING);
        }
        if !self.open {
            self.calibrate(measured_db, params.range_db);
        }

        if let Some(level) = real_db {
            if level > params.threshold_db {
                if !self.open {
                    self.closed_at = None;
                }
                self.open = true;
                self.below_since = None;
            } else if level < params.threshold_db - HYSTERESIS_DB {
                let since = *self.below_since.get_or_insert_with(Instant::now);
                if self.open && since.elapsed() >= Duration::from_secs_f32(params.hold_ms.max(0.0) / 1000.0) {
                    self.open = false;
                    self.start_calibration();
                }
            }
        }

        let target = if self.open { self.original_db } else { self.closed_db(params.range_db) };
        if (target - self.applied_db).abs() > 0.05 {
            self.write_db(target);
            self.applied_db = target;
            self.remember_leftover(!self.open);
        }
        real_db
    }

    fn start_calibration(&mut self) {
        self.before_close_db = self.smoothed_db.unwrap_or(-120.0);
        self.closed_at = Some(Instant::now());
        self.calibration_sum = 0.0;
        self.calibration_count = 0;
    }

    /// Kurz nach dem Zugehen: wie viel leiser kommt das Mikrofon wirklich an?
    /// Ist es viel mehr als gewollt, den Regler weniger weit herunterziehen und neu messen.
    fn calibrate(&mut self, measured_db: Option<f32>, range_db: f32) {
        let Some(closed_at) = self.closed_at else { return };
        let elapsed = closed_at.elapsed();
        if elapsed < CALIBRATION_SETTLE {
            return;
        }
        if elapsed < CALIBRATION_END {
            if let Some(db) = measured_db {
                self.calibration_sum += db;
                self.calibration_count += 1;
            }
            return;
        }
        self.closed_at = None;
        if self.calibration_count == 0 {
            return;
        }
        let after_db = self.calibration_sum / self.calibration_count as f32;
        let attenuation = (self.before_close_db - after_db).clamp(0.0, 90.0);
        self.measured_attenuation_db = Some(attenuation);

        let wanted = range_db.max(0.0);
        if attenuation > wanted + 6.0 && self.backoff_db < MAX_BACKOFF_DB {
            self.backoff_db = (self.backoff_db + (attenuation - wanted) * 0.7).min(MAX_BACKOFF_DB);
            log!("Gate: Gerät senkt {attenuation:.0} dB statt {wanted:.0} dB ab, Regler {:.0} dB weniger weit runter", self.backoff_db);
            // Mit dem neuen Wert gleich noch einmal messen.
            self.closed_at = Some(Instant::now());
            self.calibration_sum = 0.0;
            self.calibration_count = 0;
            self.measured_attenuation_db = None;
        }
    }

    /// Regler zurück auf den ursprünglichen Wert und loslassen.
    pub fn release(&mut self) {
        // Nichts angefasst: nichts zu tun (wird sonst jedes Bild aufgerufen, solange das Gate aus ist).
        if !self.has_volume() {
            return;
        }
        if (self.applied_db - self.original_db).abs() > 0.05 {
            let original = self.original_db;
            self.write_db(original);
        }
        #[cfg(windows)]
        {
            self.volume = None;
        }
        self.open = true;
        self.below_since = None;
        self.remember_leftover(false);
    }

    fn remember_leftover(&self, closed: bool) {
        let Some(path) = leftover_path() else { return };
        match (&self.endpoint_id, closed) {
            (Some(id), true) => {
                let leftover = Leftover { endpoint_id: id.clone(), original_db: self.original_db };
                if let Ok(json) = serde_json::to_string(&leftover) {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(path, json);
                }
            }
            _ => {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    fn restore_leftover(&self) {
        let Some(path) = leftover_path() else { return };
        let Some(leftover) = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Leftover>(&s).ok()) else {
            return;
        };
        #[cfg(windows)]
        if let Some(volume) = win::Volume::open(&leftover.endpoint_id) {
            volume.set_db(leftover.original_db);
            log!("Gate: Mikrofon-Lautstärke nach Absturz auf {:.1} dB zurückgesetzt", leftover.original_db);
        }
        #[cfg(not(windows))]
        let _ = leftover;
        let _ = std::fs::remove_file(path);
    }

    #[cfg(windows)]
    fn attach(&mut self) -> bool {
        let Some(id) = &self.endpoint_id else { return false };
        let Some(volume) = win::Volume::open(id) else { return false };
        let Some(current) = volume.get_db() else { return false };
        self.original_db = current;
        self.applied_db = current;
        self.volume = Some(volume);
        self.open = true;
        self.backoff_db = 0.0;
        self.measured_attenuation_db = None;
        log!("Gate: nutzt den Windows-Regler von {id}, steht auf {current:.1} dB");
        true
    }

    #[cfg(not(windows))]
    fn attach(&mut self) -> bool {
        false
    }

    fn closed_db(&self, range_db: f32) -> f32 {
        #[cfg(windows)]
        if let Some(volume) = &self.volume {
            return (self.original_db - range_db.max(0.0) + self.backoff_db).min(self.original_db).max(volume.min_db);
        }
        self.original_db - range_db.max(0.0) + self.backoff_db
    }

    fn read_db(&self) -> Option<f32> {
        #[cfg(windows)]
        return self.volume.as_ref().and_then(win::Volume::get_db);
        #[cfg(not(windows))]
        None
    }

    fn write_db(&self, db: f32) {
        #[cfg(windows)]
        if let Some(volume) = &self.volume {
            volume.set_db(db);
        }
        #[cfg(not(windows))]
        let _ = db;
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
    use windows::core::HSTRING;

    pub struct Volume {
        endpoint: IAudioEndpointVolume,
        pub min_db: f32,
    }

    impl Volume {
        pub fn open(endpoint_id: &str) -> Option<Volume> {
            unsafe {
                // Schon initialisiert ist auch recht; der Rückgabewert ist dann nur ein Hinweis.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                let device = enumerator.GetDevice(&HSTRING::from(endpoint_id)).ok()?;
                let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
                let (mut min_db, mut max_db, mut step) = (0f32, 0f32, 0f32);
                endpoint.GetVolumeRange(&mut min_db, &mut max_db, &mut step).ok()?;
                Some(Volume { endpoint, min_db })
            }
        }

        pub fn get_db(&self) -> Option<f32> {
            unsafe { self.endpoint.GetMasterVolumeLevel().ok() }
        }

        pub fn set_db(&self, db: f32) {
            unsafe {
                let _ = self.endpoint.SetMasterVolumeLevel(db, std::ptr::null());
            }
        }
    }
}

//! Gate, Mute und Fader über den Mikrofon-Regler von Windows: ohne VB-Cable, ohne Adminrechte.
//!
//! Das Gate arbeitet gleitend (wie in `dsp`): unter der Schwelle zieht es den Windows-Regler umso
//! weiter herunter, je leiser es ist. Weil dann auch die Lärmampel leiser hört, wird die Absenkung
//! aus der Messung wieder herausgerechnet. Wie stark das Gerät wirklich auf den Regler reagiert,
//! misst ein kurzer Test in Sprechpausen; viele Headsets senken deutlich stärker ab als angegeben.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dsp::{GATE_OPEN_BELOW_DB, gate_reduction_db};

/// So oft wird nachgesehen, ob jemand den Windows-Regler von Hand verstellt hat.
const RESYNC_INTERVAL: Duration = Duration::from_millis(500);
/// Glättung des Pegels, auf den das Gate reagiert, damit Rauschspitzen es nicht aufreißen.
const LEVEL_SMOOTHING: f32 = 0.3;
/// Kleinere Änderungen am Regler lassen wir weg, sonst wird er ständig verstellt.
const MIN_STEP_DB: f32 = 0.5;

/// Test, wie stark das Gerät reagiert: Regler kurz um so viel herunter, Pegel vorher/nachher vergleichen.
const TEST_DIP_DB: f32 = 10.0;
const TEST_SETTLE: Duration = Duration::from_millis(60);
const TEST_LENGTH: Duration = Duration::from_millis(260);
/// Unter diesem gemessenen Pegel hört die Lärmampel praktisch nichts mehr; dann wird die
/// Absenkung zurückgenommen, damit sie merkt, wenn wieder gesprochen wird.
const MEASURE_FLOOR_DB: f32 = -85.0;
/// So viel Luft über der Messgrenze wird angestrebt, bevor wieder tiefer abgesenkt wird.
const MEASURE_HEADROOM_DB: f32 = 10.0;
/// Wie schnell die Absenkungsgrenze zurückgenommen bzw. wieder vergrößert wird (dB pro Sekunde).
const CAP_DOWN_DB_PER_SECOND: f32 = 40.0;
const CAP_UP_DB_PER_SECOND: f32 = 6.0;

/// Steigt der gemessene Pegel bei geschlossenem Gate um so viel, geht es sofort auf.
/// Das funktioniert ohne Schätzung, wie stark der Regler absenkt.
const OPEN_JUMP_DB: f32 = 6.0;
/// So lange nach dem Zugehen wird der Ruhepegel gemerkt.
const BASELINE_AFTER: Duration = Duration::from_millis(250);
/// Ist das Gate so lange zu, wird der Regler kurz geöffnet, um den echten Pegel zu messen.
const RECHECK_AFTER: Duration = Duration::from_secs(4);
const RECHECK_LENGTH: Duration = Duration::from_millis(150);
/// Bis zum ersten Test wird der Regler höchstens so weit gezogen, falls das Gerät viel stärker reagiert.
const UNTESTED_MAX_DB: f32 = 10.0;
/// Nur in ruhigen Momenten testen, und nicht zu oft.
const TEST_INTERVAL: Duration = Duration::from_secs(30);
const TEST_STEADY_DB: f32 = 2.0;
const TEST_STEADY_FOR: Duration = Duration::from_millis(400);

pub struct GateParams {
    pub threshold_db: f32,
    pub range_db: f32,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
}

/// Merkt sich die ursprüngliche Einstellung auf der Platte, falls die Lärmampel abstürzt,
/// während der Regler heruntergezogen oder stumm ist. Beim nächsten Start wird sie zurückgesetzt.
#[derive(Serialize, Deserialize)]
struct Leftover {
    endpoint_id: String,
    original_db: f32,
    #[serde(default)]
    muted: bool,
}

fn leftover_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("de", "LucyWolf", "Laermampel")?;
    Some(dirs.data_local_dir().join("gate_lautstaerke.json"))
}

enum Test {
    Idle,
    /// Regler ist für den Test um `TEST_DIP_DB` tiefer als `restore_db`. Gemessen wird ungekorrigiert.
    Running { started: Instant, restore_db: f32, before_raw_db: f32, sum: f32, count: u32 },
}

pub struct VolumeGate {
    endpoint_id: Option<String>,
    #[cfg(windows)]
    volume: Option<win::Volume>,
    muted: bool,
    /// Stand des Windows-Reglers, bevor die Lärmampel ihn angefasst hat.
    original_db: f32,
    /// Ausgangspunkt inklusive Fader; von hier senkt das Gate ab.
    base_db: f32,
    /// Was die Lärmampel zuletzt eingestellt hat.
    applied_db: f32,
    last_resync: Instant,
    last_tick: Instant,

    /// Wie viele dB das Gerät pro dB am Regler wirklich absenkt (gemessen).
    response: f32,
    smoothed_db: Option<f32>,
    /// Geglätteter gemessener Pegel ohne Korrektur, für den Test.
    smoothed_raw_db: Option<f32>,
    /// Aktuelle Absenkung durch das Gate in dB (wie sie bei anderen ankommt).
    reduction_db: f32,
    last_above: Instant,

    test: Test,
    last_test: Option<Instant>,
    steady_since: Instant,

    /// Seit wann das Gate zu ist, und der gemessene Ruhepegel danach.
    closed_since: Option<Instant>,
    closed_baseline_db: Option<f32>,
    /// Läuft gerade das kurze Nachsehen mit offenem Regler?
    recheck_until: Option<Instant>,
    /// So weit darf höchstens abgesenkt werden, damit die Messung noch etwas hört.
    reduction_cap_db: f32,
}

impl VolumeGate {
    pub fn new() -> Self {
        let gate = Self {
            endpoint_id: None,
            #[cfg(windows)]
            volume: None,
            muted: false,
            original_db: 0.0,
            base_db: 0.0,
            applied_db: 0.0,
            last_resync: Instant::now(),
            last_tick: Instant::now(),
            response: 1.0,
            smoothed_db: None,
            smoothed_raw_db: None,
            reduction_db: 0.0,
            last_above: Instant::now(),
            test: Test::Idle,
            last_test: None,
            steady_since: Instant::now(),
            closed_since: None,
            closed_baseline_db: None,
            recheck_until: None,
            reduction_cap_db: 0.0,
        };
        gate.restore_leftover();
        gate
    }

    pub fn is_open(&self) -> bool {
        self.reduction_db < GATE_OPEN_BELOW_DB
    }

    /// Wie viel leiser andere dich gerade hören.
    pub fn reduction_db(&self) -> f32 {
        self.reduction_db
    }

    /// Wie viel leiser die Lärmampel das Mikrofon gerade hört, weil der Regler unten steht.
    fn heard_attenuation_db(&self) -> f32 {
        // Nur die Absenkung durchs Gate herausrechnen; den Fader soll man im Pegel sehen.
        if self.has_volume() { ((self.base_db - self.applied_db) * self.response).max(0.0) } else { 0.0 }
    }

    fn has_volume(&self) -> bool {
        #[cfg(windows)]
        return self.volume.is_some();
        #[cfg(not(windows))]
        false
    }

    /// Einmal pro Bild. `device_id` ist die cpal-ID, `measured_db` der gerade gemessene Pegel.
    /// Gibt den echten Pegel zurück, also mit herausgerechneter Absenkung.
    pub fn update(
        &mut self,
        device_id: Option<&str>,
        gate: bool,
        mute: bool,
        fader_db: f32,
        measured_db: Option<f32>,
        params: &GateParams,
    ) -> Option<f32> {
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.2);
        self.last_tick = Instant::now();
        let real_db = measured_db.map(|db| db + self.heard_attenuation_db());

        let endpoint_id = device_id.and_then(|id| id.strip_prefix("wasapi:")).map(str::to_string);
        if endpoint_id != self.endpoint_id {
            self.release();
            self.endpoint_id = endpoint_id;
        }
        let uses_fader = fader_db.abs() > 0.05;
        if !gate && !mute && !uses_fader {
            self.release();
            return real_db;
        }
        if !self.has_volume() && !self.attach() {
            return real_db;
        }
        // Fader verschiebt den Ausgangspunkt, das Gate senkt von dort weiter ab.
        self.base_db = self.clamp_db(self.original_db + fader_db);

        // Mute: Windows schaltet das Mikrofon für alle Programme stumm, auch für die Lärmampel.
        if mute != self.muted {
            self.set_mute(mute);
            self.muted = mute;
            self.remember_leftover();
        }

        if !gate && self.reduction_db > 0.0 {
            self.reduction_db = 0.0;
            self.apply_reduction();
        }

        // Von Hand verstellter Regler, solange die Lärmampel nichts verschiebt: neuer Ausgangswert.
        if self.reduction_db < MIN_STEP_DB
            && !uses_fader
            && matches!(self.test, Test::Idle)
            && self.last_resync.elapsed() >= RESYNC_INTERVAL
        {
            self.last_resync = Instant::now();
            if let Some(current) = self.read_db()
                && (current - self.applied_db).abs() > MIN_STEP_DB
            {
                self.original_db = current;
                self.applied_db = current;
            }
        }

        if !gate {
            self.reduction_db = 0.0;
            self.apply_reduction();
            return real_db;
        }

        if let (Some(level), Some(raw)) = (real_db, measured_db) {
            let previous = self.smoothed_db.unwrap_or(level);
            let smoothed = previous + (level - previous) * LEVEL_SMOOTHING;
            self.smoothed_db = Some(smoothed);
            let previous_raw = self.smoothed_raw_db.unwrap_or(raw);
            let smoothed_raw = previous_raw + (raw - previous_raw) * LEVEL_SMOOTHING;
            self.smoothed_raw_db = Some(smoothed_raw);
            if (raw - smoothed_raw).abs() > TEST_STEADY_DB {
                self.steady_since = Instant::now();
            }
            if self.run_test(raw) {
                // Während des Tests keine Entscheidungen, der Regler steht absichtlich tiefer.
                return real_db;
            }

            // Notbremsen gegen ein festhängendes Gate: Sprung im gemessenen Pegel und
            // regelmäßiges Nachsehen mit offenem Regler.
            if let Some(until) = self.recheck_until {
                if Instant::now() < until {
                    return real_db;
                }
                self.recheck_until = None;
                self.closed_since = None;
                self.closed_baseline_db = None;
            } else if self.reduction_db >= MIN_STEP_DB {
                let since = *self.closed_since.get_or_insert_with(Instant::now);
                if self.closed_baseline_db.is_none() && since.elapsed() >= BASELINE_AFTER {
                    self.closed_baseline_db = Some(smoothed_raw);
                }
                if let Some(baseline) = self.closed_baseline_db
                    && smoothed_raw > baseline + OPEN_JUMP_DB
                {
                    log!("Gate: Pegel um {:.0} dB gestiegen, geht auf", smoothed_raw - baseline);
                    self.reduction_db = 0.0;
                    self.closed_since = None;
                    self.closed_baseline_db = None;
                    self.smoothed_db = None;
                    self.apply_reduction();
                    return real_db;
                }
                if since.elapsed() >= RECHECK_AFTER {
                    log!("Gate: sieht kurz mit offenem Regler nach, wie laut es wirklich ist");
                    self.reduction_db = 0.0;
                    self.smoothed_db = None;
                    self.apply_reduction();
                    self.recheck_until = Some(Instant::now() + RECHECK_LENGTH);
                    return real_db;
                }
            } else {
                self.closed_since = None;
                self.closed_baseline_db = None;
            }

            // Absenkungsgrenze nachregeln: nur so weit runter, dass die Messung noch etwas hört.
            let range = params.range_db.max(0.0);
            if smoothed_raw < MEASURE_FLOOR_DB {
                let before = self.reduction_cap_db;
                self.reduction_cap_db = (self.reduction_db - CAP_DOWN_DB_PER_SECOND * dt).max(0.0);
                if before - self.reduction_cap_db > 3.0 {
                    log!("Gate: senkt nur noch {:.0} dB ab, sonst hört die Lärmampel nichts mehr", self.reduction_cap_db);
                }
            } else if smoothed_raw > MEASURE_FLOOR_DB + MEASURE_HEADROOM_DB {
                self.reduction_cap_db = (self.reduction_cap_db + CAP_UP_DB_PER_SECOND * dt).min(range);
            }
            self.reduction_cap_db = self.reduction_cap_db.min(range);

            let mut target = gate_reduction_db(smoothed, params.threshold_db, range).min(self.reduction_cap_db);
            if smoothed >= params.threshold_db {
                self.last_above = Instant::now();
            } else if self.last_above.elapsed() < Duration::from_secs_f32(params.hold_ms.max(0.0) / 1000.0) {
                target = target.min(self.reduction_db);
            }
            let ms = if target < self.reduction_db { params.attack_ms } else { params.release_ms };
            let k = if ms <= 0.0 { 1.0 } else { 1.0 - (-dt * 1000.0 / ms).exp() };
            self.reduction_db += (target - self.reduction_db) * k;
        }
        self.apply_reduction();
        self.maybe_start_test();
        real_db
    }

    /// Regler auf Ausgangspunkt minus Absenkung durchs Gate.
    fn apply_reduction(&mut self) {
        let mut asked = self.reduction_db / self.response.max(0.1);
        if self.last_test.is_none() {
            asked = asked.min(UNTESTED_MAX_DB);
        }
        let wanted = self.base_db - asked;
        let target = self.clamp_db(wanted);
        let back_to_start = self.reduction_db < MIN_STEP_DB && target != self.applied_db;
        if (target - self.applied_db).abs() >= MIN_STEP_DB || back_to_start {
            self.write_db(target);
            self.applied_db = target;
            self.remember_leftover();
        }
    }

    /// In einem ruhigen Moment (Pegel gleichmäßig, egal ob Gate offen oder zu) den Regler kurz
    /// tiefer stellen und messen, wie viel leiser es wirklich wird.
    fn maybe_start_test(&mut self) {
        let due = self.last_test.is_none_or(|t| t.elapsed() >= TEST_INTERVAL);
        let steady = self.steady_since.elapsed() >= TEST_STEADY_FOR;
        if !due || !steady || !matches!(self.test, Test::Idle) {
            return;
        }
        let Some(before_raw_db) = self.smoothed_raw_db else { return };
        // Zu leise: der Unterschied ginge im Grundrauschen der Messung unter.
        if before_raw_db < -85.0 {
            return;
        }
        let restore_db = self.applied_db;
        let dip = self.clamp_db_min(restore_db - TEST_DIP_DB);
        if restore_db - dip < 3.0 {
            return;
        }
        self.write_db(dip);
        self.applied_db = dip;
        self.remember_leftover();
        self.test = Test::Running { started: Instant::now(), restore_db, before_raw_db, sum: 0.0, count: 0 };
    }

    /// Gibt `true` zurück, solange ein Test läuft.
    fn run_test(&mut self, raw_db: f32) -> bool {
        let dip = match &self.test {
            Test::Running { restore_db, .. } => restore_db - self.applied_db,
            Test::Idle => return false,
        };
        let Test::Running { started, restore_db, before_raw_db, sum, count } = &mut self.test else { return false };
        let elapsed = started.elapsed();
        if elapsed < TEST_SETTLE {
            return true;
        }
        if elapsed < TEST_LENGTH {
            *sum += raw_db;
            *count += 1;
            return true;
        }
        let (restore, before, total, n) = (*restore_db, *before_raw_db, *sum, *count);
        self.test = Test::Idle;
        self.last_test = Some(Instant::now());
        self.write_db(restore);
        self.applied_db = restore;
        self.remember_leftover();
        if n > 0 && dip > 0.0 {
            let dropped = before - total / n as f32;
            let response = (dropped / dip).clamp(0.3, 6.0);
            log!("Gate: Regler {dip:.0} dB tiefer → {dropped:.1} dB leiser (Faktor {response:.2})");
            self.response = response;
        }
        false
    }

    /// Auf den Bereich begrenzen, den das Gerät hergibt.
    fn clamp_db(&self, db: f32) -> f32 {
        #[cfg(windows)]
        if let Some(volume) = &self.volume {
            return db.clamp(volume.min_db, volume.max_db);
        }
        db
    }

    /// Wie `clamp_db`, aber nie über den Ausgangspunkt (für den Test).
    fn clamp_db_min(&self, db: f32) -> f32 {
        self.clamp_db(db).min(self.base_db)
    }

    /// Regler und Stummschaltung zurück auf den ursprünglichen Stand und loslassen.
    pub fn release(&mut self) {
        // Nichts angefasst: nichts zu tun (wird sonst jedes Bild aufgerufen, solange alles aus ist).
        if !self.has_volume() {
            return;
        }
        if (self.applied_db - self.original_db).abs() > 0.05 {
            let original = self.original_db;
            self.write_db(original);
            self.applied_db = original;
        }
        if self.muted {
            self.set_mute(false);
            self.muted = false;
        }
        #[cfg(windows)]
        {
            self.volume = None;
        }
        self.reduction_db = 0.0;
        self.reduction_cap_db = 0.0;
        self.smoothed_db = None;
        self.smoothed_raw_db = None;
        self.test = Test::Idle;
        self.closed_since = None;
        self.closed_baseline_db = None;
        self.recheck_until = None;
        self.remember_leftover();
    }

    fn remember_leftover(&self) {
        let Some(path) = leftover_path() else { return };
        let touched = self.has_volume() && (self.muted || (self.applied_db - self.original_db).abs() > 0.05);
        match (&self.endpoint_id, touched) {
            (Some(id), true) => {
                let leftover = Leftover { endpoint_id: id.clone(), original_db: self.original_db, muted: self.muted };
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
            if leftover.muted {
                volume.set_mute(false);
            }
            log!("Gate: Mikrofon nach Absturz zurückgesetzt ({:.1} dB, Stummschaltung aufgehoben)", leftover.original_db);
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
        self.base_db = current;
        self.reduction_cap_db = 0.0;
        self.applied_db = current;
        self.volume = Some(volume);
        self.reduction_db = 0.0;
        self.response = 1.0;
        self.last_test = None;
        log!("Gate: nutzt den Windows-Regler von {id}, steht auf {current:.1} dB");
        true
    }

    #[cfg(not(windows))]
    fn attach(&mut self) -> bool {
        false
    }

    fn read_db(&self) -> Option<f32> {
        #[cfg(windows)]
        return self.volume.as_ref().and_then(win::Volume::get_db);
        #[cfg(not(windows))]
        None
    }

    fn set_mute(&self, mute: bool) {
        #[cfg(windows)]
        if let Some(volume) = &self.volume {
            volume.set_mute(mute);
        }
        #[cfg(not(windows))]
        let _ = mute;
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
        pub max_db: f32,
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
                let _ = step;
                endpoint.GetVolumeRange(&mut min_db, &mut max_db, &mut step).ok()?;
                Some(Volume { endpoint, min_db, max_db })
            }
        }

        pub fn get_db(&self) -> Option<f32> {
            unsafe { self.endpoint.GetMasterVolumeLevel().ok() }
        }

        pub fn set_mute(&self, mute: bool) {
            unsafe {
                let _ = self.endpoint.SetMute(mute, std::ptr::null());
            }
        }

        pub fn set_db(&self, db: f32) {
            unsafe {
                let _ = self.endpoint.SetMasterVolumeLevel(db, std::ptr::null());
            }
        }
    }
}

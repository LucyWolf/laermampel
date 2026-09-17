//! Gemeinsamer Speicher zwischen Lärmampel und Filter.
//!
//! Der Filter läuft als LOCAL SERVICE und legt den Speicher im globalen Namensraum an,
//! die Lärmampel öffnet ihn nur. Alle Felder sind Atomics, keiner muss auf den anderen warten.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::dsp::{CompSettings, GateSettings, Settings};

pub const MAPPING_NAME: &str = "Global\\Laermampel.Apo";
const MAGIC: u32 = u32::from_le_bytes(*b"LAPO");
/// Bei Änderungen am Aufbau erhöhen, dann erkennt die Lärmampel einen veralteten Filter.
pub const VERSION: u32 = 2;

/// Ein f32 als Atomic, direkt im gemeinsamen Speicher.
#[repr(transparent)]
pub struct SharedF32(AtomicU32);

impl SharedF32 {
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

#[repr(transparent)]
pub struct SharedBool(AtomicU32);

impl SharedBool {
    fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed) != 0
    }

    fn set(&self, value: bool) {
        self.0.store(value as u32, Ordering::Relaxed);
    }
}

#[repr(C)]
pub struct SharedParams {
    magic: AtomicU32,
    version: AtomicU32,

    // Lärmampel → Filter
    gate_enabled: SharedBool,
    gate_threshold_db: SharedF32,
    gate_range_db: SharedF32,
    gate_attack_ms: SharedF32,
    gate_hold_ms: SharedF32,
    gate_release_ms: SharedF32,
    comp_enabled: SharedBool,
    comp_target_db: SharedF32,
    comp_max_gain_db: SharedF32,
    comp_max_cut_db: SharedF32,
    comp_attack_ms: SharedF32,
    comp_release_ms: SharedF32,
    comp_pause_db: SharedF32,
    fader_db: SharedF32,
    muted: SharedBool,
    ceiling_db: SharedF32,

    // Filter → Lärmampel
    /// Zählt bei jedem verarbeiteten Block hoch; bleibt er stehen, läuft kein Filter.
    heartbeat: AtomicU32,
    /// Pegel vor der Bearbeitung in dBFS.
    input_level_db: SharedF32,
    out_level_db: SharedF32,
    out_peak_db: SharedF32,
    gate_open: SharedBool,
    gate_level_db: SharedF32,
    comp_gain_db: SharedF32,
}

/// Rückmeldung des Filters für die Anzeige.
#[derive(Clone, Copy, Debug, Default)]
pub struct Feedback {
    pub input_level_db: f32,
    pub out_level_db: f32,
    pub out_peak_db: f32,
    pub gate_open: bool,
    pub gate_level_db: f32,
    pub comp_gain_db: f32,
}

impl SharedParams {
    /// Beim ersten Anlegen: alles aus, das Signal geht unverändert durch.
    pub fn init(&self) {
        if self.magic.load(Ordering::Relaxed) != MAGIC {
            self.set_settings(&Settings::default());
            self.input_level_db.set(-120.0);
            self.out_level_db.set(-120.0);
            self.out_peak_db.set(-120.0);
            self.version.store(VERSION, Ordering::Relaxed);
            self.magic.store(MAGIC, Ordering::Relaxed);
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic.load(Ordering::Relaxed) == MAGIC
    }

    pub fn version(&self) -> u32 {
        self.version.load(Ordering::Relaxed)
    }

    pub fn settings(&self) -> Settings {
        Settings {
            gate: GateSettings {
                enabled: self.gate_enabled.get(),
                threshold_db: self.gate_threshold_db.get(),
                range_db: self.gate_range_db.get(),
                attack_ms: self.gate_attack_ms.get(),
                hold_ms: self.gate_hold_ms.get(),
                release_ms: self.gate_release_ms.get(),
            },
            comp: CompSettings {
                enabled: self.comp_enabled.get(),
                target_db: self.comp_target_db.get(),
                max_gain_db: self.comp_max_gain_db.get(),
                max_cut_db: self.comp_max_cut_db.get(),
                attack_ms: self.comp_attack_ms.get(),
                release_ms: self.comp_release_ms.get(),
                pause_db: self.comp_pause_db.get(),
            },
            fader_db: self.fader_db.get(),
            muted: self.muted.get(),
            ceiling_db: self.ceiling_db.get(),
        }
    }

    pub fn set_settings(&self, s: &Settings) {
        self.gate_enabled.set(s.gate.enabled);
        self.gate_threshold_db.set(s.gate.threshold_db);
        self.gate_range_db.set(s.gate.range_db);
        self.gate_attack_ms.set(s.gate.attack_ms);
        self.gate_hold_ms.set(s.gate.hold_ms);
        self.gate_release_ms.set(s.gate.release_ms);
        self.comp_enabled.set(s.comp.enabled);
        self.comp_target_db.set(s.comp.target_db);
        self.comp_max_gain_db.set(s.comp.max_gain_db);
        self.comp_max_cut_db.set(s.comp.max_cut_db);
        self.comp_attack_ms.set(s.comp.attack_ms);
        self.comp_release_ms.set(s.comp.release_ms);
        self.comp_pause_db.set(s.comp.pause_db);
        self.fader_db.set(s.fader_db);
        self.muted.set(s.muted);
        self.ceiling_db.set(s.ceiling_db);
    }

    pub fn heartbeat(&self) -> u32 {
        self.heartbeat.load(Ordering::Relaxed)
    }

    pub fn beat(&self) {
        self.heartbeat.fetch_add(1, Ordering::Relaxed);
    }

    pub fn feedback(&self) -> Feedback {
        Feedback {
            input_level_db: self.input_level_db.get(),
            out_level_db: self.out_level_db.get(),
            out_peak_db: self.out_peak_db.get(),
            gate_open: self.gate_open.get(),
            gate_level_db: self.gate_level_db.get(),
            comp_gain_db: self.comp_gain_db.get(),
        }
    }

    pub fn set_feedback(&self, f: &Feedback) {
        self.input_level_db.set(f.input_level_db);
        self.out_level_db.set(f.out_level_db);
        self.out_peak_db.set(f.out_peak_db);
        self.gate_open.set(f.gate_open);
        self.gate_level_db.set(f.gate_level_db);
        self.comp_gain_db.set(f.comp_gain_db);
    }
}

#[cfg(windows)]
pub use mapping::Mapping;

#[cfg(windows)]
mod mapping {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, OpenFileMappingW,
        PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows::core::{HSTRING, w};

    use super::{MAPPING_NAME, SharedParams};

    /// Ein geöffneter gemeinsamer Speicher. Schließt sich beim Wegwerfen.
    pub struct Mapping {
        handle: HANDLE,
        view: MEMORY_MAPPED_VIEW_ADDRESS,
    }

    // Nur ein Handle und ein Zeiger auf Atomics.
    unsafe impl Send for Mapping {}
    unsafe impl Sync for Mapping {}

    impl Mapping {
        /// Für den Filter: anlegen oder, falls schon da, öffnen. Jeder Benutzer darf lesen und schreiben.
        pub fn create() -> Option<Mapping> {
            unsafe {
                let mut descriptor = PSECURITY_DESCRIPTOR::default();
                ConvertStringSecurityDescriptorToSecurityDescriptorW(w!("D:(A;;GA;;;WD)"), SDDL_REVISION_1, &mut descriptor, None).ok()?;
                let attributes = SECURITY_ATTRIBUTES {
                    nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor.0,
                    bInheritHandle: false.into(),
                };
                let size = size_of::<SharedParams>() as u32;
                let handle = CreateFileMappingW(HANDLE(-1isize as _), Some(&attributes), PAGE_READWRITE, 0, size, &HSTRING::from(MAPPING_NAME));
                let _ = LocalFree(Some(HLOCAL(descriptor.0)));
                Self::map(handle.ok()?, true)
            }
        }

        /// Für die Lärmampel: nur öffnen. `None`, solange kein Filter läuft.
        pub fn open() -> Option<Mapping> {
            unsafe {
                let handle = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &HSTRING::from(MAPPING_NAME)).ok()?;
                Self::map(handle, false)
            }
        }

        unsafe fn map(handle: HANDLE, init: bool) -> Option<Mapping> {
            unsafe {
                let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size_of::<SharedParams>());
                if view.Value.is_null() {
                    let _ = CloseHandle(handle);
                    return None;
                }
                let mapping = Mapping { handle, view };
                if init {
                    mapping.params().init();
                }
                Some(mapping)
            }
        }

        pub fn params(&self) -> &SharedParams {
            unsafe { &*(self.view.Value as *const SharedParams) }
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                let _ = UnmapViewOfFile(self.view);
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

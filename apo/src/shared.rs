//! Gemeinsamer Speicher zwischen Lärmampel und Filter.
//!
//! Der Filter läuft als LOCAL SERVICE und legt den Speicher im globalen Namensraum an,
//! die Lärmampel öffnet ihn nur. Alle Felder sind Atomics, keiner muss auf den anderen warten.

use std::sync::atomic::{AtomicU32, Ordering};

pub const MAPPING_NAME: &str = "Global\\Laermampel.Apo";
const MAGIC: u32 = u32::from_le_bytes(*b"LAPO");
/// Bei Änderungen am Aufbau erhöhen, dann erkennt die Lärmampel einen veralteten Filter.
pub const VERSION: u32 = 1;

#[repr(C)]
pub struct SharedParams {
    magic: AtomicU32,
    version: AtomicU32,

    // Lärmampel → Filter
    gain_db: AtomicU32,
    muted: AtomicU32,

    // Filter → Lärmampel
    /// Zählt bei jedem verarbeiteten Block hoch; bleibt er stehen, läuft kein Filter.
    heartbeat: AtomicU32,
    /// Pegel vor der Bearbeitung in dBFS.
    input_level_db: AtomicU32,
}

impl SharedParams {
    pub fn init(&self) {
        if self.magic.load(Ordering::Relaxed) != MAGIC {
            self.gain_db.store(0f32.to_bits(), Ordering::Relaxed);
            self.muted.store(0, Ordering::Relaxed);
            self.input_level_db.store((-120f32).to_bits(), Ordering::Relaxed);
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

    pub fn gain_db(&self) -> f32 {
        f32::from_bits(self.gain_db.load(Ordering::Relaxed))
    }

    pub fn set_gain_db(&self, db: f32) {
        self.gain_db.store(db.to_bits(), Ordering::Relaxed);
    }

    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed) != 0
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted as u32, Ordering::Relaxed);
    }

    pub fn heartbeat(&self) -> u32 {
        self.heartbeat.load(Ordering::Relaxed)
    }

    pub fn beat(&self) {
        self.heartbeat.fetch_add(1, Ordering::Relaxed);
    }

    pub fn input_level_db(&self) -> f32 {
        f32::from_bits(self.input_level_db.load(Ordering::Relaxed))
    }

    pub fn set_input_level_db(&self, db: f32) {
        self.input_level_db.store(db.to_bits(), Ordering::Relaxed);
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

//! Nur eine Lärmampel gleichzeitig. Ein zweiter Start öffnet stattdessen
//! die Einstellungen der laufenden.

use std::time::Duration;

pub use imp::Guard;

/// `wait` > 0 wartet so lange, bis eine alte Instanz beendet ist (nach einem Update).
/// Gibt `None` zurück, wenn schon eine läuft; die wird dann benachrichtigt.
pub fn acquire(wait: Duration) -> Option<Guard> {
    imp::acquire(wait)
}

#[cfg(windows)]
mod imp {
    use std::ptr::null;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    const MUTEX_NAME: &str = "Local\\Laermampel.Instanz";
    const EVENT_NAME: &str = "Local\\Laermampel.Anzeigen";

    pub struct Guard {
        mutex: HANDLE,
        event: HANDLE,
    }

    // Die Handles sind nur Kernel-Objekt-Nummern und dürfen zwischen Threads wandern.
    unsafe impl Send for Guard {}

    impl Guard {
        /// `true`, wenn seit dem letzten Aufruf jemand die Lärmampel nochmal gestartet hat.
        pub fn show_requested(&self) -> bool {
            !self.event.is_null() && unsafe { WaitForSingleObject(self.event, 0) } == WAIT_OBJECT_0
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.event);
                CloseHandle(self.mutex);
            }
        }
    }

    pub fn acquire(wait: Duration) -> Option<Guard> {
        let started = Instant::now();
        let mutex_name = wide(MUTEX_NAME);
        let event_name = wide(EVENT_NAME);
        loop {
            let mutex = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
            let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            if !exists {
                // Auto-Reset-Event, damit jede Anfrage nur einmal zählt.
                let event = unsafe { CreateEventW(null(), 0, 0, event_name.as_ptr()) };
                return Some(Guard { mutex, event });
            }
            unsafe { CloseHandle(mutex) };
            if started.elapsed() >= wait {
                unsafe {
                    let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                    if !event.is_null() {
                        SetEvent(event);
                        CloseHandle(event);
                    }
                }
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::time::Duration;

    pub struct Guard;

    impl Guard {
        pub fn show_requested(&self) -> bool {
            false
        }
    }

    pub fn acquire(_wait: Duration) -> Option<Guard> {
        Some(Guard)
    }
}

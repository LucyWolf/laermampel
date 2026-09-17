//! „Mit Windows starten“ über den Run-Schlüssel des Benutzers.
//! Der Installer schreibt denselben Wert, beides passt also zusammen.

#[cfg(windows)]
mod imp {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE_NAME: &str = "Laermampel";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Was im Run-Schlüssel steht, oder `None`, wenn es keinen Eintrag gibt.
    fn stored_command() -> Option<String> {
        let (key, value) = (wide(RUN_KEY), wide(VALUE_NAME));
        unsafe {
            let mut size = 0u32;
            let status =
                RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size);
            if status != ERROR_SUCCESS {
                return None;
            }
            let mut buffer = vec![0u16; size as usize / 2 + 1];
            let mut size = (buffer.len() * 2) as u32;
            let status = RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            );
            if status != ERROR_SUCCESS {
                return None;
            }
            let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..end]))
        }
    }

    /// Der Befehl, den diese Lärmampel eintragen würde.
    fn own_command() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        Some(format!("\"{}\"", exe.display()))
    }

    pub fn is_enabled() -> bool {
        // Zeigt der Eintrag auf eine andere Datei (verschoben, anders installiert), gilt er nicht.
        // Sonst stünde das Häkchen auf „an“, während Windows etwas anderes oder nichts startet.
        match (stored_command(), own_command()) {
            (Some(stored), Some(own)) => stored.eq_ignore_ascii_case(&own),
            (Some(_), None) => true,
            _ => false,
        }
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let (key, value) = (wide(RUN_KEY), wide(VALUE_NAME));
        let status = if enabled {
            let command = own_command().ok_or_else(|| "Eigener Pfad ist unbekannt".to_string())?;
            let data = wide(&command);
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) }
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("Autostart konnte nicht geändert werden (Fehler {status})"))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn is_enabled() -> bool {
        false
    }

    pub fn set_enabled(_enabled: bool) -> Result<(), String> {
        Err("Autostart gibt es bisher nur unter Windows".to_string())
    }
}

pub use imp::{is_enabled, set_enabled};

pub const SUPPORTED: bool = cfg!(windows);

// Unter Linux gibt es den Filter nicht, die Hilfsfunktionen bleiben nur für die Tests.
#![cfg_attr(not(windows), allow(dead_code))]
//! Den Audio-Filter (APO) beim Mikrofon eintragen und wieder austragen.
//!
//! Das braucht Adminrechte. Die Lärmampel startet sich dafür selbst mit `--apo …` als Admin
//! (Windows fragt per UAC). Vor jeder Änderung werden die ursprünglichen Werte des Mikrofons
//! gesichert, damit „Entfernen“ genau den alten Zustand wiederherstellt.

pub use imp::*;

/// Endpunkt-GUID aus der cpal-Geräte-ID, z.B. `wasapi:{0.0.1.00000000}.{abcd-…}` → `{abcd-…}`.
pub fn endpoint_guid(device_id: &str) -> Option<String> {
    let id = device_id.strip_prefix("wasapi:")?;
    // Aufnahmegeräte beginnen mit {0.0.1.…}, Wiedergabe mit {0.0.0.…}.
    if !id.starts_with("{0.0.1.") {
        return None;
    }
    let guid = id.rsplit('.').next()?;
    (guid.starts_with('{') && guid.ends_with('}') && guid.len() == 38).then(|| guid.to_string())
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use laermampel_apo::APO_CLSID_STRING;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
        TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS, KEY_READ, REG_DWORD, REG_OPTION_BACKUP_RESTORE, REG_OPTION_NON_VOLATILE,
        REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW,
        RegQueryValueExW, RegSetValueExW,
    };
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_STATUS, SERVICE_STOP,
        SERVICE_STOPPED, StartServiceW,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    const DLL_NAME: &str = "laermampel_apo.dll";
    const CAPTURE_ROOT: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Capture";
    const BACKUP_ROOT: &str = "SOFTWARE\\LucyWolf\\Laermampel\\ApoBackup";
    /// Unter diesem Namen merkt sich die Sicherung, welche Werte vorher gar nicht existierten.
    const MISSING_PREFIX: &str = "fehlte:";

    /// PKEY_FX_PreMixEffectClsid (ältere Treiber) und PKEY_FX_StreamEffectClsid (ab Windows 8.1).
    const PREMIX_EFFECT: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},1";
    const STREAM_EFFECT: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5";
    /// PKEY_SFX_ProcessingModes_Supported_For_Streaming: ohne Modus lädt Windows den Stream-Filter nicht.
    const STREAM_MODES: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},5";
    const DEFAULT_MODE: &str = "{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}";
    const TOUCHED: [&str; 3] = [PREMIX_EFFECT, STREAM_EFFECT, STREAM_MODES];

    /// IID von IAudioProcessingObject, steht so in der APO-Registrierung.
    const APO_INTERFACE: &str = "{FD7F2B29-24D0-4B5C-B177-592C39F9CA10}";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // ---------------------------------------------------------------- Status (ohne Adminrechte)

    /// Ist unser Filter bei diesem Mikrofon eingetragen?
    pub fn is_installed(guid: &str) -> bool {
        let path = format!("{CAPTURE_ROOT}\\{guid}\\FxProperties");
        let Some(key) = Key::open_read(&path) else { return false };
        [PREMIX_EFFECT, STREAM_EFFECT]
            .iter()
            .any(|name| key.read_string(name).is_some_and(|v| v.eq_ignore_ascii_case(APO_CLSID_STRING)))
    }

    /// Hatte das Mikrofon schon einen Filter vom Hersteller, den wir ersetzen würden?
    pub fn has_vendor_effect(guid: &str) -> bool {
        let path = format!("{CAPTURE_ROOT}\\{guid}\\FxProperties");
        let Some(key) = Key::open_read(&path) else { return false };
        [PREMIX_EFFECT, STREAM_EFFECT]
            .iter()
            .filter_map(|name| key.read_string(name))
            .any(|v| !v.is_empty() && !v.eq_ignore_ascii_case(APO_CLSID_STRING))
    }

    /// Gibt es irgendwo noch eine Sicherung, also einen eingetragenen Filter?
    pub fn any_installed() -> bool {
        Key::open_read(BACKUP_ROOT).is_some_and(|key| !key.subkeys().is_empty())
    }

    // ---------------------------------------------------------------- Mit Adminrechten starten

    /// Startet die Lärmampel mit Adminrechten und den gegebenen Argumenten, wartet aufs Ende.
    pub fn run_elevated(args: &str) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(args));
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            info.nShow = SW_HIDE;
            if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
                return Err("Adminrechte wurden nicht erteilt.".to_string());
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 1u32;
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
            match code {
                0 => Ok(()),
                _ => Err(format!("Einrichten fehlgeschlagen (Code {code}), Details in der Log-Datei.")),
            }
        }
    }

    /// Wird in `main` aufgerufen. Gibt einen Exit-Code zurück, wenn es ein APO-Aufruf war.
    pub fn handle_command_line() -> Option<i32> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["--apo", "install", guid] => install(guid),
            ["--apo", "uninstall", guid] => uninstall(guid),
            ["--apo", "uninstall-all"] => uninstall_all(),
            // Für den Deinstaller: nur nach Adminrechten fragen, wenn wirklich etwas eingetragen ist.
            ["--apo-elevate", "uninstall-all"] => {
                if any_installed() {
                    run_elevated("--apo uninstall-all")
                } else {
                    Ok(())
                }
            }
            _ => return None,
        };
        Some(match result {
            Ok(()) => {
                log!("APO: {:?} erledigt", args);
                0
            }
            Err(e) => {
                log!("APO: {:?} fehlgeschlagen: {e}", args);
                1
            }
        })
    }

    // ---------------------------------------------------------------- Die eigentlichen Schritte (als Admin)

    fn install(guid: &str) -> Result<(), String> {
        enable_privileges()?;
        let source = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .with_file_name(DLL_NAME);
        if !source.exists() {
            return Err(format!("{} fehlt neben der Lärmampel", DLL_NAME));
        }

        with_audio_stopped(|| {
            let target = dll_target()?;
            std::fs::create_dir_all(target.parent().unwrap_or(&target)).map_err(|e| e.to_string())?;
            copy_with_retry(&source, &target)?;
            register_com(&target)?;
            backup_endpoint(guid)?;

            let fx = Key::create_backup_restore(&format!("{CAPTURE_ROOT}\\{guid}\\FxProperties"))?;
            fx.write_string(PREMIX_EFFECT, APO_CLSID_STRING)?;
            fx.write_string(STREAM_EFFECT, APO_CLSID_STRING)?;
            fx.write_multi_string(STREAM_MODES, &[DEFAULT_MODE])?;
            log!("APO: bei {guid} eingetragen");
            Ok(())
        })
    }

    fn uninstall(guid: &str) -> Result<(), String> {
        enable_privileges()?;
        with_audio_stopped(|| {
            restore_endpoint(guid)?;
            if !any_installed() {
                unregister_com();
            }
            Ok(())
        })
    }

    fn uninstall_all() -> Result<(), String> {
        enable_privileges()?;
        with_audio_stopped(|| {
            let guids = Key::open_read(BACKUP_ROOT).map(|k| k.subkeys()).unwrap_or_default();
            for guid in guids {
                restore_endpoint(&guid)?;
            }
            unregister_com();
            Ok(())
        })
    }

    /// Nach dem Anhalten des Audiodienstes hält Windows die alte DLL manchmal noch kurz fest.
    fn copy_with_retry(source: &std::path::Path, target: &std::path::Path) -> Result<(), String> {
        let started = Instant::now();
        loop {
            match std::fs::copy(source, target) {
                Ok(_) => return Ok(()),
                Err(e) if started.elapsed() < Duration::from_secs(10) => {
                    log!("APO: DLL noch belegt ({e}), neuer Versuch");
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(format!("DLL kopieren: {e}")),
            }
        }
    }

    fn dll_target() -> Result<PathBuf, String> {
        let program_files = std::env::var("ProgramW6432")
            .or_else(|_| std::env::var("ProgramFiles"))
            .map_err(|_| "Programme-Ordner nicht gefunden".to_string())?;
        Ok(PathBuf::from(program_files).join("Laermampel").join(DLL_NAME))
    }

    fn register_com(dll: &std::path::Path) -> Result<(), String> {
        let clsid = Key::create(&format!("SOFTWARE\\Classes\\CLSID\\{APO_CLSID_STRING}"))?;
        clsid.write_string("", "Lärmampel Audio-Filter")?;
        let server = Key::create(&format!("SOFTWARE\\Classes\\CLSID\\{APO_CLSID_STRING}\\InprocServer32"))?;
        server.write_string("", &dll.to_string_lossy())?;
        server.write_string("ThreadingModel", "Both")?;

        let apo = Key::create(&format!("SOFTWARE\\Classes\\AudioEngine\\AudioProcessingObjects\\{APO_CLSID_STRING}"))?;
        apo.write_string("FriendlyName", "Lärmampel")?;
        apo.write_string("Copyright", "LucyWolf")?;
        for (name, value) in [
            ("MajorVersion", 1u32),
            ("MinorVersion", 0),
            ("Flags", 0x0E), // APO_FLAG_DEFAULT
            ("MinInputConnections", 1),
            ("MaxInputConnections", 1),
            ("MinOutputConnections", 1),
            ("MaxOutputConnections", 1),
            ("MaxInstances", u32::MAX),
            ("NumAPOInterfaces", 1),
        ] {
            apo.write_dword(name, value)?;
        }
        apo.write_string("APOInterface0", APO_INTERFACE)
    }

    fn unregister_com() {
        delete_tree(&format!("SOFTWARE\\Classes\\CLSID\\{APO_CLSID_STRING}"));
        delete_tree(&format!("SOFTWARE\\Classes\\AudioEngine\\AudioProcessingObjects\\{APO_CLSID_STRING}"));
        if let Ok(dll) = dll_target() {
            let _ = std::fs::remove_file(&dll);
            if let Some(dir) = dll.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }

    /// Sichert die Originalwerte, aber nur beim ersten Eintragen: sonst würden wir unsere eigenen sichern.
    fn backup_endpoint(guid: &str) -> Result<(), String> {
        let backup_path = format!("{BACKUP_ROOT}\\{guid}");
        if Key::open_read(&backup_path).is_some() {
            return Ok(());
        }
        let fx = Key::create_backup_restore(&format!("{CAPTURE_ROOT}\\{guid}\\FxProperties"))?;
        let backup = Key::create(&backup_path)?;
        for name in TOUCHED {
            match fx.read_raw(name) {
                Some((kind, data)) => backup.write_raw(name, kind, &data)?,
                None => backup.write_dword(&format!("{MISSING_PREFIX}{name}"), 1)?,
            }
        }
        log!("APO: Originalwerte von {guid} gesichert");
        Ok(())
    }

    fn restore_endpoint(guid: &str) -> Result<(), String> {
        let backup_path = format!("{BACKUP_ROOT}\\{guid}");
        let Some(backup) = Key::open_read(&backup_path) else {
            return Ok(());
        };
        let fx = Key::create_backup_restore(&format!("{CAPTURE_ROOT}\\{guid}\\FxProperties"))?;
        for name in TOUCHED {
            match backup.read_raw(name) {
                Some((kind, data)) => fx.write_raw(name, kind, &data)?,
                None => fx.delete_value(name),
            }
        }
        drop(backup);
        delete_tree(&backup_path);
        log!("APO: Originalwerte von {guid} wiederhergestellt");
        Ok(())
    }

    fn delete_tree(path: &str) {
        let path = wide(path);
        unsafe {
            RegDeleteTreeW(HKEY_LOCAL_MACHINE, path.as_ptr());
            let _ = windows_sys::Win32::System::Registry::RegDeleteKeyW(HKEY_LOCAL_MACHINE, path.as_ptr());
        }
    }

    /// Die Mikrofon-Einträge gehören dem System. Mit Sicherungs- und Wiederherstellungsrecht
    /// darf ein Admin sie trotzdem ändern, ohne die Berechtigungen umzubiegen.
    fn enable_privileges() -> Result<(), String> {
        unsafe {
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) == 0 {
                return Err("Kein Zugriff auf die eigenen Rechte".to_string());
            }
            for name in ["SeBackupPrivilege", "SeRestorePrivilege"] {
                let mut luid = LUID { LowPart: 0, HighPart: 0 };
                let wide_name = wide(name);
                if LookupPrivilegeValueW(null(), wide_name.as_ptr(), &mut luid) == 0 {
                    CloseHandle(token);
                    return Err(format!("Recht {name} unbekannt"));
                }
                let privileges = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
                };
                AdjustTokenPrivileges(token, 0, &privileges, 0, null_mut(), null_mut());
            }
            CloseHandle(token);
        }
        Ok(())
    }

    /// Audiodienst anhalten, `work` ausführen, Dienst wieder starten (auch wenn `work` scheitert).
    fn with_audio_stopped(work: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        let service = Service::open("Audiosrv")?;
        service.stop()?;
        let result = work();
        let started = service.start();
        result.and(started)
    }

    struct Service {
        manager: *mut core::ffi::c_void,
        handle: *mut core::ffi::c_void,
    }

    impl Service {
        fn open(name: &str) -> Result<Service, String> {
            unsafe {
                let manager = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
                if manager.is_null() {
                    return Err("Dienstverwaltung nicht erreichbar".to_string());
                }
                let wide_name = wide(name);
                let handle = OpenServiceW(manager, wide_name.as_ptr(), SERVICE_STOP | SERVICE_START | SERVICE_QUERY_STATUS);
                if handle.is_null() {
                    CloseServiceHandle(manager);
                    return Err(format!("Dienst {name} nicht erreichbar"));
                }
                Ok(Service { manager, handle })
            }
        }

        fn state(&self) -> u32 {
            unsafe {
                let mut status: SERVICE_STATUS = std::mem::zeroed();
                QueryServiceStatus(self.handle, &mut status);
                status.dwCurrentState
            }
        }

        fn wait_for(&self, state: u32) -> bool {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(15) {
                if self.state() == state {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            false
        }

        fn stop(&self) -> Result<(), String> {
            unsafe {
                let mut status: SERVICE_STATUS = std::mem::zeroed();
                ControlService(self.handle, SERVICE_CONTROL_STOP, &mut status);
            }
            if self.wait_for(SERVICE_STOPPED) { Ok(()) } else { Err("Audiodienst lässt sich nicht anhalten".to_string()) }
        }

        fn start(&self) -> Result<(), String> {
            unsafe {
                StartServiceW(self.handle, 0, null());
            }
            if self.wait_for(SERVICE_RUNNING) { Ok(()) } else { Err("Audiodienst startet nicht wieder".to_string()) }
        }
    }

    impl Drop for Service {
        fn drop(&mut self) {
            unsafe {
                CloseServiceHandle(self.handle);
                CloseServiceHandle(self.manager);
            }
        }
    }

    /// Kleiner Helfer um Registry-Schlüssel unter HKLM.
    struct Key(HKEY);

    impl Key {
        fn open_read(path: &str) -> Option<Key> {
            let path = wide(path);
            let mut key: HKEY = null_mut();
            let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) };
            (status == ERROR_SUCCESS).then_some(Key(key))
        }

        fn create(path: &str) -> Result<Key, String> {
            Self::create_with(path, REG_OPTION_NON_VOLATILE)
        }

        fn create_backup_restore(path: &str) -> Result<Key, String> {
            Self::create_with(path, REG_OPTION_BACKUP_RESTORE)
        }

        fn create_with(path: &str, options: u32) -> Result<Key, String> {
            let wide_path = wide(path);
            let mut key: HKEY = null_mut();
            let status = unsafe {
                RegCreateKeyExW(HKEY_LOCAL_MACHINE, wide_path.as_ptr(), 0, null(), options, KEY_ALL_ACCESS, null(), &mut key, null_mut())
            };
            if status == ERROR_SUCCESS { Ok(Key(key)) } else { Err(format!("Registry {path}: Fehler {status}")) }
        }

        fn read_raw(&self, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
            let name = wide(name);
            let mut kind: REG_VALUE_TYPE = 0;
            let mut size = 0u32;
            unsafe {
                if RegQueryValueExW(self.0, name.as_ptr(), null(), &mut kind, null_mut(), &mut size) != ERROR_SUCCESS {
                    return None;
                }
                let mut data = vec![0u8; size as usize];
                if RegQueryValueExW(self.0, name.as_ptr(), null(), &mut kind, data.as_mut_ptr(), &mut size) != ERROR_SUCCESS {
                    return None;
                }
                data.truncate(size as usize);
                Some((kind, data))
            }
        }

        fn read_string(&self, name: &str) -> Option<String> {
            let (kind, data) = self.read_raw(name)?;
            if kind != REG_SZ {
                return None;
            }
            let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
        }

        fn write_raw(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), String> {
            let wide_name = wide(name);
            let status = unsafe { RegSetValueExW(self.0, wide_name.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
            if status == ERROR_SUCCESS { Ok(()) } else { Err(format!("Registry-Wert {name}: Fehler {status}")) }
        }

        fn write_string(&self, name: &str, value: &str) -> Result<(), String> {
            let bytes: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
            self.write_raw(name, REG_SZ, &bytes)
        }

        fn write_multi_string(&self, name: &str, values: &[&str]) -> Result<(), String> {
            let mut units: Vec<u16> = Vec::new();
            for value in values {
                units.extend(value.encode_utf16());
                units.push(0);
            }
            units.push(0);
            let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
            self.write_raw(name, windows_sys::Win32::System::Registry::REG_MULTI_SZ, &bytes)
        }

        fn write_dword(&self, name: &str, value: u32) -> Result<(), String> {
            self.write_raw(name, REG_DWORD, &value.to_le_bytes())
        }

        fn delete_value(&self, name: &str) {
            let name = wide(name);
            let status = unsafe { RegDeleteValueW(self.0, name.as_ptr()) };
            let _ = status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND;
        }

        fn subkeys(&self) -> Vec<String> {
            let mut names = Vec::new();
            for index in 0.. {
                let mut buffer = [0u16; 256];
                let mut length = buffer.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(self.0, index, buffer.as_mut_ptr(), &mut length, null(), null_mut(), null_mut(), null_mut())
                };
                if status != ERROR_SUCCESS {
                    break;
                }
                names.push(String::from_utf16_lossy(&buffer[..length as usize]));
            }
            names
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                RegCloseKey(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn is_installed(_guid: &str) -> bool {
        false
    }

    pub fn has_vendor_effect(_guid: &str) -> bool {
        false
    }

    pub fn run_elevated(_args: &str) -> Result<(), String> {
        Err("Nur unter Windows".to_string())
    }

    pub fn handle_command_line() -> Option<i32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint_guid;

    #[test]
    fn guid_aus_geraete_id() {
        assert_eq!(
            endpoint_guid("wasapi:{0.0.1.00000000}.{a1b2c3d4-0000-1111-2222-333344445555}").as_deref(),
            Some("{a1b2c3d4-0000-1111-2222-333344445555}")
        );
        // Wiedergabegeräte und fremde Hosts gehen nicht.
        assert_eq!(endpoint_guid("wasapi:{0.0.0.00000000}.{a1b2c3d4-0000-1111-2222-333344445555}"), None);
        assert_eq!(endpoint_guid("alsa:hw:0,0"), None);
    }
}

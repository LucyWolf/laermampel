//! Sucht im USB-Protokoll des Headsets nach der Mute-Taste.
//!
//! Windows erfährt von einer Stummschaltung im Headset nichts: Die Taste wirkt im Gerät
//! selbst, gemeldet wird sie nur über ein herstellereigenes HID-Paket. Welches Byte darin
//! umklappt, steht nirgends – das muss man einmal mitschreiben, während jemand die Taste
//! drückt. Genau das macht dieser Sucher: er hört an allen Schnittstellen des Headsets mit
//! und schreibt jede Änderung mit Zeitstempel in eine Datei.

// Unter Linux wird nur gegengelesen, benutzt wird der Sucher allein unter Windows.
#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
use std::path::PathBuf;

/// ROCCAT. Andere Hersteller haben eigene Nummern; gesucht wird nach allen Geräten dieser Nummer.
pub const ROCCAT_VID: u16 = 0x1e7d;

/// Wie lange mitgehört wird.
pub const SCAN_SECONDS: u64 = 12;

#[cfg(windows)]
pub fn scan_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("de", "LucyWolf", "Laermampel")?;
    Some(dirs.data_local_dir().join("headset-tasten.txt"))
}

/// Hört `SCAN_SECONDS` lang mit und schreibt alles Auffällige in eine Datei.
#[cfg(windows)]
pub fn scan() -> Result<PathBuf, String> {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    let api = hidapi::HidApi::new().map_err(|e| format!("HID lässt sich nicht öffnen: {e}"))?;
    let mut bericht = String::new();
    let _ = writeln!(bericht, "Lärmampel v{} – Suche nach der Mute-Taste", crate::updater::CURRENT_VERSION);
    let _ = writeln!(bericht, "Gesucht wird nach Geräten von ROCCAT (Hersteller-Nummer {ROCCAT_VID:#06x}).\n");

    // Ein Gerät meldet sich unter mehreren Schnittstellen; jede kann die Taste melden.
    let mut offen = Vec::new();
    for info in api.device_list().filter(|d| d.vendor_id() == ROCCAT_VID) {
        let name = format!(
            "Gerät {:#06x}:{:#06x} „{}“, Schnittstelle {}, Verwendung {:#06x}/{:#06x}",
            info.vendor_id(),
            info.product_id(),
            info.product_string().unwrap_or("?"),
            info.interface_number(),
            info.usage_page(),
            info.usage(),
        );
        match info.open_device(&api) {
            Ok(gerät) => {
                if let Err(e) = gerät.set_blocking_mode(false) {
                    let _ = writeln!(bericht, "{name}: lässt sich nicht auf Mithören stellen ({e})");
                    continue;
                }
                let _ = writeln!(bericht, "[{}] {name}", offen.len());
                offen.push((offen.len(), gerät));
            }
            // Belegte Schnittstellen sind normal: Windows und die ROCCAT-Software halten welche.
            Err(e) => {
                let _ = writeln!(bericht, "(belegt) {name}: {e}");
            }
        }
    }

    if offen.is_empty() {
        let _ = writeln!(
            bericht,
            "\nKeine Schnittstelle ließ sich öffnen. Läuft die ROCCAT-Software (Swarm/Neon)? \
             Die hält die Schnittstellen unter Umständen für sich – dann bitte beenden und nochmal."
        );
        schreiben(&bericht)?;
        return Err("Keine Schnittstelle zum Mithören gefunden".to_string());
    }

    let _ = writeln!(
        bericht,
        "\nMitschrift (nur Änderungen). Jetzt die Mute-Taste mehrmals drücken:\n\
         Zeit    Schnittstelle  Länge  Pakethalt"
    );

    let start = Instant::now();
    let mut letztes: Vec<Option<Vec<u8>>> = vec![None; offen.len()];
    let mut zeilen = 0;
    while start.elapsed() < Duration::from_secs(SCAN_SECONDS) {
        for (nummer, gerät) in &offen {
            let mut puffer = [0u8; 64];
            match gerät.read_timeout(&mut puffer, 5) {
                Ok(0) => {}
                Ok(n) => {
                    let paket = puffer[..n].to_vec();
                    // Nur Änderungen, sonst ersäuft die Datei in Wiederholungen.
                    if letztes[*nummer].as_deref() != Some(paket.as_slice()) {
                        let hex: Vec<String> = paket.iter().map(|b| format!("{b:02x}")).collect();
                        let _ = writeln!(bericht, "{:6.2}s  [{nummer}]           {n:3}    {}", start.elapsed().as_secs_f32(), hex.join(" "));
                        letztes[*nummer] = Some(paket);
                        zeilen += 1;
                    }
                }
                Err(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    if zeilen == 0 {
        let _ = writeln!(
            bericht,
            "\nEs kam gar nichts an. Entweder meldet das Headset die Taste nicht über USB, \
             oder die geöffneten Schnittstellen sind die falschen."
        );
    } else {
        let _ = writeln!(bericht, "\n{zeilen} Änderungen mitgeschrieben.");
    }
    schreiben(&bericht)
}

#[cfg(windows)]
fn schreiben(bericht: &str) -> Result<PathBuf, String> {
    let pfad = scan_path().ok_or_else(|| "Kein Ordner für die Datei gefunden".to_string())?;
    if let Some(ordner) = pfad.parent() {
        std::fs::create_dir_all(ordner).map_err(|e| e.to_string())?;
    }
    std::fs::write(&pfad, bericht).map_err(|e| e.to_string())?;
    Ok(pfad)
}

#[cfg(not(windows))]
pub fn scan() -> Result<std::path::PathBuf, String> {
    Err("Nur unter Windows".to_string())
}

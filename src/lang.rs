//! Zwei Sprachen: Deutsch und Englisch.
//!
//! Die Texte stehen paarweise direkt an der Stelle, an der sie gebraucht werden
//! (`t("Anzeige", "Display")`). Das spart eine Schlüsseldatei und sorgt dafür, dass beim
//! Ändern eines Textes die Übersetzung gar nicht erst vergessen werden kann.
//!
//! Welche Sprache gilt, steht global: so muss sie nicht durch jede Funktion gereicht werden.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Language {
    /// Sprache von Windows übernehmen.
    Auto,
    German,
    English,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::Auto, Language::German, Language::English];

    /// Steht in beiden Sprachen gleich da, damit man sich zurückfindet.
    pub fn label(self) -> &'static str {
        match self {
            Language::Auto => "Automatisch · Automatic",
            Language::German => "Deutsch",
            Language::English => "English",
        }
    }
}

const DE: u8 = 0;
const EN: u8 = 1;
static CURRENT: AtomicU8 = AtomicU8::new(DE);

/// Einstellung übernehmen. Bei `Auto` entscheidet die Sprache von Windows.
pub fn apply(language: Language) {
    let code = match language {
        Language::Auto => {
            if system_is_german() {
                DE
            } else {
                EN
            }
        }
        Language::German => DE,
        Language::English => EN,
    };
    CURRENT.store(code, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    CURRENT.load(Ordering::Relaxed) == EN
}

/// Ein Text in beiden Sprachen.
pub fn t(de: &'static str, en: &'static str) -> &'static str {
    if is_english() { en } else { de }
}

#[cfg(windows)]
fn system_is_german() -> bool {
    // Die unteren zehn Bit sind die Hauptsprache; 0x07 ist Deutsch.
    const LANG_GERMAN: u16 = 0x07;
    unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() & 0x3ff == LANG_GERMAN }
}

#[cfg(not(windows))]
fn system_is_german() -> bool {
    std::env::var("LANG").or_else(|_| std::env::var("LC_ALL")).is_ok_and(|l| l.starts_with("de"))
}

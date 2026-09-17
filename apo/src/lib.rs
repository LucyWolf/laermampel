//! Audio-Filter (APO) der Lärmampel.
//!
//! Windows lädt diese DLL in den Audiodienst und schickt jedes Stück Mikrofon-Audio hindurch,
//! bevor Discord, Spiele usw. es bekommen. So wirken Lautstärke und Mute in allen Programmen,
//! ohne virtuelles Gerät. Die Lärmampel stellt die Werte über einen gemeinsamen Speicher ein.

pub mod shared;

#[cfg(windows)]
mod apo;
#[cfg(windows)]
pub use apo::{APO_CLSID, APO_CLSID_STRING};

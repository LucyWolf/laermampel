//! Update-Prüfung und Update über den Installer aus den GitHub-Releases.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;

use crate::lang::t;
use ureq::ResponseExt;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_PAGE: &str = "https://github.com/LucyWolf/laermampel/releases/latest";
const RELEASE_DOWNLOAD_BASE: &str = "https://github.com/LucyWolf/laermampel/releases/download";
const SETUP_PREFIX: &str = "Laermampel-Setup-";
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// Startargument der neuen Version nach einem Update.
pub const RESTART_ARG: &str = "--nach-update";

#[derive(Clone)]
pub struct Release {
    pub version: semver::Version,
    pub page_url: String,
    setup_url: String,
}

impl Release {
    /// Installieren geht nur unter Windows, woanders gibt es nur den Link.
    pub fn installable(&self) -> bool {
        cfg!(windows)
    }
}

#[derive(Clone)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    /// Installer läuft, dieses Programm beendet sich gleich; der Installer startet die neue Version.
    Installed(Release),
    Failed(String),
}

pub struct Updater {
    status: Arc<Mutex<Status>>,
}

impl Updater {
    pub fn new() -> Self {
        Self { status: Arc::new(Mutex::new(Status::Idle)) }
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or(Status::Idle)
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.status(), Status::Checking | Status::Installing(_))
    }

    pub fn check(&self, ctx: &egui::Context) {
        if self.is_busy() {
            return;
        }
        self.run(ctx, Status::Checking, || match fetch_latest() {
            Ok(release) if release.version > current_version() => Status::Available(release),
            Ok(_) => Status::UpToDate,
            Err(e) => {
                log!("Update-Prüfung fehlgeschlagen: {e}");
                Status::Failed(format!("{}: {e}", t("Update-Prüfung fehlgeschlagen", "Update check failed")))
            }
        });
    }

    pub fn install(&self, ctx: &egui::Context, release: Release) {
        if self.is_busy() {
            return;
        }
        self.run(ctx, Status::Installing(release.clone()), move || match download_and_run_setup(&release) {
            Ok(()) => Status::Installed(release),
            Err(e) => {
                log!("Update fehlgeschlagen: {e}");
                Status::Failed(format!("{}: {e}", t("Update fehlgeschlagen", "Update failed")))
            }
        });
    }

    fn run(&self, ctx: &egui::Context, busy: Status, job: impl FnOnce() -> Status + Send + 'static) {
        if let Ok(mut s) = self.status.lock() {
            *s = busy;
        }
        let status = Arc::clone(&self.status);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = job();
            if let Ok(mut s) = status.lock() {
                *s = result;
            }
            ctx.request_repaint();
        });
    }
}

fn current_version() -> semver::Version {
    semver::Version::parse(CURRENT_VERSION).expect("Cargo-Version ist gültiges semver")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(format!("laermampel/{CURRENT_VERSION}"))
        .build()
        .into()
}

/// Neueste Version über die normale Release-Seite: `…/releases/latest` leitet auf `…/tag/vX.Y.Z` um.
/// Die GitHub-API wäre ohne Anmeldung auf 60 Abfragen pro Stunde begrenzt (dann HTTP 403).
///
/// Die Seite selbst liegt bei GitHub im Zwischenspeicher: direkt nach einer neuen Version zeigt
/// sie noch minutenlang auf die alte. Ein eindeutiger Anhang an der Adresse umgeht das.
fn fetch_latest() -> Result<Release, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let url = format!("{LATEST_RELEASE_PAGE}?t={stamp}");
    let response = agent()
        .get(&url)
        .header("Cache-Control", "no-cache")
        .call()
        .map_err(describe_http_error)?;
    let page_url = response.get_uri().to_string();
    let tag = page_url
        .rsplit_once("/tag/")
        .map(|(_, tag)| tag.trim_end_matches('/').to_string())
        .ok_or_else(|| t("Noch kein Release gefunden", "No release found yet").to_string())?;
    let version = semver::Version::parse(tag.trim_start_matches('v'))
        .map_err(|_| format!("{}: {tag}", t("Unbekanntes Versionsformat", "Unknown version format")))?;
    let setup_url = format!("{RELEASE_DOWNLOAD_BASE}/{tag}/{SETUP_PREFIX}{version}.exe");
    Ok(Release { version, page_url, setup_url })
}

fn describe_http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(403 | 429) => t(
            "GitHub lehnt gerade zu viele Anfragen ab, später nochmal versuchen",
            "GitHub is refusing requests right now, try again later",
        )
        .to_string(),
        ureq::Error::StatusCode(404) => t("Datei bei GitHub nicht gefunden", "File not found on GitHub").to_string(),
        other => other.to_string(),
    }
}

fn download_and_run_setup(release: &Release) -> Result<(), String> {
    let url = &release.setup_url;
    let bytes = agent()
        .get(url)
        .call()
        .map_err(describe_http_error)?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;

    // Eine Windows-Exe beginnt immer mit "MZ", schützt vor Fehlerseiten statt Datei.
    if !bytes.starts_with(b"MZ") {
        return Err(t("Heruntergeladene Datei ist kein gültiger Installer", "Downloaded file is not a valid installer").to_string());
    }

    let setup = std::env::temp_dir().join(format!("{SETUP_PREFIX}{}.exe", release.version));
    std::fs::write(&setup, &bytes).map_err(|e| e.to_string())?;

    // Ohne Rückfragen, aber mit Fortschrittsfenster. Der Installer schließt eine noch laufende
    // Lärmampel selbst und startet am Ende die neue Version.
    log!("Update auf v{}: starte Installer {}", release.version, setup.display());
    std::process::Command::new(&setup)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
        .spawn()
        .map_err(|e| format!("{}: {e}", t("Installer lässt sich nicht starten", "Cannot start installer")))?;
    Ok(())
}

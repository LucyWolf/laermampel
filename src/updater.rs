//! Update-Prüfung und Selbst-Update über die GitHub-Releases.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use serde::Deserialize;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/LucyWolf/laermampel/releases/latest";
const ASSET_NAME: &str = "laermampel.exe";
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Release {
    pub version: semver::Version,
    pub page_url: String,
    download_url: Option<String>,
}

impl Release {
    /// Selbst ersetzen geht nur unter Windows, woanders gibt es nur den Link.
    pub fn installable(&self) -> bool {
        cfg!(windows) && self.download_url.is_some()
    }
}

#[derive(Clone)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Installing(Release),
    /// Neue Version liegt bereit, wird beim nächsten Start aktiv.
    Installed(Release),
    Failed(String),
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
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
            Err(e) => Status::Failed(format!("Update-Prüfung fehlgeschlagen: {e}")),
        });
    }

    pub fn install(&self, ctx: &egui::Context, release: Release) {
        if self.is_busy() {
            return;
        }
        self.run(ctx, Status::Installing(release.clone()), move || match download_and_replace(&release) {
            Ok(()) => Status::Installed(release),
            Err(e) => Status::Failed(format!("Update fehlgeschlagen: {e}")),
        });
    }

    pub fn fail(&self, message: String) {
        if let Ok(mut s) = self.status.lock() {
            *s = Status::Failed(message);
        }
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

fn fetch_latest() -> Result<Release, String> {
    let release: GithubRelease = agent()
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;

    let version = semver::Version::parse(release.tag_name.trim_start_matches('v'))
        .map_err(|_| format!("Unbekanntes Versionsformat: {}", release.tag_name))?;
    let download_url = release
        .assets
        .into_iter()
        .find(|a| a.name == ASSET_NAME)
        .map(|a| a.browser_download_url);

    Ok(Release { version, page_url: release.html_url, download_url })
}

fn download_and_replace(release: &Release) -> Result<(), String> {
    let url = release.download_url.as_deref().ok_or("Keine .exe im Release")?;
    let bytes = agent()
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;

    // Eine Windows-Exe beginnt immer mit "MZ", schützt vor Fehlerseiten statt Datei.
    if !bytes.starts_with(b"MZ") {
        return Err("Heruntergeladene Datei ist keine gültige .exe".to_string());
    }

    let tmp = std::env::temp_dir().join(format!("laermampel-{}.exe", release.version));
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    let result = self_replace::self_replace(&tmp).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Startet die (inzwischen ersetzte) eigene .exe neu.
pub fn restart() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

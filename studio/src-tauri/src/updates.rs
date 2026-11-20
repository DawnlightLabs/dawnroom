//! Updates through Tauri's updater: Studio checks the release's latest.json once a day (the window
//! decides when, and whether updates are on), downloads the signed installer in the background and
//! keeps it. "Restart to update" installs it and reopens Studio; otherwise Quit from the tray
//! installs it without reopening. The installer's own hook stops the engine first, and it replaces
//! Studio, the engine (`dawnroom`) and the registry together; the data folder is never touched.

use std::sync::Mutex;

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The downloaded update, waiting to be installed.
#[derive(Default)]
pub struct Pending(Mutex<Option<(Update, Vec<u8>)>>);

/// Tests point Studio at a fake release with `DAWNROOM_UPDATE_JSON` (a latest.json URL). Only a
/// loopback https address is taken, and only then is its self-signed certificate accepted; the
/// update's signature is checked either way.
fn test_endpoint() -> Option<Url> {
    let url = Url::parse(&std::env::var("DAWNROOM_UPDATE_JSON").ok()?).ok()?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    (url.scheme() == "https" && loopback).then_some(url)
}

/// Checks for a newer version and downloads it. Returns its version when one is ready.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<String>, String> {
    if let Some(v) = update_ready(app.clone()) {
        return Ok(Some(v));
    }
    let mut builder = app.updater_builder();
    if let Some(url) = test_endpoint() {
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?.configure_client(|c| c.danger_accept_invalid_certs(true));
    }
    let updater = builder.build().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else { return Ok(None) };
    // A .deb install can only take a .deb. When the release lists no `linux-x86_64-deb`, the plugin
    // falls back to the AppImage, which it would download and then refuse to install: skip it.
    if wrong_package(&update) {
        return Ok(None);
    }
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    let version = update.version.clone();
    *app.state::<Pending>().0.lock().unwrap() = Some((update, bytes));
    Ok(Some(version))
}

/// The version that's downloaded and waiting, if any.
#[tauri::command]
pub fn update_ready(app: AppHandle) -> Option<String> {
    app.state::<Pending>().0.lock().unwrap().as_ref().map(|(u, _)| u.version.clone())
}

/// "Restart to update": stops the engine, installs and reopens Studio.
#[tauri::command]
pub fn update_install(app: AppHandle) -> Result<(), String> {
    install(&app, true)
}

/// Installs the waiting update, if any. On Windows the installer takes over and Studio exits here.
pub fn install(app: &AppHandle, restart: bool) -> Result<(), String> {
    let Some((update, bytes)) = app.state::<Pending>().0.lock().unwrap().take() else { return Ok(()) };
    crate::stop_engine();
    update.restart_after_install(restart).install(bytes).map_err(|e| e.to_string())?;
    // Linux (AppImage): the file was replaced in place; reopen it or just leave.
    if restart {
        app.restart();
    }
    Ok(())
}

/// True when this is a .deb install and the update on offer is an AppImage.
fn wrong_package(update: &Update) -> bool {
    use tauri::utils::{config::BundleType, platform::bundle_type};
    bundle_type() == Some(BundleType::Deb) && update.download_url.path().to_ascii_lowercase().ends_with(".appimage")
}

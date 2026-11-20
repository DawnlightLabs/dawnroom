//! The Studio shell. All product logic lives in the engine; this only opens the window and,
//! when the engine isn't answering, starts `dawnroom serve` the way `dawnroom mcp` does:
//! detached, no console window, stdio to a log file. It also owns the two things only the
//! desktop app has: its WebView cache and the folder picker (the dialog plugin).

mod signin;
mod tray;
mod updates;

use std::path::PathBuf;
use std::process::{Command, Stdio};

use tauri::{Emitter, Manager};

/// Where the engine CLI lives. `DAWNROOM_ENGINE` wins (dev, custom installs); then a
/// `dawnroom` binary next to Studio (installed layout); then, in debug builds, the repo build.
fn engine_exe() -> Option<PathBuf> {
    let name = if cfg!(windows) { "dawnroom.exe" } else { "dawnroom" };
    if let Some(p) = std::env::var_os("DAWNROOM_ENGINE").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    if beside.is_file() {
        return Some(beside);
    }
    #[cfg(debug_assertions)]
    {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../engine/target/debug").join(name);
        if repo.is_file() {
            return repo.canonicalize().ok();
        }
    }
    None
}

/// The engine path Studio would start, for Settings and error messages.
#[tauri::command]
fn engine_path() -> Option<String> {
    engine_exe().map(|p| p.to_string_lossy().trim_start_matches(r"\?\").to_string())
}

/// Starts `dawnroom --port <port> serve` detached. The caller polls /api/health afterwards.
#[tauri::command]
fn start_engine(app: tauri::AppHandle, port: u16) -> Result<String, String> {
    // Quit stops the engine; the window's polling must not start a new one in that moment.
    if QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("Dawnroom is quitting.".into());
    }
    let exe = engine_exe().ok_or("Couldn't find the engine (dawnroom). Set DAWNROOM_ENGINE to its path and reopen Studio.")?;
    let logs = app.path().app_log_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&logs).map_err(|e| format!("Couldn't create {} ({e}).", logs.display()))?;
    let log_path = logs.join("engine.log");
    // Same rule as the engine's own logs (engine/src/logs.rs): 10 MB, three old ones kept.
    rotate_log(&log_path, 10_000_000, 3);
    let open = || std::fs::OpenOptions::new().create(true).append(true).open(&log_path);
    let (out, err) = (open().map_err(|e| e.to_string())?, open().map_err(|e| e.to_string())?);
    let mut cmd = Command::new(&exe);
    cmd.args(["--port", &port.to_string(), "serve"]).stdin(Stdio::null()).stdout(out).stderr(err);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map_err(|e| format!("Couldn't start the engine at {} ({e}).", exe.display()))?;
    Ok(log_path.to_string_lossy().into_owned())
}

/// Set when Quit starts: from then on Studio never starts an engine.
pub(crate) static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `dawnroom stop`: the engine and sd-server stop (a download keeps its .part). Waits at most 15 s.
pub(crate) fn stop_engine() {
    let Some(exe) = engine_exe() else { return };
    let mut cmd = Command::new(exe);
    cmd.arg("stop").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(mut child) = cmd.spawn() else { return };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
}

fn rotate_log(path: &std::path::Path, max: u64, keep: u32) {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) <= max {
        return;
    }
    let numbered = |n: u32| PathBuf::from(format!("{}.{n}", path.display()));
    let _ = std::fs::remove_file(numbered(keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(numbered(n), numbered(n + 1));
    }
    let _ = std::fs::rename(path, numbered(1));
}

/// WebView2's rebuildable caches (Windows). Not localStorage, which holds Studio's settings.
fn web_cache_dirs() -> Vec<PathBuf> {
    let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return vec![] };
    let profile = base.join(IDENTIFIER).join("EBWebView").join("Default");
    ["Cache", "Code Cache", "GPUCache"].iter().map(|d| profile.join(d)).collect()
}

const IDENTIFIER: &str = "com.dawnlightlabs.dawnroom";
/// Asks the next start to clear the web cache (WebView2 holds it open while Studio runs).
fn clear_marker() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join(IDENTIFIER).join("clear-web-cache"))
}

fn dir_bytes(dir: &std::path::Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten().map(|e| match e.metadata() {
        Ok(m) if m.is_dir() => dir_bytes(&e.path()),
        Ok(m) => m.len(),
        Err(_) => 0,
    }).sum()
}

/// Bytes of Studio's web cache, and whether clearing it is already scheduled.
#[tauri::command]
fn web_cache_info() -> (u64, bool) {
    (web_cache_dirs().iter().map(|d| dir_bytes(d)).sum(), clear_marker().is_some_and(|m| m.exists()))
}

/// Schedules the web cache to be cleared the next time Studio starts.
#[tauri::command]
fn clear_web_cache() -> Result<(), String> {
    let m = clear_marker().ok_or("No app data folder.")?;
    std::fs::write(&m, b"").map_err(|e| e.to_string())
}

pub fn run() {
    // Before the window (and WebView2) exists: clear the web cache if asked to last time.
    if let Some(m) = clear_marker().filter(|m| m.exists()) {
        for d in web_cache_dirs() {
            let _ = std::fs::remove_dir_all(d);
        }
        let _ = std::fs::remove_file(m);
    }
    tauri::Builder::default()
        // First, so a second launch (Start menu, sign-in) brings this window back instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updates::Pending::default())
        .setup(|app| {
            tray::build(app.handle())?;
            // The window starts hidden (tauri.conf.json); started at sign-in it stays in the tray.
            if !std::env::args().any(|a| a == "--hidden") {
                tray::show(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing hides to the tray; the window decides how (the first time it says so once).
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit("close-requested", ());
            }
        })
        .invoke_handler(tauri::generate_handler![
            engine_path,
            start_engine,
            web_cache_info,
            clear_web_cache,
            tray::set_tray,
            tray::hide_window,
            tray::quit_app,
            signin::sign_in_get,
            signin::sign_in_set,
            updates::update_check,
            updates::update_ready,
            updates::update_install
        ])
        .run(tauri::generate_context!())
        .expect("error while running Studio");
}

//! The tray icon: Studio keeps running there when its window is closed, so downloads and agents
//! keep working. The menu's middle lines are what the window reports (`set_tray`): each download
//! and what the GPU is doing. Quit stops the engine (`dawnroom stop`), installs an update that's
//! waiting, and closes Studio.

use serde::Deserialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

const TRAY_ID: &str = "dawnroom";

/// What the window last reported: one line per download ("Z-Image Turbo · 62%") and the GPU's state.
#[derive(Deserialize, Default)]
pub struct TrayStatus {
    #[serde(default)]
    pub downloads: Vec<String>,
    #[serde(default)]
    pub gpu: Option<String>,
}

fn menu<R: Runtime>(app: &AppHandle<R>, status: &TrayStatus) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(app, "open", "Open Dawnroom", true, None::<&str>)?)?;
    let lines: Vec<&String> = status.downloads.iter().chain(status.gpu.iter()).collect();
    if !lines.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
        for (i, line) in lines.into_iter().enumerate() {
            // Information, not actions: shown greyed, like Windows' own status lines.
            menu.append(&MenuItem::with_id(app, format!("line-{i}"), line, false, None::<&str>)?)?;
        }
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit Dawnroom", true, None::<&str>)?)?;
    Ok(menu)
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .tooltip("Dawnroom")
        .menu(&menu(app, &TrayStatus::default())?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "open" => show(app),
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Brings the window back (tray click, "Open Dawnroom", or Dawnroom started a second time).
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Hides the window; Studio stays in the tray.
#[tauri::command]
pub fn hide_window(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

/// The window's report for the tray menu and tooltip.
#[tauri::command]
pub fn set_tray(app: AppHandle, status: TrayStatus) -> Result<(), String> {
    let tray = app.tray_by_id(TRAY_ID).ok_or("No tray icon.")?;
    tray.set_menu(Some(menu(&app, &status).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    let tip = match (status.downloads.first(), &status.gpu) {
        (Some(d), _) => format!("Dawnroom · {d}"),
        (None, Some(g)) => format!("Dawnroom · {g}"),
        _ => "Dawnroom".into(),
    };
    tray.set_tooltip(Some(tip)).map_err(|e| e.to_string())
}

/// Quit Dawnroom: hide at once, stop the engine (it frees the GPU; a download keeps its .part), exit.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    quit(&app);
}

fn quit(app: &AppHandle) {
    crate::QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    let app = app.clone();
    std::thread::spawn(move || {
        crate::stop_engine();
        // A downloaded update installs now, without reopening Studio (Windows exits inside).
        if let Err(e) = crate::updates::install(&app, false) {
            eprintln!("update not installed: {e}");
        }
        app.exit(0);
    });
}

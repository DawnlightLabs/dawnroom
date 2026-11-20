//! Start at sign-in (Settings and first run; off by default). Windows: the HKCU Run value
//! "Dawnroom", which the uninstaller also removes. Linux: an XDG autostart entry. Studio starts
//! with `--hidden`, so it opens in the tray, not as a window.

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE: &str = "Dawnroom";

fn command() -> Result<String, String> {
    // An AppImage runs from a temporary mount; its own path is in $APPIMAGE.
    let exe = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).map_or_else(std::env::current_exe, Ok).map_err(|e| e.to_string())?;
    Ok(format!("\"{}\" --hidden", exe.display()))
}

#[cfg(windows)]
#[tauri::command]
pub fn sign_in_get() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY).and_then(|k| k.get_value::<String, _>(VALUE)).is_ok()
}

#[cfg(windows)]
#[tauri::command]
pub fn sign_in_set(on: bool) -> Result<bool, String> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY).map_err(|e| e.to_string())?;
    if on {
        key.set_value(VALUE, &command()?).map_err(|e| e.to_string())?;
    } else if let Err(e) = key.delete_value(VALUE) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.to_string());
        }
    }
    Ok(sign_in_get())
}

#[cfg(not(windows))]
fn desktop_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("autostart").join("dawnroom.desktop"))
}

#[cfg(not(windows))]
#[tauri::command]
pub fn sign_in_get() -> bool {
    desktop_file().is_some_and(|f| f.exists())
}

#[cfg(not(windows))]
#[tauri::command]
pub fn sign_in_set(on: bool) -> Result<bool, String> {
    let f = desktop_file().ok_or("No home folder.")?;
    if on {
        std::fs::create_dir_all(f.parent().unwrap()).map_err(|e| e.to_string())?;
        let body = format!("[Desktop Entry]\nType=Application\nName=Dawnroom\nExec={}\nX-GNOME-Autostart-enabled=true\n", command()?);
        std::fs::write(&f, body).map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(&f);
    }
    Ok(sign_in_get())
}

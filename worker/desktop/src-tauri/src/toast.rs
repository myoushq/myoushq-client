//! Windows shows a toast only for an app it knows: one whose
//! AppUserModelID is registered. The notification plugin gives every toast
//! the app's identifier as its AppUserModelID, and the installer's
//! Start-menu shortcut registers it, but an app run from an unzipped folder
//! has no shortcut, and Windows drops its toasts without a word. So at
//! start the app registers the id itself, the way Microsoft documents for
//! an unpackaged app without a shortcut: a key under
//! `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\<id>` with the name
//! and the icon the toasts show. Elsewhere this does nothing.

#[cfg(any(windows, test))]
use std::fs;
#[cfg(any(windows, test))]
use std::io;
#[cfg(any(windows, test))]
use std::path::{Path, PathBuf};

/// The app's identifier (`identifier` in tauri.conf.json), which the
/// notification plugin gives every toast as its AppUserModelID.
#[cfg_attr(not(windows), allow(dead_code))]
pub const APP_ID: &str = "com.myoushq.desktop";

/// The picture a toast shows, written out for Windows to read.
#[cfg(any(windows, test))]
const ICON: &[u8] = include_bytes!("../icons/128x128.png");

/// Writes the toast icon into `dir` unless it is there already, byte for
/// byte. Returns its path and whether it wrote.
#[cfg(any(windows, test))]
fn ensure_icon(dir: &Path) -> io::Result<(PathBuf, bool)> {
    let path = dir.join("toast-icon.png");
    if fs::read(&path).is_ok_and(|b| b == ICON) {
        return Ok((path, false));
    }
    crate::paths::ensure(dir)?;
    fs::write(&path, ICON)?;
    Ok((path, true))
}

/// Gives this process the app's AppUserModelID, so its window and its
/// toasts belong to one app on the taskbar. Called first thing, before
/// the window exists.
pub fn set_process_id() {
    #[cfg(windows)]
    {
        let id = windows::core::HSTRING::from(APP_ID);
        // SAFETY: a plain call with a valid, NUL-terminated wide string.
        if let Err(e) =
            unsafe { windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(&id) }
        {
            crate::files::append_app_log(&format!("toast: could not set the app id: {e}"));
        }
    }
}

/// Registers the app for toasts (Windows; elsewhere nothing). Writes only
/// what is missing or different, logs when it wrote, and never fails.
pub fn register() {
    #[cfg(windows)]
    match register_windows(&crate::paths::app_data()) {
        Ok(true) => {
            crate::files::append_app_log(&format!("toast: registered {APP_ID} for notifications"))
        }
        Ok(false) => {}
        Err(e) => crate::files::append_app_log(&format!(
            "toast: could not register {APP_ID} for notifications: {e}"
        )),
    }
}

/// Returns whether it changed the registry.
#[cfg(windows)]
fn register_windows(app_data: &Path) -> io::Result<bool> {
    use std::ffi::OsString;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let (icon, _) = ensure_icon(app_data)?;
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
    let mut wrote = false;
    if key.get_value::<String, _>("DisplayName").ok().as_deref() != Some("myous") {
        key.set_value("DisplayName", &"myous")?;
        wrote = true;
    }
    let icon = OsString::from(icon);
    if key.get_value::<OsString, _>("IconUri").ok().as_ref() != Some(&icon) {
        key.set_value("IconUri", &icon)?;
        wrote = true;
    }
    Ok(wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_is_the_identifier() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], APP_ID);
    }

    #[test]
    fn icon_is_written_once_and_mended() {
        let dir = std::env::temp_dir().join(format!("myous-toast-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (path, wrote) = ensure_icon(&dir).unwrap();
        assert!(wrote);
        assert_eq!(path, dir.join("toast-icon.png"));
        assert_eq!(fs::read(&path).unwrap(), ICON);
        assert!(ICON.starts_with(b"\x89PNG"));
        assert!(!ensure_icon(&dir).unwrap().1);
        fs::write(&path, b"not a png").unwrap();
        assert!(ensure_icon(&dir).unwrap().1);
        assert_eq!(fs::read(&path).unwrap(), ICON);
        let _ = fs::remove_dir_all(&dir);
    }
}

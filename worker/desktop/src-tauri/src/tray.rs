//! The tray icon and its menu, built from what the window sends: the
//! window owns the state, the tray only shows it and reports clicks.

use serde::Deserialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

/// One menu entry as the window describes it.
#[derive(Deserialize, Clone)]
pub struct Item {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub separator: bool,
    /// A shortcut ("CmdOrCtrl+,"), for a Mac's app menu; the tray's have none.
    #[serde(default)]
    pub accelerator: String,
    #[serde(default)]
    pub children: Vec<Item>,
}

fn yes() -> bool {
    true
}

pub fn build<R: Runtime>(app: &AppHandle<R>, items: &[Item]) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    for it in items {
        append(app, &menu, it)?;
    }
    Ok(menu)
}

fn append<R: Runtime>(app: &AppHandle<R>, menu: &Menu<R>, it: &Item) -> tauri::Result<()> {
    if it.separator {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    } else if !it.children.is_empty() {
        let sub = Submenu::new(app, &it.label, it.enabled)?;
        for c in &it.children {
            append_sub(app, &sub, c)?;
        }
        menu.append(&sub)?;
    } else {
        menu.append(&item(app, it)?)?;
    }
    Ok(())
}

fn append_sub<R: Runtime>(app: &AppHandle<R>, menu: &Submenu<R>, it: &Item) -> tauri::Result<()> {
    if it.separator {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    } else if !it.children.is_empty() {
        let sub = Submenu::new(app, &it.label, it.enabled)?;
        for c in &it.children {
            append_sub(app, &sub, c)?;
        }
        menu.append(&sub)?;
    } else {
        menu.append(&item(app, it)?)?;
    }
    Ok(())
}

fn item<R: Runtime>(app: &AppHandle<R>, it: &Item) -> tauri::Result<MenuItem<R>> {
    let accel = (!it.accelerator.is_empty()).then_some(it.accelerator.as_str());
    MenuItem::with_id(app, &it.id, &it.label, it.enabled, accel)
}

/// A Mac's menu bar while the window is in front: the app menu (About,
/// the window's items, Hide, Quit), Edit (so copy and paste work in the
/// fields) and Window. The window's items are its ⋯ menu elsewhere; a
/// click on one reaches the tray's handler, which Tauri calls for every
/// menu, and the window runs it.
#[cfg(target_os = "macos")]
pub fn app_menu<R: Runtime>(app: &AppHandle<R>, items: &[Item]) -> tauri::Result<Menu<R>> {
    let name = Submenu::new(app, "myous", true)?;
    name.append(&PredefinedMenuItem::about(app, Some("About myous"), None)?)?;
    name.append(&PredefinedMenuItem::separator(app)?)?;
    for it in items {
        append_sub(app, &name, it)?;
    }
    name.append(&PredefinedMenuItem::separator(app)?)?;
    name.append(&PredefinedMenuItem::hide(app, Some("Hide myous"))?)?;
    name.append(&PredefinedMenuItem::hide_others(app, None)?)?;
    name.append(&PredefinedMenuItem::show_all(app, None)?)?;
    name.append(&PredefinedMenuItem::separator(app)?)?;
    name.append(&PredefinedMenuItem::quit(app, Some("Quit myous"))?)?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&name, &edit, &window])
}

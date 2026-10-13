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
        menu.append(&MenuItem::with_id(
            app,
            &it.id,
            &it.label,
            it.enabled,
            None::<&str>,
        )?)?;
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
        menu.append(&MenuItem::with_id(
            app,
            &it.id,
            &it.label,
            it.enabled,
            None::<&str>,
        )?)?;
    }
    Ok(())
}

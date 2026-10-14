//! Open at login on a Mac: the app as a login item through SMAppService
//! (`SMAppService.mainAppService`), as myous for Mac does, listed under
//! System Settings › General › Login Items › Open at Login. The autostart
//! plugin offers only a LaunchAgent or AppleScript, so this calls the
//! `auto-launch` crate under the plugin directly, in its SMAppService mode.
//!
//! SMAppService needs an app bundle; a program run from target/ keeps the
//! plugin's LaunchAgent (lib.rs decides, with `usernotify::in_app_bundle`).

use auto_launch::{AutoLaunch, AutoLaunchBuilder, MacOSLaunchMode};

fn service() -> Result<AutoLaunch, String> {
    AutoLaunchBuilder::new()
        .set_macos_launch_mode(MacOSLaunchMode::SMAppService)
        .build()
        .map_err(|e| e.to_string())
}

/// Whether the app opens at login: registered and enabled (not waiting
/// for the owner's approval in System Settings).
pub fn is_enabled() -> bool {
    service().and_then(|s| s.is_enabled().map_err(|e| e.to_string())) == Ok(true)
}

/// Registers or unregisters the app. Registered but not enabled means
/// the owner turned myous off under Login Items, and only they can turn
/// it back on there.
pub fn set(on: bool) -> Result<(), String> {
    let s = service()?;
    if on == is_enabled() {
        return Ok(());
    }
    if on {
        s.enable().map_err(|e| e.to_string())?;
        if !is_enabled() {
            return Err("macOS asks for your approval: System Settings › General › Login Items, turn on myous".into());
        }
        Ok(())
    } else {
        s.disable().map_err(|e| e.to_string())
    }
}

/// An upgrade from a version that opened at login through the plugin's
/// LaunchAgent (`~/Library/LaunchAgents/myous.plist`): the plist goes, and
/// the app registers with SMAppService instead, so the owner never has
/// two entries. `agent_on` says whether the plist is there; `remove_agent`
/// removes it. Returns what it did, for the log.
pub fn migrate(
    agent_on: bool,
    remove_agent: impl FnOnce() -> Result<(), String>,
) -> Option<String> {
    if !agent_on {
        return None;
    }
    let mut line = match remove_agent() {
        Ok(()) => "open at login: the LaunchAgent is removed".to_string(),
        Err(e) => {
            return Some(format!(
                "open at login: could not remove the LaunchAgent: {e}"
            ))
        }
    };
    match set(true) {
        Ok(()) => line.push_str("; a login item now (SMAppService)"),
        Err(e) => line.push_str(&format!("; the login item: {e}")),
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without the plugin's LaunchAgent nothing happens; when it can't be
    /// removed, the app doesn't register as well (that would make two).
    /// (Registering itself needs the app bundle, so it isn't run here.)
    #[test]
    fn migrate_only_from_a_launch_agent() {
        assert_eq!(migrate(false, || panic!("nothing to remove")), None);
        let line = migrate(true, || Err("permission denied".into())).unwrap();
        assert_eq!(
            line,
            "open at login: could not remove the LaunchAgent: permission denied"
        );
    }
}

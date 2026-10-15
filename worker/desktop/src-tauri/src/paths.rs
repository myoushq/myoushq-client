//! One worker's folder and the files in it. The layout is the Mac app's,
//! so a folder moves between the apps unchanged: `~/.myous-worker` for
//! the first worker (`%LOCALAPPDATA%\myous\worker` on Windows), and
//! `<default>-<n>` next to it for the others.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn user_home() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h);
    }
    if let Some(h) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(h);
    }
    PathBuf::from(".")
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix('~') {
        return user_home().join(rest.trim_start_matches(['/', '\\']));
    }
    PathBuf::from(p)
}

/// The first worker's folder: $MYOUS_WORKER_HOME, else the platform default.
pub fn default_home() -> PathBuf {
    if let Ok(env) = std::env::var("MYOUS_WORKER_HOME") {
        if !env.is_empty() {
            return expand_home(&env);
        }
    }
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("myous").join("worker");
            }
        }
    }
    user_home().join(".myous-worker")
}

/// The default home plus every `<default>-<n>` next to it that has an
/// app.json, in name order.
pub fn all_homes() -> Vec<PathBuf> {
    let def = default_home();
    let mut out = vec![def.clone()];
    let base = def
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut extra = Vec::new();
    if let Some(parent) = def.parent() {
        if let Ok(entries) = fs::read_dir(parent) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.starts_with(&format!("{base}-")) {
                    continue;
                }
                let home = parent.join(&name);
                if home.join("app.json").is_file() {
                    extra.push(home);
                }
            }
        }
    }
    extra.sort();
    out.extend(extra);
    out
}

/// The next free `<default>-<n>`.
pub fn new_home() -> Option<PathBuf> {
    let def = default_home();
    let s = def.to_string_lossy().to_string();
    (2..1000)
        .map(|n| PathBuf::from(format!("{s}-{n}")))
        .find(|p| !p.exists())
}

pub fn is_default(home: &Path) -> bool {
    clean(home) == clean(&default_home())
}

fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        out.push(c);
    }
    out
}

/// The compose project name: myous-worker for the first worker,
/// myous-worker-<n> for the others, whatever the mode. Tests override
/// the first worker's with MYOUS_COMPOSE_PROJECT so they never touch an
/// installed worker's containers.
pub fn project(home: &Path) -> String {
    if is_default(home) {
        if let Ok(env) = std::env::var("MYOUS_COMPOSE_PROJECT") {
            if !env.is_empty() {
                return env;
            }
        }
        return "myous-worker".into();
    }
    let base = home
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    match base.rfind('-') {
        Some(i) => format!("myous-worker-{}", &base[i + 1..]),
        None => "myous-worker".into(),
    }
}

pub fn status_file(home: &Path) -> PathBuf {
    home.join("worker.json")
}
pub fn config_file(home: &Path) -> PathBuf {
    home.join("app.json")
}
pub fn paused_file(home: &Path) -> PathBuf {
    home.join("worker.paused")
}
pub fn app_log(home: &Path) -> PathBuf {
    home.join("app.log")
}
pub fn commands_dir(home: &Path) -> PathBuf {
    home.join("commands")
}
/// The command that stops the worker (as SIGTERM does), which the app
/// sends on every system: on Windows it is the only graceful way.
pub const QUIT_COMMAND: &str = "quit";
pub fn quit_file(home: &Path) -> PathBuf {
    commands_dir(home).join(QUIT_COMMAND)
}
pub fn requests_dir(home: &Path) -> PathBuf {
    home.join("requests")
}
pub fn approvals_dir(home: &Path) -> PathBuf {
    home.join("approvals")
}
pub fn review_file(home: &Path) -> PathBuf {
    home.join("review.json")
}
pub fn worker_log(home: &Path) -> PathBuf {
    home.join("worker.log")
}

pub fn browser_json(home: &Path) -> PathBuf {
    home.join("browser.json")
}
pub fn browser_dir(home: &Path) -> PathBuf {
    home.join("browser")
}
pub fn browser_log(home: &Path) -> PathBuf {
    home.join("browser.log")
}

/// The app's own folder for what isn't one worker's: $MYOUS_APP_DATA (the
/// tests), else `%LOCALAPPDATA%\myous` on Windows (the first worker's
/// folder is in it), `~/Library/Application Support/myous` on a Mac, and
/// `$XDG_DATA_HOME/myous` (`~/.local/share/myous`) elsewhere.
pub fn app_data() -> PathBuf {
    if let Ok(d) = std::env::var("MYOUS_APP_DATA") {
        if !d.is_empty() {
            return expand_home(&d);
        }
    }
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.is_empty() {
                return PathBuf::from(local).join("myous");
            }
        }
    }
    if cfg!(target_os = "macos") {
        return user_home().join("Library/Application Support/myous");
    }
    match std::env::var("XDG_DATA_HOME") {
        Ok(x) if !x.is_empty() => PathBuf::from(x).join("myous"),
        _ => user_home().join(".local/share/myous"),
    }
}

pub fn compose_file(home: &Path) -> PathBuf {
    home.join("compose.yml")
}

/// Creates the folder, private to the user where the OS has modes.
pub fn ensure(home: &Path) -> io::Result<()> {
    fs::create_dir_all(home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(home, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Drops a command file for the worker (it removes the file once done):
/// new-code, unpair, stop-<id>, quit. Made in the worker folder and
/// renamed into commands/, where the worker takes every name for a
/// command; never written through a link there (files.rs).
pub fn send_command(home: &Path, name: &str) -> io::Result<()> {
    let dir = commands_dir(home);
    crate::files::dir_in(home, &dir)?;
    crate::files::write_atomic_via(home, &dir.join(name), b"")
}

pub fn is_paused(home: &Path) -> bool {
    paused_file(home).exists()
}

pub fn set_paused(home: &Path, on: bool) -> io::Result<()> {
    if on {
        ensure(home)?;
        crate::files::write_atomic(&paused_file(home), b"")
    } else {
        match fs::remove_file(paused_file(home)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_names() {
        assert_eq!(project(Path::new("/x/.myous-worker-3")), "myous-worker-3");
        assert_eq!(project(Path::new("/x/.myous-worker-12")), "myous-worker-12");
    }

    #[test]
    fn commands_and_pause() {
        let dir = std::env::temp_dir().join(format!("myous-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        send_command(&dir, "new-code").unwrap();
        assert!(dir.join("commands").join("new-code").is_file());
        assert!(!is_paused(&dir));
        set_paused(&dir, true).unwrap();
        assert!(is_paused(&dir));
        set_paused(&dir, false).unwrap();
        set_paused(&dir, false).unwrap();
        assert!(!is_paused(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }
}

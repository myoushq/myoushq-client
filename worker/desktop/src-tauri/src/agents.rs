//! The agents that live on this computer: each is a myous client state
//! directory (~/.myous, ~/.myous-<name>, or a folder the owner added) with
//! its own key. The app reads them through the client's own
//! `myous status --json`, run in that directory, and never touches their
//! files. It runs the client as the agent only for what the owner asks
//! (accepting a pairing code), with --added-by owner so the agent can tell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::engine::run_capture;

#[derive(Serialize, Clone)]
pub struct Agent {
    pub home: String,
    pub binary: String,
    pub status: Value,
    pub error: String,
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The candidate folders: ~/.myous and ~/.myous-* that hold a key (never
/// the app's own worker folders), plus `extra`. MYOUS_AGENT_HOMES
/// (path-list separated) replaces the search, for tests.
pub fn homes(extra: &[String]) -> Vec<String> {
    if let Some(env) = std::env::var_os("MYOUS_AGENT_HOMES") {
        return std::env::split_paths(&env)
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_string_lossy().to_string())
            .collect();
    }
    let user = user_home();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&user) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n != ".myous" && !n.starts_with(".myous-") {
                continue;
            }
            if n == ".myous-worker" || n.starts_with(".myous-worker-") {
                continue;
            }
            let home = user.join(&n);
            if home.join("key").is_file() {
                out.push(home.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    for h in extra {
        if Path::new(h).exists() && !out.contains(h) {
            out.push(h.clone());
        }
    }
    out
}

/// The client for a home: <home>/venv/bin/myous (Scripts on Windows),
/// <home>/bin/myous, or `myous` on PATH.
pub fn binary_for(home: &Path) -> String {
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        vec![
            home.join("venv").join("Scripts").join("myous.exe"),
            home.join("bin").join("myous.exe"),
        ]
    } else {
        vec![
            home.join("venv").join("bin").join("myous"),
            home.join("bin").join("myous"),
        ]
    };
    for c in candidates {
        if c.is_file() {
            return c.to_string_lossy().to_string();
        }
    }
    crate::engine::which("myous")
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Runs the client as this agent (MYOUS_HOME set, in its folder) and
/// returns the exit status and the combined output.
pub fn run_sync(home: &str, binary: &str, args: &[String], timeout: Duration) -> (i32, String) {
    let mut cmd = crate::engine::command(binary);
    cmd.args(args).current_dir(home).env("MYOUS_HOME", home);
    run_capture(cmd, timeout)
}

fn read_one(home: &str) -> Agent {
    let binary = binary_for(Path::new(home));
    let mut a = Agent {
        home: home.to_string(),
        binary: binary.clone(),
        status: Value::Null,
        error: String::new(),
    };
    if binary.is_empty() {
        a.error = "no myous client found for this folder".into();
        return a;
    }
    let (status, text) = run_sync(
        home,
        &binary,
        &["status".into(), "--json".into()],
        Duration::from_secs(20),
    );
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if status == 0 && v.is_object() => a.status = v,
        _ => {
            let last = text
                .trim()
                .lines()
                .last()
                .unwrap_or("no output")
                .to_string();
            a.error = format!("the client didn't answer ({last})");
        }
    }
    a
}

/// Runs `status --json` in every home (in parallel) and returns one
/// Agent per home, in the given order.
pub fn read(homes: &[String]) -> Vec<Agent> {
    let handles: Vec<_> = homes
        .iter()
        .cloned()
        .map(|h| std::thread::spawn(move || read_one(&h)))
        .collect();
    handles
        .into_iter()
        .map(|h| {
            h.join().unwrap_or_else(|_| Agent {
                home: String::new(),
                binary: String::new(),
                status: Value::Null,
                error: "crashed".into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homes_from_env_and_missing_binary() {
        let dir = std::env::temp_dir().join(format!("myous-agents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("MYOUS_AGENT_HOMES", dir.to_string_lossy().to_string());
        let h = homes(&[]);
        assert_eq!(h.len(), 1);
        std::env::remove_var("MYOUS_AGENT_HOMES");
        #[cfg(unix)]
        {
            std::fs::write(dir.join("status"), "#!/bin/sh\necho '{\"alias\":\"A\"}'\n").unwrap();
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::write(
                dir.join("bin").join("myous"),
                "#!/bin/sh\necho '{\"alias\":\"A\",\"identity\":\"npub1x\"}'\n",
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.join("bin").join("myous"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            let a = read(&h);
            assert_eq!(a[0].status["alias"], "A", "{}", a[0].error);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

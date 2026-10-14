//! A worker without a container: the Go worker the app carries inside
//! itself (`myous-worker`, built from the client's go/cmd/myous), run on
//! this computer as the owner, as myous for Mac's direct mode does. For
//! computers that can't run containers (Windows without WSL 2) and for
//! owners who choose it. The review rules are built into the worker (no
//! `--review`): the level in review.json, worker.paused, approvals/.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use crate::engine::{hide, run_capture};
use crate::paths;

/// The bundled worker's file name. Never `myous`: on Windows that is the
/// app itself (myous.exe), and a bare `myous` would start the app again.
pub const WORKER_FILE: &str = if cfg!(windows) {
    "myous-worker.exe"
} else {
    "myous-worker"
};

/// The worker program: $MYOUS_WORKER_BIN (a development override, such as
/// a local Go build), else the one bundled with the app. Tauri puts its
/// bundled programs (`externalBin`) next to the app's own: `/usr/bin` in
/// the deb, `usr/bin` inside the AppImage, the install folder on Windows,
/// `target/<profile>` from source. True with the path when overridden.
pub fn worker_bin() -> Result<(PathBuf, bool), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    find_worker(&exe, std::env::var_os("MYOUS_WORKER_BIN"))
}

fn find_worker(exe: &Path, overridden: Option<OsString>) -> Result<(PathBuf, bool), String> {
    if let Some(p) = overridden.filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        // A path, never a name looked up on PATH or in the current folder.
        return match p.canonicalize() {
            Ok(full) if full.is_file() => Ok((full, true)),
            _ => Err(format!(
                "MYOUS_WORKER_BIN is {} but there is no program there",
                p.display()
            )),
        };
    }
    let bundled = exe
        .parent()
        .map(|d| d.join(WORKER_FILE))
        .ok_or("the app's own folder is unknown")?;
    if bundled.is_file() {
        Ok((bundled, false))
    } else {
        Err(format!(
            "this copy of myous has no worker ({} is missing); reinstall myous",
            bundled.display()
        ))
    }
}

/// The worker's command line, the program first. The name is passed only
/// when it changed (passing it registers again). `notes` are lines for
/// the worker's help (`--note`); its browser isn't one: the worker reads
/// that from browser.json itself.
pub fn worker_args(
    home: &Path,
    bin: &Path,
    name: &str,
    description: &str,
    notes: &[String],
) -> Vec<String> {
    let current = std::fs::read_to_string(home.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("alias").and_then(|a| a.as_str()).map(String::from))
        .unwrap_or_default();
    let mut args = vec![bin.to_string_lossy().to_string(), "worker".into()];
    if !name.is_empty() && name != current {
        args.extend(["--alias".into(), name.to_string()]);
    }
    if !description.is_empty() {
        args.extend(["--description".into(), description.to_string()]);
    }
    for n in notes {
        args.extend(["--note".into(), n.clone()]);
    }
    args.extend([
        "--work".into(),
        home.join("work").to_string_lossy().to_string(),
    ]);
    args
}

fn children() -> &'static Mutex<HashMap<String, Child>> {
    static CHILDREN: std::sync::OnceLock<Mutex<HashMap<String, Child>>> =
        std::sync::OnceLock::new();
    CHILDREN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Starts the worker, its output appended to worker.log. `exited` is
/// called (from a thread) with the exit status when it ends.
pub fn start(
    home: &str,
    args: &[String],
    exited: impl FnOnce(i32) + Send + 'static,
) -> Result<u32, String> {
    let dir = PathBuf::from(home);
    paths::ensure(&dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir.join("work")).map_err(|e| e.to_string())?;
    if children().lock().unwrap().contains_key(home) {
        return Err("this worker is already running".into());
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::worker_log(&dir))
        .map_err(|e| e.to_string())?;
    let mut cmd = crate::engine::command(&args[0]);
    cmd.args(&args[1..])
        .current_dir(&dir)
        .env("MYOUS_HOME", &dir)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    hide(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", args[0]))?;
    let pid = child.id();
    children().lock().unwrap().insert(home.to_string(), child);
    let key = home.to_string();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        let mut map = children().lock().unwrap();
        let Some(c) = map.get_mut(&key) else { return };
        match c.try_wait() {
            Ok(None) => continue,
            Ok(Some(st)) => {
                map.remove(&key);
                drop(map);
                exited(st.code().unwrap_or(-1));
                return;
            }
            Err(_) => {
                map.remove(&key);
                drop(map);
                exited(-1);
                return;
            }
        }
    });
    Ok(pid)
}

/// How long a worker sent SIGTERM has to end its command, answer the
/// agent and exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Ends a process and what it started. Elsewhere than Windows: SIGTERM,
/// on which the worker ends the command it runs (its process group),
/// answers that request and exits; SIGKILL if it is still there after
/// STOP_GRACE. On Windows, where a worker without a console can't be
/// asked: taskkill /T, which ends the worker and the command it runs.
fn end(pid: u32) {
    let id = pid.to_string();
    let mut cmd = crate::engine::command(if cfg!(windows) { "taskkill" } else { "kill" });
    if cfg!(windows) {
        cmd.args(["/T", "/F", "/PID", &id]);
    } else {
        cmd.args(["-TERM", &id]);
    }
    let _ = run_capture(cmd, Duration::from_secs(15));
    if !cfg!(windows) {
        std::thread::spawn(move || {
            let begun = std::time::Instant::now();
            while begun.elapsed() < STOP_GRACE {
                std::thread::sleep(Duration::from_millis(500));
                if !signal(pid, "-0") {
                    return; // gone (an exited child of ours is reaped by start's thread)
                }
            }
            signal(pid, "-KILL");
        });
    }
}

/// `kill <sig> <pid>`: true if it succeeded (for -0: the process exists).
fn signal(pid: u32, sig: &str) -> bool {
    let mut cmd = crate::engine::command("kill");
    cmd.args([sig, &pid.to_string()]);
    run_capture(cmd, Duration::from_secs(15)).0 == 0
}

/// Stops the worker this app started for `home`, else the process `pid`
/// (from worker.json, a worker started before the app was). True if it
/// was this app's: its exit is then reported by `start`'s callback.
pub fn stop(home: &str, pid: u32) -> bool {
    let ours = children().lock().unwrap().get(home).map(|c| c.id());
    match ours {
        Some(p) => {
            end(p);
            true
        }
        None => {
            if pid > 0 {
                end(pid);
            }
            false
        }
    }
}

/// Stops every worker this app started (when it quits).
pub fn stop_all() {
    let pids: Vec<u32> = children()
        .lock()
        .unwrap()
        .values()
        .map(|c| c.id())
        .collect();
    for p in pids {
        end(p);
    }
}

#[cfg(test)]
#[allow(dead_code)] // the tests that use it don't run on Windows
fn running(home: &str) -> bool {
    children().lock().unwrap().contains_key(home)
}

/// The end of worker.log.
pub fn log_tail(home: &str, max: usize) -> String {
    let bytes = std::fs::read(paths::worker_log(Path::new(home))).unwrap_or_default();
    let start = bytes.len().saturating_sub(max);
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("myous-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_bundled_worker_is_next_to_the_app() {
        let dir = temp("bundled");
        let app = dir.join(if cfg!(windows) { "myous.exe" } else { "myous" });
        std::fs::write(&app, "").unwrap();
        // Not there: says so, and never falls back to a `myous` (the app on Windows).
        let e = find_worker(&app, None).unwrap_err();
        assert!(e.contains(WORKER_FILE), "{e}");
        std::fs::write(dir.join(WORKER_FILE), "").unwrap();
        assert_eq!(
            find_worker(&app, None).unwrap(),
            (dir.join(WORKER_FILE), false)
        );
        assert_eq!(
            find_worker(&app, Some(OsString::new())).unwrap().0,
            dir.join(WORKER_FILE)
        );
        // The development override wins, as a full path; a wrong one is an error.
        let other = dir.join("go-build");
        std::fs::write(&other, "").unwrap();
        let (p, overridden) = find_worker(&app, Some(other.clone().into())).unwrap();
        assert!(overridden);
        assert_eq!(p, other.canonicalize().unwrap());
        assert!(find_worker(&app, Some(dir.join("nothing").into()))
            .unwrap_err()
            .contains("MYOUS_WORKER_BIN"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn worker_command_line() {
        let home = temp("direct");
        let bin = Path::new("/opt/myous/myous-worker");
        let args = worker_args(&home, bin, "Max's PC", "My own computer", &[]);
        assert_eq!(&args[..2], &["/opt/myous/myous-worker", "worker"]);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--alias" && w[1] == "Max's PC"));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--description" && w[1] == "My own computer"));
        // The review rules are the worker's own: no hook.
        assert!(!args.contains(&"--review".to_string()));
        assert_eq!(
            &args[args.len() - 2..],
            &[
                "--work".to_string(),
                home.join("work").to_string_lossy().to_string()
            ]
        );
        // Registered under that name already: not passed again.
        std::fs::write(home.join("settings.json"), r#"{"alias": "Max's PC"}"#).unwrap();
        assert!(!worker_args(&home, bin, "Max's PC", "", &[]).contains(&"--alias".to_string()));
        // Lines for the help, one --note each.
        let notes = vec![
            "Printer: the one in the hall.".to_string(),
            "Ask before deleting anything.".to_string(),
        ];
        let args = worker_args(&home, bin, "Max's PC", "", &notes);
        assert_eq!(args.iter().filter(|a| *a == "--note").count(), 2);
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--note" && w[1] == notes[1]));
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// The worker the app carries is the same release as the app.
    #[test]
    fn the_worker_is_this_release() {
        let go = include_str!("../../../../go/notices.go");
        assert!(
            go.contains(&format!(
                "const Version = \"{}\"",
                env!("CARGO_PKG_VERSION")
            )),
            "go/notices.go is not {}",
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn start_and_stop() {
        let home = std::env::temp_dir().join(format!("myous-direct-run-{}", std::process::id()));
        let h = home.to_string_lossy().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        let args: Vec<String> = if cfg!(windows) {
            vec![
                "powershell.exe",
                "-NoProfile",
                "-Command",
                "Write-Output started; Start-Sleep 30",
            ]
        } else {
            vec!["sh", "-c", "echo started; sleep 30"]
        }
        .into_iter()
        .map(String::from)
        .collect();
        start(&h, &args, move |st| tx.send(st).unwrap()).unwrap();
        assert!(running(&h));
        assert!(start(&h, &args, |_| {}).is_err());
        // Until the command has written its line (PowerShell takes seconds on a cold runner).
        let begun = std::time::Instant::now();
        while !log_tail(&h, 1000).contains("started") && begun.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(stop(&h, 0));
        assert_ne!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), 0);
        assert!(!running(&h));
        assert!(log_tail(&h, 1000).contains("started"));
        assert!(home.join("work").is_dir());
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A hub for the end-to-end test: MYOUS_E2E_HUB, else one built from
    /// the hub source ($MYOUS_HUB_SRC, or the myoushq checkout next to
    /// this one) and started on a free port, ended when dropped.
    struct Hub {
        url: String,
        proc: Option<Child>,
    }

    impl Drop for Hub {
        fn drop(&mut self) {
            if let Some(p) = self.proc.as_mut() {
                let _ = p.kill();
                let _ = p.wait();
            }
        }
    }

    fn local_hub(tmp: &Path) -> Hub {
        if let Ok(url) = std::env::var("MYOUS_E2E_HUB") {
            return Hub { url, proc: None };
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let src = std::env::var_os("MYOUS_HUB_SRC")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("../myoushq/hub"));
        assert!(
            src.join("main.go").is_file(),
            "no hub source at {} (set MYOUS_HUB_SRC or MYOUS_E2E_HUB)",
            src.display()
        );
        let go = std::env::var("GO").unwrap_or_else(|_| {
            if crate::engine::which("go").is_some() {
                "go".into()
            } else {
                "/usr/local/go/bin/go".into()
            }
        });
        let bin = tmp.join(if cfg!(windows) { "hub.exe" } else { "hub" });
        let mut build = Command::new(&go);
        build
            .args(["build", "-o"])
            .arg(&bin)
            .arg(".")
            .current_dir(&src);
        let (st, out) = run_capture(build, Duration::from_secs(600));
        assert_eq!(st, 0, "building the hub: {out}");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let url = format!("http://127.0.0.1:{port}");
        let log = std::fs::File::create(tmp.join("hub.log")).unwrap();
        let proc = Command::new(&bin)
            .env("LISTEN_ADDR", format!("127.0.0.1:{port}"))
            .env("DATA_DIR", tmp.join("hubdata"))
            .env("WEB_DIR", src.join("web"))
            .env("DOCS_DIR", root.join("docs"))
            .env("RELAY_URL", format!("ws://127.0.0.1:{port}"))
            .env("PUBLIC_URL", &url)
            .env("POW_DIFFICULTY", "10")
            .env("RATE_LIMIT_SCALE", "100")
            // A developer's disk may have less free than production's 4 GB guard.
            .env("MIN_FREE_DISK_BYTES", "104857600")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("start the hub");
        let hub = Hub {
            url,
            proc: Some(proc),
        };
        for _ in 0..100 {
            if ureq::get(&format!("{}/config.json", hub.url))
                .call()
                .is_ok()
            {
                return hub;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "the hub didn't answer; see {}",
            tmp.join("hub.log").display()
        );
    }

    /// A Chromium for the end-to-end test: $MYOUS_E2E_BROWSER, else the
    /// Chrome for Testing that Playwright downloads.
    fn e2e_browser() -> Option<crate::browser::Found> {
        let path = std::env::var_os("MYOUS_E2E_BROWSER")
            .map(PathBuf::from)
            .or_else(|| {
                let home = std::env::var_os("HOME")?;
                let cache = PathBuf::from(home).join("Library/Caches/ms-playwright");
                let mut dirs: Vec<_> = std::fs::read_dir(&cache)
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().starts_with("chromium-"))
                            .unwrap_or(false)
                    })
                    .collect();
                dirs.sort();
                dirs.into_iter().rev().find_map(|d| {
                    ["chrome-mac-x64", "chrome-mac-arm64", "chrome-mac"]
                        .iter()
                        .map(|m| {
                            d.join(m).join(
                                "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                            )
                        })
                        .find(|p| p.is_file())
                })
            })?;
        path.is_file().then(|| crate::browser::Found {
            id: "custom".into(),
            name: "Chromium".into(),
            path: path.to_string_lossy().to_string(),
        })
    }

    /// The whole way with the real Go worker, a local hub and a Python
    /// agent: `cargo test -- --ignored e2e` (after worker/build-worker.sh;
    /// MYOUS_WORKER_BIN runs another build; MYOUS_PYTHON is a Python with
    /// the client, default /tmp/myous-venv/bin/python). Starts a worker as
    /// the app does, pairs the agent with it, runs a command that reads
    /// and one that changes things (which asks the owner at the "changes"
    /// level: answered here as the app would), stops it. With a browser:
    /// the worker's browser is a real Chromium launched as the app does
    /// once the worker runs (headless here; $MYOUS_E2E_BROWSER, else Playwright's Chrome for
    /// Testing), Playwright's Python is $MYOUS_E2E_PW_PYTHON (default
    /// /tmp/pw-venv/bin/python): the help names both, and a Playwright
    /// script the agent runs through `myous exec` reads the start page's
    /// title over the browser's port.
    #[test]
    #[ignore]
    fn e2e() {
        let tmp = temp("e2e");
        let hub = local_hub(&tmp);
        let home = tmp.join("worker");
        let h = home.to_string_lossy().to_string();
        let python =
            std::env::var("MYOUS_PYTHON").unwrap_or_else(|_| "/tmp/myous-venv/bin/python".into());

        // The worker tauri-build put next to the app (target/<profile>; this
        // test runs from target/<profile>/deps), unless overridden.
        let exe = std::env::current_exe().unwrap();
        let app = exe.parent().unwrap().parent().unwrap().join("myous");
        let (bin, _) = find_worker(&app, std::env::var_os("MYOUS_WORKER_BIN")).unwrap();
        println!("worker: {}", bin.display());

        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("settings.json"),
            format!(r#"{{"hub": "{}"}}"#, hub.url),
        )
        .unwrap();
        crate::files::set_review_level(&home, "changes").unwrap();
        let args = worker_args(&home, &bin, "E2E PC", "A test worker", &[]);
        let (tx, rx) = std::sync::mpsc::channel();
        start(&h, &args, move |st| tx.send(st).unwrap()).unwrap();

        let status = || -> serde_json::Value {
            std::fs::read_to_string(crate::paths::status_file(&home))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        };
        let mut code = String::new();
        for _ in 0..120 {
            if let Some(c) = status()["invite"]["code"].as_str() {
                code = c.to_string();
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(
            !code.is_empty(),
            "no invite; worker.log:\n{}",
            log_tail(&h, 4000)
        );
        assert_eq!(status()["phase"], "running", "{}", status());

        let agent = tmp.join("alice");
        let run = |args: &[&str]| {
            let mut cmd = Command::new(&python);
            cmd.args(["-m", "myous"])
                .args(args)
                .env("MYOUS_HOME", &agent);
            run_capture(cmd, Duration::from_secs(150))
        };
        assert_eq!(
            run(&["init", "--alias", "alice", "--hub", &hub.url]).0,
            0,
            "no Python client at {python} (set MYOUS_PYTHON)"
        );
        let (st, out) = run(&["accept", &code, "--wait", "60", "--relationship", "other"]);
        assert_eq!(st, 0, "{out}");

        // Reads run at the "changes" level.
        let (st, out) = run(&[
            "exec",
            "E2E PC",
            "--",
            if cfg!(windows) { "Get-Location" } else { "pwd" },
        ]);
        assert_eq!(st, 0, "{out}");
        assert!(out.contains("work"), "{out}");

        // A change asks the owner; the app's answer lets it run.
        let approvals = crate::paths::approvals_dir(&home);
        let answer = std::thread::spawn(move || {
            for _ in 0..120 {
                let q = std::fs::read_dir(&approvals).ok().and_then(|d| {
                    d.flatten()
                        .map(|e| e.path())
                        .find(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                });
                if let Some(q) = q {
                    let id = q.file_stem().unwrap().to_string_lossy().to_string();
                    crate::files::answer_approval(
                        q.parent().unwrap().parent().unwrap(),
                        &id,
                        "allow",
                    )
                    .unwrap();
                    return true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            false
        });
        let (st, out) = run(&["exec", "E2E PC", "--", "echo made > made.txt"]);
        assert!(answer.join().unwrap(), "the worker never asked");
        assert_eq!(st, 0, "{out}");
        assert!(home.join("work").join("made.txt").is_file());

        // The worker's browser, as the app starts it, here while the
        // worker runs (Open browser on the Browser card). Started with the
        // worker it is found the same way: the worker reads browser.json
        // at each help.
        let chromium = e2e_browser();
        let pw_python = std::env::var("MYOUS_E2E_PW_PYTHON")
            .unwrap_or_else(|_| "/tmp/pw-venv/bin/python".into());
        let mut browser_port = 0;
        // Ended even if the test fails part way.
        struct EndBrowser(PathBuf);
        impl Drop for EndBrowser {
            fn drop(&mut self) {
                crate::browser::stop(&self.0);
            }
        }
        let _end_browser = EndBrowser(home.clone());
        if let Some(found) = &chromium {
            println!("browser: {}", found.path);
            let log: crate::browser::Log = std::sync::Arc::new(|l| println!("{l}"));
            let b = crate::browser::start_with(
                &home,
                "E2E PC",
                found,
                &pw_python,
                vec!["--headless=new".into()],
                log,
            )
            .unwrap();
            browser_port = b.port;
        } else {
            println!("no Chromium for the browser part (set MYOUS_E2E_BROWSER)");
        }
        if browser_port != 0 {
            // The browser answers and browser.json says where (the app
            // writes it once the port answers).
            let json_file = crate::paths::browser_json(&home);
            for _ in 0..60 {
                if json_file.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            let j: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&json_file).expect("browser.json")).unwrap();
            assert_eq!(j["port"].as_u64(), Some(browser_port as u64));
            // The help names the browser and the Python, which the worker
            // read from browser.json (the app passes no --note for them).
            assert_eq!(run(&["send", "E2E PC", "help"]).0, 0);
            let mut help = String::new();
            for _ in 0..30 {
                let (_, out) = run(&["inbox", "--json"]);
                help.push_str(&out);
                if help.contains("Python with Playwright") {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
            assert!(
                help.contains(&format!("CDP at http://127.0.0.1:{browser_port}")),
                "{help}"
            );
            assert!(
                help.contains(&format!("Python with Playwright: {pw_python} script.py")),
                "{help}"
            );
            println!("the worker's help: {help}");
            // An agent's Playwright script, run on the worker through `myous
            // exec`, reads the start page over the browser's port.
            let script = tmp.join("title.py");
            std::fs::write(
                &script,
                format!(
                    "from playwright.sync_api import sync_playwright\n\
                     with sync_playwright() as p:\n\
                     \x20   b = p.chromium.connect_over_cdp('http://127.0.0.1:{browser_port}')\n\
                     \x20   print('TITLE:', b.contexts[0].pages[0].title())\n"
                ),
            )
            .unwrap();
            let allow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let on = allow.clone();
            let approvals = crate::paths::approvals_dir(&home);
            let answerer = std::thread::spawn(move || {
                while on.load(std::sync::atomic::Ordering::SeqCst) {
                    for e in std::fs::read_dir(&approvals)
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        let p = e.path();
                        if p.extension().map(|x| x == "json").unwrap_or(false) {
                            let id = p.file_stem().unwrap().to_string_lossy().to_string();
                            let _ = crate::files::answer_approval(
                                p.parent().unwrap().parent().unwrap(),
                                &id,
                                "allow",
                            );
                        }
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
            let (st, out) = run(&["cp", &script.to_string_lossy(), "E2E PC:title.py"]);
            assert_eq!(st, 0, "{out}");
            let cmd = format!("{pw_python} title.py");
            let (st, out) = run(&["exec", "E2E PC", "--", &cmd]);
            allow.store(false, std::sync::atomic::Ordering::SeqCst);
            answerer.join().unwrap();
            assert_eq!(st, 0, "{out}");
            assert!(out.contains("TITLE: myous · E2E PC"), "{out}");
            println!("the agent's script: {out}");
            // Show browser: its page comes to the front (DevTools' activate).
            let log: crate::browser::Log = std::sync::Arc::new(|l| println!("{l}"));
            assert_eq!(
                crate::browser::show(&home, "E2E PC", log).unwrap(),
                "brought to the front"
            );
        }

        assert!(stop(&h, 0));
        crate::browser::stop(&home);
        if browser_port != 0 {
            assert!(!crate::paths::browser_json(&home).exists());
            assert!(
                !crate::browser::devtools_answers(browser_port),
                "the browser ended with the worker"
            );
        }
        rx.recv_timeout(Duration::from_secs(15)).expect("no exit");
        assert!(!running(&h));
        let log = log_tail(&h, 20000);
        assert!(log.contains("made.txt"), "{log}");
        println!("worker.log:\n{log}");
        drop(hub);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

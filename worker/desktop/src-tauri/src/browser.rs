//! The worker's browser without a container. In a container the worker's
//! Chromium and Playwright live inside it; a worker that runs directly on
//! this computer gets a browser here instead: Edge or Chrome (whichever
//! is installed) with a profile of its own in the worker's folder, started
//! as a plain process with a DevTools port (never through Playwright,
//! which would mark every page as automated), and Playwright for Python in
//! the app's own environment, made with the Python the owner has. The
//! browser lives and dies with the worker; `browser.json` in the worker
//! folder names its port and that Python while it runs, as for myous for
//! Mac's browser on the Mac, and the worker reads it for its help, which
//! tells agents the address and the Python (so a browser opened while the
//! worker runs is in its next help).
//!
//! On a Mac this is also myous for Mac's browser (`worker/mac/src/
//! Browser.m`, ported here): Chrome, Edge, Brave or Chromium run under
//! `sandbox-exec` with that app's seatbelt profile, which keeps the browser
//! to its own folders, both without a container and for a worker in a
//! container that asked for its browser on the Mac (`browser: "mac"` in
//! app.json). For the latter, browser.json is the Mac app's (host
//! `host.docker.internal`, no `kind`), which the container's forward.py
//! reads; the browser starts hidden when `browser_hidden` says so.

use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::engine::{hide, run_capture};
use crate::{files, paths};

/// The Playwright for Python the app installs: the container's
/// (worker/Dockerfile), so agents' scripts behave the same in both.
pub const PLAYWRIGHT: &str = "1.63.0";

/// Where the app's lines go (the app log).
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// A browser the app can run.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Found {
    /// Its profile folder's name under `<home>/browser`: "msedge",
    /// "chrome", "chromium", "custom" ($MYOUS_BROWSER_BIN); on a Mac the
    /// bundle id ("com.google.Chrome").
    pub id: String,
    /// As the screens say it: "Edge", "Chrome", "Chromium".
    pub name: String,
    pub path: String,
}

/// A page to get something from.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Link {
    pub name: String,
    pub url: String,
}

const EDGE_URL: &str = "https://www.microsoft.com/edge/download";
const CHROME_URL: &str = "https://www.google.com/chrome/";

/// Windows: Edge (always there on Windows 10 and 11), then Chrome, where
/// their installers put them, else where the registry's App Paths say.
const WINDOWS: [(&str, &str, &str, &str); 2] = [
    (
        "msedge",
        "Edge",
        r"Microsoft\Edge\Application\msedge.exe",
        "msedge.exe",
    ),
    (
        "chrome",
        "Chrome",
        r"Google\Chrome\Application\chrome.exe",
        "chrome.exe",
    ),
];

/// Linux: on PATH, in this order.
const LINUX: [(&str, &str, &str); 6] = [
    ("google-chrome", "chrome", "Chrome"),
    ("google-chrome-stable", "chrome", "Chrome"),
    ("chromium", "chromium", "Chromium"),
    ("chromium-browser", "chromium", "Chromium"),
    ("microsoft-edge", "msedge", "Edge"),
    ("microsoft-edge-stable", "msedge", "Edge"),
];

/// A Mac: the apps, in myous for Mac's order, in /Applications or the
/// owner's own ~/Applications. Each is (bundle id, app folder, program).
/// The bundle id names the profile folder, as myous for Mac names it, so
/// a worker folder moved from that app keeps the browser's logins.
const MAC: [(&str, &str, &str, &str); 4] = [
    (
        "com.google.Chrome",
        "Google Chrome.app",
        "Google Chrome",
        "Chrome",
    ),
    (
        "com.microsoft.edgemac",
        "Microsoft Edge.app",
        "Microsoft Edge",
        "Edge",
    ),
    (
        "com.brave.Browser",
        "Brave Browser.app",
        "Brave Browser",
        "Brave",
    ),
    (
        "org.chromium.Chromium",
        "Chromium.app",
        "Chromium",
        "Chromium",
    ),
];

/// The browser to run: $MYOUS_BROWSER_BIN (a development override), else
/// the first of the system's list that is there. What it reads is passed
/// in, for the tests.
pub fn find_browser_with(
    os: &str,
    which: &dyn Fn(&str) -> Option<PathBuf>,
    var: &dyn Fn(&str) -> Option<String>,
    is_file: &dyn Fn(&Path) -> bool,
    app_path: &dyn Fn(&str) -> Option<String>,
) -> Option<Found> {
    let found = |id: &str, name: &str, p: &Path| Found {
        id: id.into(),
        name: name.into(),
        path: p.to_string_lossy().to_string(),
    };
    if let Some(p) = var("MYOUS_BROWSER_BIN") {
        let p = PathBuf::from(p);
        if is_file(&p) {
            return Some(found("custom", "Chromium", &p));
        }
    }
    match os {
        "windows" => {
            for (id, name, rel, exe) in WINDOWS {
                for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
                    if let Some(b) = var(base) {
                        let p = Path::new(&b).join(rel);
                        if is_file(&p) {
                            return Some(found(id, name, &p));
                        }
                    }
                }
                if let Some(p) = app_path(exe) {
                    let p = PathBuf::from(p);
                    if is_file(&p) {
                        return Some(found(id, name, &p));
                    }
                }
            }
        }
        "macos" => {
            let mut dirs = vec![PathBuf::from("/Applications")];
            dirs.extend(var("HOME").map(|h| Path::new(&h).join("Applications")));
            for (id, app, exe, name) in MAC {
                for d in &dirs {
                    let p = d.join(app).join("Contents").join("MacOS").join(exe);
                    if is_file(&p) {
                        return Some(found(id, name, &p));
                    }
                }
            }
        }
        _ => {
            for (cmd, id, name) in LINUX {
                if let Some(p) = which(cmd) {
                    if !is_snap(&p) {
                        return Some(found(id, name, &p));
                    }
                }
            }
        }
    }
    None
}

/// A snap's browser (Ubuntu's Chromium) can't use a profile in a hidden
/// folder such as ~/.myous-worker, so it is passed over: the program
/// itself under /snap, or the small script Ubuntu puts in /usr/bin that
/// runs it.
fn is_snap(p: &Path) -> bool {
    if let Ok(full) = p.canonicalize() {
        if full.to_string_lossy().contains("/snap/") {
            return true;
        }
    }
    match fs::metadata(p) {
        Ok(m) if m.len() < 16 * 1024 => fs::read(p)
            .map(|b| String::from_utf8_lossy(&b).contains("/snap/"))
            .unwrap_or(false),
        _ => false,
    }
}

/// The default value of a registry key as `reg query … /ve` prints it.
pub fn parse_reg_default(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.split_once("REG_SZ").map(|(_, v)| v.trim()))
        .map(|v| v.trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

/// Where Windows' App Paths say a program is, for this user or all.
fn app_path(exe: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    for root in ["HKCU", "HKLM"] {
        let mut cmd = crate::engine::command("reg");
        cmd.args([
            "query",
            &format!(r"{root}\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe}"),
            "/ve",
        ]);
        let (status, out) = run_capture(cmd, Duration::from_secs(5));
        if status == 0 {
            if let Some(p) = parse_reg_default(&out) {
                return Some(p);
            }
        }
    }
    None
}

fn env_var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

pub fn find_browser() -> Option<Found> {
    // A Mac: the apps as Launch Services knows them, as myous for Mac finds them.
    #[cfg(target_os = "macos")]
    if env_var("MYOUS_BROWSER_BIN").is_none() {
        return mac::find("");
    }
    find_browser_with(
        crate::platform::os(),
        &crate::engine::which,
        &env_var,
        &|p: &Path| p.is_file(),
        &app_path,
    )
}

/// A Python that can make the app's environment.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Python {
    /// The interpreter itself (`sys.executable`), never a launcher.
    pub path: String,
    /// "3.12".
    pub version: String,
    /// It can make an environment (Debian and Ubuntu ship that part,
    /// python3-venv, separately).
    pub venv: bool,
}

const PROBE: &str = "import sys, importlib.util as u; print(sys.executable); \
print('%d.%d' % sys.version_info[:2]); \
print(u.find_spec('venv') is not None and u.find_spec('ensurepip') is not None)";

/// Playwright for Python (1.63) needs 3.10 or newer.
pub const PYTHON_MIN: &str = "3.10";

fn version_ok(v: &str) -> bool {
    let mut it = v.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    let (major, minor) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    major > 3 || (major == 3 && minor >= 10)
}

/// The first Python 3.10+ there is: `py -3`, `python`, `python3` on
/// Windows, `python3`, `python`, then `python3.14` down to `python3.10`
/// elsewhere (Homebrew's and deadsnakes' names), each run by its full
/// path (`engine::which`). `probe` runs one with the arguments given and
/// returns its output. Also the version of an older one found, if any.
pub fn find_python_with(
    os: &str,
    which: &dyn Fn(&str) -> Option<PathBuf>,
    probe: &dyn Fn(&Path, &[String]) -> Option<String>,
) -> (Option<Python>, String) {
    let tries: &[(&str, &[&str])] = if os == "windows" {
        &[("py", &["-3"]), ("python", &[]), ("python3", &[])]
    } else {
        &[
            ("python3", &[]),
            ("python", &[]),
            ("python3.14", &[]),
            ("python3.13", &[]),
            ("python3.12", &[]),
            ("python3.11", &[]),
            ("python3.10", &[]),
        ]
    };
    let mut old = String::new();
    for (name, pre) in tries {
        let Some(p) = which(name) else { continue };
        // A Mac's /usr/bin/python3 is the Command Line Tools' 3.9, too old
        // for Playwright; where those tools aren't installed, running it
        // puts up the system's dialog offering to install them.
        if os == "macos" && p == Path::new("/usr/bin/python3") {
            continue;
        }
        let mut args: Vec<String> = pre.iter().map(|s| s.to_string()).collect();
        args.extend(["-c".into(), PROBE.into()]);
        let Some(out) = probe(&p, &args) else {
            continue;
        };
        let lines: Vec<&str> = out.lines().map(str::trim).collect();
        if lines.len() < 3 {
            continue;
        }
        if !version_ok(lines[1]) {
            if old.is_empty() {
                old = lines[1].to_string();
            }
            continue;
        }
        let exe = if lines[0].is_empty() {
            p.to_string_lossy().to_string()
        } else {
            lines[0].to_string()
        };
        return (
            Some(Python {
                path: exe,
                version: lines[1].into(),
                venv: lines[2] == "True",
            }),
            old,
        );
    }
    (None, old)
}

fn probe_python(p: &Path, args: &[String]) -> Option<String> {
    let mut cmd = crate::engine::command(p);
    cmd.args(args);
    let (status, out) = run_capture(cmd, Duration::from_secs(15));
    (status == 0).then_some(out)
}

pub fn find_python() -> (Option<Python>, String) {
    find_python_with(crate::platform::os(), &crate::engine::which, &probe_python)
}

/// The app's own Python environment, with Playwright in it.
pub fn env_dir() -> PathBuf {
    paths::app_data().join("browser-python")
}

pub fn env_python_in(dir: &Path, os: &str) -> PathBuf {
    if os == "windows" {
        dir.join("Scripts").join("python.exe")
    } else {
        dir.join("bin").join("python")
    }
}

pub fn env_python() -> PathBuf {
    env_python_in(&env_dir(), crate::platform::os())
}

/// The Playwright version a Python has ("" when it has none, or the
/// Python doesn't run).
pub fn playwright_version(python: &Path) -> String {
    if !python.is_file() {
        return String::new();
    }
    let mut cmd = crate::engine::command(python);
    cmd.args([
        "-c",
        "import importlib.metadata as m, playwright.sync_api; print(m.version('playwright'))",
    ]);
    let (status, out) = run_capture(cmd, Duration::from_secs(30));
    if status != 0 {
        return String::new();
    }
    out.lines().last().unwrap_or("").trim().to_string()
}

/// What the Set up card shows about the worker's browser.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Check {
    pub browser: Option<Found>,
    /// The Python the environment is (or would be) made with; not looked
    /// for when the environment already has Playwright.
    pub python: Option<Python>,
    /// A Python found that is too old ("3.8"), if no newer one was.
    pub python_old: String,
    /// Playwright's version in the app's environment, "" when not there.
    pub playwright: String,
    pub env_python: String,
    /// Linux: the line that installs Python (and its venv part) with this
    /// distribution's packages; "" where the app doesn't know it.
    pub python_line: String,
    pub venv_line: String,
    pub python_url: String,
    /// The browsers' download pages, in the order to offer them.
    pub get: Vec<Link>,
}

/// The package lines for Python on Linux: (Python with its venv part, the
/// venv part alone).
pub fn python_lines(os: &str, has: &dyn Fn(&str) -> bool) -> (String, String) {
    if os != "linux" {
        return (String::new(), String::new());
    }
    let pick = [
        (
            "apt-get",
            "sudo apt install python3 python3-venv",
            "sudo apt install python3-venv",
        ),
        ("dnf", "sudo dnf install python3", ""),
        ("pacman", "sudo pacman -S python", ""),
        ("zypper", "sudo zypper install python3", ""),
    ];
    for (tool, line, venv) in pick {
        if has(tool) {
            return (line.into(), venv.into());
        }
    }
    (String::new(), String::new())
}

pub fn get_links(os: &str) -> Vec<Link> {
    let edge = Link {
        name: "Edge".into(),
        url: EDGE_URL.into(),
    };
    let chrome = Link {
        name: "Chrome".into(),
        url: CHROME_URL.into(),
    };
    if os == "windows" {
        vec![edge, chrome]
    } else {
        vec![chrome, edge]
    }
}

/// Everything at once (a few seconds at most: Python is run to ask it).
pub fn check() -> Check {
    let os = crate::platform::os();
    let envpy = env_python();
    let playwright = playwright_version(&envpy);
    let (python, python_old) = if playwright.is_empty() {
        find_python()
    } else {
        (None, String::new())
    };
    let (python_line, venv_line) = python_lines(os, &|n: &str| crate::engine::which(n).is_some());
    Check {
        browser: find_browser(),
        python,
        python_old,
        playwright,
        env_python: envpy.to_string_lossy().to_string(),
        python_line,
        venv_line,
        python_url: if os == "windows" {
            "https://www.python.org/downloads/windows/".into()
        } else if os == "macos" {
            "https://www.python.org/downloads/macos/".into()
        } else {
            "https://www.python.org/downloads/".into()
        },
        get: get_links(os),
    }
}

/// What "Install Playwright" runs: the environment made with the owner's
/// Python if it isn't there (or is broken: `--clear`), then pip, pinned.
/// No `playwright install`: no browser is downloaded, agents connect to
/// the one the app runs.
pub fn install_steps(
    python: Option<&Python>,
    dir: &Path,
    os: &str,
    env_works: bool,
) -> Result<Vec<Vec<String>>, String> {
    let envpy = env_python_in(dir, os).to_string_lossy().to_string();
    let mut steps = Vec::new();
    if !env_works {
        let py = python.ok_or(format!(
            "there's no Python {PYTHON_MIN} or newer on this computer"
        ))?;
        if !py.venv {
            return Err(format!(
                "Python {} here can't make an environment (its venv part is missing)",
                py.version
            ));
        }
        let mut s = vec![py.path.clone(), "-m".into(), "venv".into()];
        if dir.exists() {
            s.push("--clear".into());
        }
        s.push(dir.to_string_lossy().to_string());
        steps.push(s);
    }
    steps.push(vec![
        envpy,
        "-m".into(),
        "pip".into(),
        "install".into(),
        "--disable-pip-version-check".into(),
        "--no-input".into(),
        format!("playwright=={PLAYWRIGHT}"),
    ]);
    Ok(steps)
}

/// Whether the environment's Python runs at all.
pub fn env_works(python: &Path) -> bool {
    if !python.is_file() {
        return false;
    }
    let mut cmd = crate::engine::command(python);
    cmd.args(["-c", "import sys"]);
    run_capture(cmd, Duration::from_secs(15)).0 == 0
}

/// A free port on the loopback, chosen here: with
/// `--remote-debugging-port=0` Chrome takes itself for a test harness
/// and marks every page as automated.
pub fn free_port() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()
        .map(|a| a.port())
}

/// A `file://` address for a local file, on any system.
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page the browser opens with, and its home page: whose window this
/// is, so it is never mistaken for the owner's own browser. The Mac's
/// words, without its sandbox.
pub fn start_page(worker: &str) -> String {
    let name = escape(if worker.is_empty() {
        "the worker"
    } else {
        worker
    });
    format!(
        "<!doctype html><meta charset=utf-8><title>myous · {name}</title>\
<style>body{{font:15px/1.5 -apple-system,system-ui,'Segoe UI',sans-serif;color:#222;background:#f4f1ea;margin:0}}\
main{{max-width:36em;margin:0 auto;padding:3em 2em}}h1{{font-size:1.4em;margin:0 0 .5em}}h1 b{{color:#1f7a6d}}\
p{{margin:.5em 0}}small{{color:#666}}</style>\
<main><h1><b>myous</b> · {name}'s browser</h1>\
<p>This window belongs to the worker <b>{name}</b>: your agent browses here, and so can you. \
Log into sites here for your agent; they stay logged in for it.</p>\
<p>It has a profile of its own in the worker's folder: it never sees your own browser, its logins or its history.</p>\
<p><small>If you close this window, your agent can't use the browser until you open it again with Show browser in myous. \
Downloads go to the downloads folder in the worker's work folder.</small></p></main>"
    )
}

/// The start page of a browser under the Mac's sandbox: myous for Mac's
/// words (Browser.m's `startPage`), with the bubble that explains the bar
/// Chrome shows about `--no-sandbox`.
pub fn start_page_sandboxed(worker: &str) -> String {
    let name = escape(if worker.is_empty() {
        "the worker"
    } else {
        worker
    });
    format!(
        "<!doctype html><meta charset=utf-8><title>myous · {name}</title>\
<style>body{{font:15px/1.5 -apple-system,system-ui,sans-serif;color:#222;background:#f4f1ea;margin:0;min-height:100vh;display:flex;flex-direction:column}}\
main{{max-width:36em;margin:0 auto;padding:2em 2em 3em}}h1{{font-size:1.4em;margin:0 0 .5em}}h1 b{{color:#1f7a6d}}p{{margin:.5em 0}}small{{color:#666}}\
.bubble{{position:relative;align-self:flex-end;margin:22px 14px 0 14px;max-width:24em;background:#fff6dc;border:1px solid #d9b25a;border-radius:12px;padding:.7em 1em;box-shadow:0 2px 8px rgba(0,0,0,.08)}}\
.bubble:before,.bubble:after{{content:'';position:absolute;right:22px;border:12px solid transparent;border-top:0}}\
.bubble:before{{top:-13px;border-bottom-color:#d9b25a}}.bubble:after{{top:-11px;border-bottom-color:#fff6dc}}\
.bubble b{{color:#7a5a00}}kbd{{font:inherit;white-space:nowrap;background:#eee;border:1px solid #ccc;border-radius:4px;padding:0 .3em}}</style>\
<div class=bubble><b>That bar is expected.</b> Close it with its &times;. It says <kbd>--no-sandbox</kbd> is unsupported: \
myous confines this browser with a macOS sandbox of its own, and macOS allows no sandbox inside another, so the browser's built-in one is off here. The outer one does the work.</div>\
<main><h1><b>myous</b> · {name}'s browser</h1>\
<p>This window belongs to the worker <b>{name}</b>: your agent browses here, and so can you. \
Sites you log into here stay logged in for it.</p>\
<p>It is kept to the worker's folder by that sandbox: it can't read your files, reach other programs on this Mac, or save anywhere but the worker's downloads folder.</p>\
<p><small>Your own browser and its logins are untouched. Close this window when you like; myous starts it again while the worker runs.</small></p></main>"
    )
}

/// myous for Mac's seatbelt profile (Browser.m's `seatbeltFor:`, the same
/// text; a test compares them): the browser may use its own folder, its
/// downloads and the network, nothing else of the Mac. `home` is the
/// owner's home folder, `bundle` the browser's app folder, `id` its
/// bundle id (for its preferences file).
pub fn seatbelt(home: &str, browser_dir: &str, downloads: &str, id: &str, bundle: &str) -> String {
    format!(
        "(version 1)\n\
;; myous: this worker's browser may use its own folder and the network, nothing else of this Mac.\n\
(allow default)\n\
;; The owner's files: nothing, except the browser's own folder and downloads.\n\
(deny file-read* file-write* (subpath \"{home}\"))\n\
(allow file-read-metadata (subpath \"{home}\"))\n\
(allow file-read* file-write* (subpath \"{browser_dir}\"))\n\
(allow file-read* file-write* (subpath \"{downloads}\"))\n\
(allow file-read* (subpath \"{home}/Library/Fonts\"))\n\
(allow file-read* (literal \"{home}/Library/Preferences/.GlobalPreferences.plist\"))\n\
(allow file-read* (literal \"{home}/Library/Preferences/{id}.plist\"))\n\
;; Other disks: no.\n\
(deny file-read* file-write* (subpath \"/Volumes\"))\n\
;; Programs: only the browser's own.\n\
(deny process-exec*)\n\
(allow process-exec* (subpath \"{bundle}\"))\n\
;; Nothing on this Mac's local ports (the owner's services, Docker, dev servers).\n\
(deny network-outbound (remote ip \"localhost:*\"))\n"
    )
}

/// What myous for Mac runs to learn whether this macOS can sandbox a
/// program (Browser.m's `canSandbox`): sandbox-exec with a small profile.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const SANDBOX_PROBE: &str =
    "(version 1)(allow default)(deny network-outbound (remote ip \"localhost:*\"))";

/// Why the browser can't run on this Mac, in myous for Mac's words (short:
/// they go in a pop-up's line), or None when it can.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn unavailable_reason(installed: bool, sandbox: bool) -> Option<&'static str> {
    if !installed {
        Some("no browser it can run is installed")
    } else if !sandbox {
        Some("this macOS can't sandbox it")
    } else {
        None
    }
}

/// myous for Mac's message when its browser can't start.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn cant_run(why: &str) -> String {
    format!(
        "Can't run the browser on this Mac: {why}. Install Google Chrome, Microsoft Edge, Brave or Chromium (all free), or choose the browser in the container."
    )
}

/// The app folder a program is in (`…/Google Chrome.app` for its
/// `Contents/MacOS/Google Chrome`), else the program's own folder.
pub fn bundle_of(exe: &Path) -> PathBuf {
    exe.ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .unwrap_or_else(|| exe.parent().unwrap_or(exe))
        .to_path_buf()
}

/// The profile's settings, set once before it exists (the owner's later
/// choices stand): a name, avatar and toolbar colour that say "myous", the
/// start page as home, downloads into the work folder, where the agent
/// can fetch them (`myous cp`). `profile` is in the worker folder `home`;
/// nothing is written through a link there (files.rs).
pub fn seed_preferences(
    home: &Path,
    profile: &Path,
    worker: &str,
    start_url: &str,
    downloads: &Path,
) {
    let def = profile.join("Default");
    let prefs = def.join("Preferences");
    if fs::symlink_metadata(&prefs).is_ok() {
        return;
    }
    if files::dir_in(home, &def).is_err() {
        return;
    }
    let d = downloads.to_string_lossy().to_string();
    let p = json!({
        "download": {"default_directory": d, "prompt_for_download": false},
        "savefile": {"default_directory": d},
        "profile": {
            "name": format!("{} · myous", if worker.is_empty() { "Worker" } else { worker }),
            "using_default_name": false,
            "avatar_index": 26,
            "using_default_avatar": false
        },
        "browser": {"theme": {"user_color": 0xFF1F_7A6Du32 as i32, "color_variant": 1}, "show_home_button": true},
        "homepage": start_url,
        "homepage_is_newtabpage": false
    });
    let _ = files::write_atomic(&prefs, p.to_string().as_bytes());
}

/// How one browser is run for one worker.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub exe: PathBuf,
    pub app: String,
    pub profile: PathBuf,
    pub start_url: String,
    pub port: u16,
    /// The environment's Python, for browser.json.
    pub python: String,
    /// More switches (the tests' `--headless=new`).
    pub extra: Vec<String>,
    /// A Mac: the seatbelt profile it runs under (`<home>/browser.sb`,
    /// through sandbox-exec), as myous for Mac runs it.
    pub sandbox: Option<PathBuf>,
    /// A Mac: the browser of a worker in a container (browser.json for
    /// the container's forward.py).
    pub container: bool,
    /// A Mac: hidden once it is up, until Show browser.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub hidden: bool,
}

/// How the browser runs, beyond the program and its port. All off on
/// Linux and Windows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct How {
    /// For a worker in a container (a Mac): browser.json as myous for Mac
    /// writes it, and downloads in `<home>/downloads`, which the
    /// container sees in the worker folder.
    pub container: bool,
    /// Under sandbox-exec with myous for Mac's seatbelt profile (a Mac).
    pub sandbox: bool,
    /// Hidden once its window is up, until Show browser (a Mac's
    /// `browser_hidden`).
    pub hidden: bool,
}

/// The browser's command line after the program: its own profile, the
/// port, no first-run questions, and the switches that keep pages working
/// while the window is hidden or behind others.
pub fn args(s: &Spec, os: &str) -> Vec<String> {
    let mut a = vec![
        format!("--user-data-dir={}", s.profile.display()),
        format!("--remote-debugging-port={}", s.port),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-search-engine-choice-screen".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-background-timer-throttling".into(),
    ];
    if os == "macos" {
        // Never the login keychain (and never its dialog).
        a.push("--use-mock-keychain".into());
    }
    if s.sandbox.is_some() {
        // No sandbox inside a sandbox on macOS: the seatbelt is the
        // sandbox (Chrome shows a bar about it; --test-type would hide the
        // bar but marks every page as automated). The rest as myous for
        // Mac runs it.
        a.push("--no-sandbox".into());
        a.push(format!(
            "--disk-cache-dir={}",
            s.profile.join("cache").display()
        ));
        a.push("--window-size=1200,800".into());
    }
    a.extend(s.extra.iter().cloned());
    a.push("--new-window".into());
    a.push(s.start_url.clone());
    a
}

/// Writes the start page and the profile's first settings; the folders
/// it needs. Each browser has its own profile (`<home>/browser/<id>`, as
/// on the Mac), so Edge and Chrome never share one. As on Linux and
/// Windows (the tests; the app uses `prepare_with`).
#[cfg(test)]
pub fn prepare(
    home: &Path,
    worker: &str,
    found: &Found,
    port: u16,
    python: &str,
    extra: Vec<String>,
) -> Result<Spec, String> {
    prepare_with(home, worker, found, port, python, extra, &How::default())
}

/// `prepare`, and on a Mac the sandbox's profile (`<home>/browser.sb`)
/// and a container worker's downloads folder.
pub fn prepare_with(
    home: &Path,
    worker: &str,
    found: &Found,
    port: u16,
    python: &str,
    extra: Vec<String>,
    how: &How,
) -> Result<Spec, String> {
    let dir = paths::browser_dir(home);
    let profile = dir.join(&found.id);
    // In a container the worker folder is mounted (~/.myous), the work
    // folder isn't: downloads where myous for Mac puts them.
    let downloads = if how.container {
        home.join("downloads")
    } else {
        home.join("work").join("downloads")
    };
    for d in [&dir, &profile, &downloads] {
        files::private_dir_in(home, d).map_err(|e| e.to_string())?;
    }
    let page = dir.join("start.html");
    let html = if how.sandbox {
        start_page_sandboxed(worker)
    } else {
        start_page(worker)
    };
    files::write_atomic(&page, html.as_bytes()).map_err(|e| format!("{}: {e}", page.display()))?;
    let start_url = file_url(&page);
    seed_preferences(home, &profile, worker, &start_url, &downloads);
    let sandbox = if how.sandbox {
        // The sandbox sees resolved paths (/private/tmp, not /tmp).
        let real = |p: &Path| {
            p.canonicalize()
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_string()
        };
        let text = seatbelt(
            &real(&paths::user_home()),
            &real(&dir),
            &real(&downloads),
            &found.id,
            &real(&bundle_of(Path::new(&found.path))),
        );
        let sb = home.join("browser.sb");
        fs::write(&sb, text).map_err(|e| format!("{}: {e}", sb.display()))?;
        Some(sb)
    } else {
        None
    };
    Ok(Spec {
        exe: PathBuf::from(&found.path),
        app: found.name.clone(),
        profile,
        start_url,
        port,
        python: python.into(),
        extra,
        sandbox,
        container: how.container,
        hidden: how.hidden,
    })
}

/// The command that runs the browser's program: itself, or on a Mac
/// sandbox-exec with the profile, which runs it confined (the same
/// process: sandbox-exec ends in exec, so the pid is the browser's).
fn browser_command(s: &Spec) -> std::process::Command {
    let mut cmd = match &s.sandbox {
        Some(sb) => {
            let mut c = crate::engine::command(SANDBOX_EXEC);
            c.arg("-f").arg(sb).arg(&s.exe);
            c
        }
        None => crate::engine::command(&s.exe),
    };
    hide(&mut cmd);
    cmd
}

/// browser.json: the keys myous for Mac writes (host, port, pid, app,
/// at), plus the Python with Playwright and `kind` "host" (on this
/// computer, no container in between). For a worker in a container (a
/// Mac): exactly myous for Mac's, the host as the container reaches it,
/// no `kind` (the worker in the container reads it through forward.py,
/// and its help names localhost:9222 as with the container's browser).
pub fn browser_json(spec: &Spec, pid: u32) -> Value {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if spec.container {
        return json!({
            "host": "host.docker.internal",
            "port": spec.port,
            "pid": pid,
            "app": spec.app,
            "at": at,
        });
    }
    json!({
        "host": "127.0.0.1",
        "port": spec.port,
        "pid": pid,
        "app": spec.app,
        "at": at,
        "python": spec.python,
        "kind": "host",
    })
}

// --- running it --------------------------------------------------------------

struct Proc {
    spec: Spec,
    /// None when adopted from a run before this app's (its pid is known).
    child: Option<Child>,
    /// 0 once it has ended.
    pid: u32,
    /// Which launch this is; a watcher of an earlier one stops.
    gen: u64,
    /// The owner asked to see it (Show browser): a Mac stops hiding it.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    shown: bool,
}

fn procs() -> &'static Mutex<HashMap<String, Proc>> {
    static P: OnceLock<Mutex<HashMap<String, Proc>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

static GEN: AtomicU64 = AtomicU64::new(1);

fn port_answers(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// A plain HTTP client for the browser's DevTools address (no proxy: the
/// loopback is never a proxy's).
fn local_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .proxy(None)
            .build(),
    )
}

fn get_text(url: &str) -> Result<String, String> {
    local_agent(Duration::from_secs(3))
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Whether a DevTools server answers on the port.
pub fn devtools_answers(port: u16) -> bool {
    port_answers(port) && get_text(&format!("http://127.0.0.1:{port}/json/version")).is_ok()
}

/// The browser a browser.json names, if it is this app's and still
/// answers: (port, pid, whether it is a container worker's). A container
/// worker's (myous for Mac's kind: no `kind`, the host as the container
/// reaches it) only on a Mac.
fn adoptable(home: &Path) -> Option<(u16, u32, bool)> {
    let v = files::read_json(&paths::browser_json(home))?;
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
    let host = v.get("host").and_then(Value::as_str).unwrap_or("");
    let container = match (kind, host) {
        ("host", "127.0.0.1") => false,
        ("", "host.docker.internal") if cfg!(target_os = "macos") => true,
        _ => return None,
    };
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let pid = v.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    devtools_answers(port).then_some((port, pid, container))
}

/// Starts the program for `spec`, keeps it as this worker's browser and
/// watches it: browser.json once its port answers, removed when it ends.
pub fn launch(home: &Path, spec: Spec, log: Log) -> Result<u32, String> {
    let key = home.to_string_lossy().to_string();
    files::dir_in(home, &spec.profile).map_err(|e| e.to_string())?;
    let log_file = paths::browser_log(home);
    let out = files::fresh_file(&log_file).map_err(|e| format!("{}: {e}", log_file.display()))?;
    let mut cmd = browser_command(&spec);
    cmd.args(args(&spec, crate::platform::os()))
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|e| e.to_string())?)
        .stderr(out);
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", spec.exe.display()))?;
    let pid = child.id();
    let gen = GEN.fetch_add(1, Ordering::SeqCst);
    log(format!(
        "browser: {} started (pid {pid}), profile {}, DevTools port {}{}; its output is in browser.log",
        spec.app,
        spec.profile.display(),
        spec.port,
        if spec.sandbox.is_some() {
            ", sandboxed to the worker's browser folder"
        } else {
            ""
        }
    ));
    procs().lock().unwrap().insert(
        key.clone(),
        Proc {
            spec,
            child: Some(child),
            pid,
            gen,
            shown: false,
        },
    );
    watch(home.to_path_buf(), gen, log);
    Ok(pid)
}

fn adopt(home: &Path, spec: Spec, pid: u32, log: Log) {
    let gen = GEN.fetch_add(1, Ordering::SeqCst);
    log(format!(
        "browser: {} from before is still running on port {} (pid {pid}); using it",
        spec.app, spec.port
    ));
    procs().lock().unwrap().insert(
        home.to_string_lossy().to_string(),
        Proc {
            spec,
            child: None,
            pid,
            gen,
            shown: false,
        },
    );
    watch(home.to_path_buf(), gen, log);
}

fn watch(home: PathBuf, gen: u64, log: Log) {
    let key = home.to_string_lossy().to_string();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut ready = false;
        let mut warned = false;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let (spec, pid, ended) = {
                let mut map = procs().lock().unwrap();
                let Some(p) = map.get_mut(&key) else { return };
                if p.gen != gen {
                    return;
                }
                let ended = match p.child.as_mut() {
                    Some(c) => match c.try_wait() {
                        Ok(None) => None,
                        Ok(Some(st)) => Some(format!("exit {}", st.code().unwrap_or(-1))),
                        Err(e) => Some(e.to_string()),
                    },
                    None => (!port_answers(p.spec.port)).then(|| "gone".to_string()),
                };
                if ended.is_some() {
                    p.child = None;
                    p.pid = 0;
                }
                (p.spec.clone(), p.pid, ended)
            };
            if let Some(why) = ended {
                let _ = fs::remove_file(paths::browser_json(&home));
                log(format!(
                    "browser: {} closed ({why}); {}",
                    spec.app,
                    if spec.container {
                        "myous starts it again while the worker runs"
                    } else {
                        "Show browser opens it again"
                    }
                ));
                return;
            }
            if !ready && port_answers(spec.port) {
                ready = true;
                let _ = crate::files::write_atomic(
                    &paths::browser_json(&home),
                    serde_json::to_string_pretty(&browser_json(&spec, pid))
                        .unwrap_or_default()
                        .as_bytes(),
                );
                log(format!(
                    "browser: {} answers on 127.0.0.1:{} (browser.json){}",
                    spec.app,
                    spec.port,
                    if spec.container {
                        "; the container's localhost:9222 goes there"
                    } else {
                        ""
                    }
                ));
                #[cfg(target_os = "macos")]
                if spec.hidden {
                    mac::hide_while_starting(key.clone(), gen, pid);
                }
            } else if !ready && !warned && started.elapsed() > Duration::from_secs(30) {
                warned = true;
                log(format!(
                    "browser: {} hasn't opened its port in 30 s; browser.log says why",
                    spec.app
                ));
            }
        }
    });
}

/// What `start` reports.
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    pub port: u16,
    pub app: String,
    pub python: String,
}

/// How the browser of a worker without a container runs: on a Mac under
/// the sandbox when this macOS can sandbox it (else as on Linux and
/// Windows, said in the log); plain elsewhere.
fn direct_how(log: &Log) -> How {
    #[cfg(target_os = "macos")]
    {
        if mac::can_sandbox() {
            return How {
                sandbox: true,
                ..How::default()
            };
        }
        log(
            "browser: this macOS can't sandbox it; it runs without myous's sandbox, as on Linux and Windows"
                .into(),
        );
    }
    let _ = log;
    How::default()
}

/// Starts the worker's browser, with the worker or while it runs (its
/// help names the browser from browser.json): only when there is a
/// browser and Playwright in the app's environment.
/// A browser from before (the app ended without stopping it) is taken
/// over when it still answers.
pub fn start(home: &Path, worker: &str, extra: Vec<String>, log: Log) -> Result<Started, String> {
    let found = find_browser()
        .ok_or("no browser found here (Chrome, Edge or Chromium); the worker runs without one")?;
    let envpy = env_python();
    if playwright_version(&envpy).is_empty() {
        return Err(
            "Playwright isn't installed in myous's Python environment yet (Set up offers it); the worker runs without a browser"
                .into(),
        );
    }
    let how = direct_how(&log);
    start_with_how(
        home,
        worker,
        &found,
        &envpy.to_string_lossy(),
        extra,
        how,
        log,
    )
}

/// `start_with_how` as on Linux and Windows (the tests).
#[cfg(test)]
pub fn start_with(
    home: &Path,
    worker: &str,
    found: &Found,
    python: &str,
    extra: Vec<String>,
    log: Log,
) -> Result<Started, String> {
    start_with_how(home, worker, found, python, extra, How::default(), log)
}

pub fn start_with_how(
    home: &Path,
    worker: &str,
    found: &Found,
    python: &str,
    extra: Vec<String>,
    how: How,
    log: Log,
) -> Result<Started, String> {
    let key = home.to_string_lossy().to_string();
    if let Some(p) = procs().lock().unwrap().get(&key) {
        if p.pid != 0 {
            return Ok(Started {
                port: p.spec.port,
                app: p.spec.app.clone(),
                python: p.spec.python.clone(),
            });
        }
    }
    match adoptable(home) {
        Some((port, pid, container)) if container == how.container => {
            let spec = prepare_with(home, worker, found, port, python, extra, &how)?;
            adopt(home, spec, pid, log);
            return Ok(Started {
                port,
                app: found.name.clone(),
                python: python.into(),
            });
        }
        // The other kind (a Mac's worker that changed between a container
        // and none while its browser ran): ended, a new one started.
        Some((_, pid, _)) if pid != 0 => end_gracefully(pid, None),
        _ => {}
    }
    let port = free_port().ok_or("no free port for the browser")?;
    let spec = prepare_with(home, worker, found, port, python, extra, &how)?;
    launch(home, spec, log)?;
    Ok(Started {
        port,
        app: found.name.clone(),
        python: python.into(),
    })
}

/// Whether the process `pid` exists.
pub(crate) fn pid_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = crate::engine::output(
            "tasklist",
            &["/FI", &format!("PID eq {pid}"), "/NH"],
            Duration::from_secs(10),
        );
        out.split_whitespace().any(|w| w == pid.to_string())
    } else {
        let mut cmd = crate::engine::command("kill");
        cmd.args(["-0", &pid.to_string()]);
        run_capture(cmd, Duration::from_secs(5)).0 == 0
    }
}

/// Asks the browser to close (its windows on Windows, SIGTERM elsewhere),
/// so it ends as if the owner quit it, then ends it after 8 s.
fn end_gracefully(pid: u32, mut child: Option<&mut Child>) {
    let gone = |child: &mut Option<&mut Child>| match child {
        Some(c) => !matches!(c.try_wait(), Ok(None)),
        None => !pid_alive(pid),
    };
    let p = pid.to_string();
    let signal = |force: bool| {
        let mut cmd = crate::engine::command(if cfg!(windows) { "taskkill" } else { "kill" });
        if cfg!(windows) {
            cmd.args(["/T"]);
            if force {
                cmd.arg("/F");
            }
            cmd.args(["/PID", &p]);
        } else {
            cmd.args([if force { "-KILL" } else { "-TERM" }, &p]);
        }
        let _ = run_capture(cmd, Duration::from_secs(15));
    };
    signal(false);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(8) {
        if gone(&mut child) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    signal(true);
    if let Some(c) = child {
        let _ = c.wait();
    }
}

/// Ends the worker's browser (with the worker, and when the app quits):
/// this app's, else the one a browser.json from before names, if it
/// still answers there.
pub fn stop(home: &Path) {
    let key = home.to_string_lossy().to_string();
    let taken = procs().lock().unwrap().remove(&key);
    match taken {
        Some(mut p) => {
            if p.pid != 0 {
                end_gracefully(p.pid, p.child.as_mut());
            }
        }
        None => {
            if let Some((_, pid, _)) = adoptable(home) {
                if pid != 0 {
                    end_gracefully(pid, None);
                }
            }
        }
    }
    let _ = fs::remove_file(paths::browser_json(home));
}

pub fn stop_all() {
    let homes: Vec<String> = procs().lock().unwrap().keys().cloned().collect();
    let threads: Vec<_> = homes
        .into_iter()
        .map(|h| std::thread::spawn(move || stop(Path::new(&h))))
        .collect();
    for t in threads {
        let _ = t.join();
    }
}

/// Whether this app runs a browser for the worker now.
#[cfg(test)]
#[allow(dead_code)] // the tests that use it don't run on Windows
fn running(home: &Path) -> bool {
    procs()
        .lock()
        .unwrap()
        .get(&home.to_string_lossy().to_string())
        .map(|p| p.pid != 0)
        .unwrap_or(false)
}

/// Brings a page of the browser to the front (DevTools' activate, which
/// raises its window): false when it has no page to show.
fn activate(port: u16) -> Result<bool, String> {
    let list: Value =
        serde_json::from_str(&get_text(&format!("http://127.0.0.1:{port}/json/list"))?)
            .map_err(|e| e.to_string())?;
    let id = list.as_array().and_then(|a| {
        a.iter()
            .find(|t| {
                t.get("type").and_then(Value::as_str) == Some("page")
                    && !t
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .starts_with("devtools://")
            })
            .and_then(|t| t.get("id").and_then(Value::as_str))
            .map(String::from)
    });
    match id {
        None => Ok(false),
        Some(id) => {
            get_text(&format!("http://127.0.0.1:{port}/json/activate/{id}"))?;
            Ok(true)
        }
    }
}

/// Windows lets only the program in front give the front away: this app
/// (whose window or tray was just clicked) lets the browser take it.
#[cfg(windows)]
fn allow_foreground() {
    #[link(name = "user32")]
    extern "system" {
        fn AllowSetForegroundWindow(pid: u32) -> i32;
    }
    // ASFW_ANY
    unsafe {
        AllowSetForegroundWindow(u32::MAX);
    }
}

#[cfg(not(windows))]
fn allow_foreground() {}

/// "Show browser": its window to the front. Running with a page: that
/// page is activated (no new window). Running without a window (Edge can
/// stay in the background): the program is run again with the same
/// profile and the start page, which opens a window in the running one.
/// Not running (the owner closed it): started again on the same port, so
/// the address the worker gave agents stays right. What it did, in words.
pub fn show(home: &Path, worker: &str, log: Log) -> Result<String, String> {
    let how = direct_how(&log);
    let fresh = |port: u16| -> Result<Spec, String> {
        let found = find_browser().ok_or("no browser found here")?;
        let envpy = env_python();
        prepare_with(
            home,
            worker,
            &found,
            port,
            &envpy.to_string_lossy(),
            Vec::new(),
            &how,
        )
    };
    show_with(home, &fresh, log)
}

/// `show`, with what a browser started from nothing would be (`fresh`,
/// given the port to use).
pub fn show_with(
    home: &Path,
    fresh: &dyn Fn(u16) -> Result<Spec, String>,
    log: Log,
) -> Result<String, String> {
    let key = home.to_string_lossy().to_string();
    let known = procs()
        .lock()
        .unwrap()
        .get(&key)
        .map(|p| (p.spec.clone(), p.pid));
    let (port, pid) = match &known {
        Some((s, pid)) => (s.port, *pid),
        None => adoptable(home)
            .map(|(port, pid, _)| (port, pid))
            .unwrap_or((0, 0)),
    };
    if port != 0 && port_answers(port) {
        // A Mac: out of hiding and to the front, as myous for Mac's Show
        // browser does; and no more hiding while it starts.
        #[cfg(target_os = "macos")]
        let shown = pid != 0 && {
            if let Some(p) = procs().lock().unwrap().get_mut(&key) {
                p.shown = true;
            }
            mac::show(pid)
        };
        #[cfg(not(target_os = "macos"))]
        let shown = {
            let _ = pid;
            false
        };
        allow_foreground();
        match activate(port) {
            Ok(true) => return Ok("brought to the front".into()),
            Ok(false) => {
                if let Some((spec, _)) = &known {
                    let mut cmd = browser_command(spec);
                    if spec.sandbox.is_some() {
                        cmd.args(["--no-sandbox", "--use-mock-keychain"]);
                    }
                    cmd.arg(format!("--user-data-dir={}", spec.profile.display()))
                        .arg(&spec.start_url)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    let mut c = cmd.spawn().map_err(|e| e.to_string())?;
                    // It hands the address to the running browser and ends.
                    std::thread::spawn(move || {
                        let _ = c.wait();
                    });
                    return Ok("opened a window".into());
                }
            }
            // A Mac's browser came forward through AppKit; its page can wait.
            Err(_) if shown => return Ok("brought to the front".into()),
            Err(e) => log(format!("browser: couldn't bring it forward: {e}")),
        }
    }
    let mut spec = match known {
        Some((spec, _)) => spec,
        None => {
            let port = if port != 0 {
                port
            } else {
                free_port().ok_or("no free port for the browser")?
            };
            fresh(port)?
        }
    };
    if port_answers(spec.port) {
        return Err(format!(
            "something else answers on the browser's port {}",
            spec.port
        ));
    }
    // Asked for: not hidden this time.
    spec.hidden = false;
    launch(home, spec, log)?;
    Ok("started it again".into())
}

// --- a Mac ---------------------------------------------------------------------

/// myous for Mac's browser (Browser.m): the browsers as Launch Services
/// knows them, the sandbox probe, a container worker's browser kept
/// running while the worker runs, and hide and show through AppKit.
#[cfg(target_os = "macos")]
pub mod mac {
    use super::*;
    use objc2::rc::Retained;
    use objc2::{msg_send, ClassType};
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
    use objc2_foundation::{NSBundle, NSString};

    /// The names myous for Mac gives them (Browser.m's `known`).
    fn long_name(id: &str) -> &'static str {
        match id {
            "com.google.Chrome" => "Google Chrome",
            "com.microsoft.edgemac" => "Microsoft Edge",
            "com.brave.Browser" => "Brave",
            _ => "Chromium",
        }
    }

    /// Where Launch Services has the app with this bundle id: its program.
    fn program_for(id: &str) -> Option<String> {
        let url = NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(id))?;
        let bundle = NSBundle::bundleWithURL(&url)?;
        let exe = bundle.executableURL()?.path()?;
        Some(exe.to_string())
    }

    /// The browsers this Mac has, in myous for Mac's order: where Launch
    /// Services has them, else in /Applications or ~/Applications.
    pub fn installed() -> Vec<Found> {
        MAC.iter()
            .filter_map(|(id, app, exe, _)| {
                let path = program_for(id)
                    .filter(|p| Path::new(p).is_file())
                    .or_else(|| {
                        [
                            PathBuf::from("/Applications"),
                            paths::user_home().join("Applications"),
                        ]
                        .into_iter()
                        .map(|d| d.join(app).join("Contents").join("MacOS").join(exe))
                        .find(|p| p.is_file())
                        .map(|p| p.to_string_lossy().to_string())
                    })?;
                Some(Found {
                    id: id.to_string(),
                    name: long_name(id).into(),
                    path,
                })
            })
            .collect()
    }

    /// The browser with this bundle id if it is installed, else the first
    /// one that is (Browser.m's `find:`).
    pub fn find(id: &str) -> Option<Found> {
        pick(installed(), id)
    }

    fn pick(all: Vec<Found>, id: &str) -> Option<Found> {
        let chosen = all.iter().position(|b| b.id == id).unwrap_or(0);
        all.into_iter().nth(chosen)
    }

    /// Whether this macOS can confine the browser: sandbox-exec runs a
    /// small profile (asked once).
    pub fn can_sandbox() -> bool {
        static OK: OnceLock<bool> = OnceLock::new();
        *OK.get_or_init(|| {
            let mut cmd = crate::engine::command(SANDBOX_EXEC);
            cmd.args(["-p", SANDBOX_PROBE, "/usr/bin/true"]);
            run_capture(cmd, Duration::from_secs(10)).0 == 0
        })
    }

    /// What Set up and Settings offer for a worker in a container.
    #[derive(Serialize, Clone, Debug, Default)]
    pub struct Choices {
        /// The browsers it can run, in order (the first is recommended).
        pub installed: Vec<Found>,
        /// Why "On this Mac" can't be offered ("" when it can).
        pub why: String,
    }

    pub fn choices() -> Choices {
        let installed = installed();
        let why = unavailable_reason(!installed.is_empty(), can_sandbox()).unwrap_or("");
        Choices {
            installed,
            why: why.into(),
        }
    }

    fn running_app(pid: u32) -> Option<Retained<NSRunningApplication>> {
        let pid = i32::try_from(pid).ok().filter(|p| *p > 0)?;
        unsafe {
            msg_send![
                NSRunningApplication::class(),
                runningApplicationWithProcessIdentifier: pid
            ]
        }
    }

    /// Out of the way, like ⌘H: still running (its launch switches keep
    /// its pages working).
    pub fn hide(pid: u32) {
        if let Some(app) = running_app(pid) {
            app.hide();
        }
    }

    /// Out of hiding, every window to the front; false when no app has
    /// that pid.
    #[allow(deprecated)]
    pub fn show(pid: u32) -> bool {
        let Some(app) = running_app(pid) else {
            return false;
        };
        app.unhide();
        app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        true
    }

    /// The DevTools port opens before the window exists, and the window
    /// may come to the front after it shows: hidden now and again every
    /// half second for the first six, unless the owner asked to see it
    /// meanwhile (Browser.m's `hideWhenReady:`).
    pub(super) fn hide_while_starting(key: String, gen: u64, pid: u32) {
        std::thread::spawn(move || {
            for _ in 0..13 {
                let wanted = procs()
                    .lock()
                    .unwrap()
                    .get(&key)
                    .is_some_and(|p| p.gen == gen && p.pid == pid && p.spec.hidden && !p.shown);
                if !wanted {
                    return;
                }
                hide(pid);
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    }

    /// The browser of a worker in a container, started if this app doesn't
    /// run it (or take over the one a run before left), as myous for Mac's
    /// `ensureMacBrowser:`: called while the worker starts and runs, so a
    /// browser the owner quit comes back. `app` is app.json's
    /// `browser_app` ("" the first installed). What it did, in words.
    pub fn ensure(
        home: &Path,
        worker: &str,
        app: &str,
        hidden: bool,
        log: Log,
    ) -> Result<String, String> {
        let key = home.to_string_lossy().to_string();
        if let Some(p) = procs().lock().unwrap().get_mut(&key) {
            if p.pid != 0 {
                p.spec.hidden = hidden;
                return Ok("running".into());
            }
        }
        let all = installed();
        if let Some(why) = unavailable_reason(!all.is_empty(), can_sandbox()) {
            return Err(cant_run(why));
        }
        let found = pick(all, app).ok_or_else(|| cant_run("no browser it can run is installed"))?;
        if !app.is_empty() && found.id != app {
            log(format!(
                "browser: {app} isn't installed; using {}",
                found.name
            ));
        }
        let how = How {
            container: true,
            sandbox: true,
            hidden,
        };
        let s = start_with_how(home, worker, &found, "", Vec::new(), how, log)?;
        Ok(format!("{} on port {}", s.app, s.port))
    }

    /// Show browser for a worker in a container: to the front, or started
    /// (not hidden) if it isn't running.
    pub fn show_container(
        home: &Path,
        worker: &str,
        app: &str,
        log: Log,
    ) -> Result<String, String> {
        let fresh = |port: u16| -> Result<Spec, String> {
            let all = installed();
            if let Some(why) = unavailable_reason(!all.is_empty(), can_sandbox()) {
                return Err(cant_run(why));
            }
            let found = pick(all, app).ok_or_else(|| cant_run("no browser"))?;
            let how = How {
                container: true,
                sandbox: true,
                hidden: false,
            };
            prepare_with(home, worker, &found, port, "", Vec::new(), &how)
        };
        show_with(home, &fresh, log)
    }

    /// Settings' "Start this worker's browser hidden", applied at once:
    /// hidden now, or shown.
    pub fn set_hidden(home: &Path, hidden: bool) {
        let key = home.to_string_lossy().to_string();
        let pid = {
            let mut map = procs().lock().unwrap();
            let Some(p) = map.get_mut(&key) else { return };
            p.spec.hidden = hidden;
            p.shown = !hidden;
            p.pid
        };
        if pid == 0 {
            return;
        }
        if hidden {
            hide(pid);
        } else {
            show(pid);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn picks_the_chosen_browser_else_the_first() {
            let b = |id: &str| Found {
                id: id.into(),
                name: long_name(id).into(),
                path: format!("/Applications/{id}"),
            };
            let all = vec![b("com.google.Chrome"), b("com.brave.Browser")];
            assert_eq!(
                pick(all.clone(), "com.brave.Browser").unwrap().name,
                "Brave"
            );
            assert_eq!(pick(all.clone(), "").unwrap().name, "Google Chrome");
            assert_eq!(
                pick(all, "com.microsoft.edgemac").unwrap().id,
                "com.google.Chrome"
            );
            assert_eq!(pick(Vec::new(), ""), None);
        }

        /// The real thing on this Mac, by hand: `cargo test -- --ignored
        /// real_browser --nocapture`. A container worker's browser for a
        /// temporary worker folder, under the sandbox: browser.json and its
        /// DevTools port, hidden after launch, shown and hidden again,
        /// Show browser, the sandbox keeping it to its own folder,
        /// then stopped.
        #[test]
        #[ignore]
        fn real_browser() {
            // In the owner's home, as a worker folder is (~/.myous-worker).
            let home = paths::user_home()
                .join("Library/Caches")
                .join(format!("myous-real-browser-{}", std::process::id()));
            let _ = fs::remove_dir_all(&home);
            fs::create_dir_all(&home).unwrap();
            let log: Log = Arc::new(|l| println!("{l}"));
            // Stopped however the test ends.
            struct Stop(PathBuf);
            impl Drop for Stop {
                fn drop(&mut self) {
                    crate::browser::stop(&self.0);
                }
            }
            let _stop = Stop(home.clone());
            let started = ensure(&home, "Real Test", "", true, log.clone()).unwrap();
            println!("ensure: {started}");
            let json = paths::browser_json(&home);
            let t = Instant::now();
            while !json.exists() && t.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(200));
            }
            let j: Value = serde_json::from_slice(&fs::read(&json).expect("browser.json")).unwrap();
            println!("browser.json: {j}");
            assert_eq!(j["host"], "host.docker.internal");
            assert!(j.get("kind").is_none());
            let port = j["port"].as_u64().unwrap() as u16;
            let pid = j["pid"].as_u64().unwrap() as u32;
            println!(
                "/json/version: {}",
                get_text(&format!("http://127.0.0.1:{port}/json/version")).unwrap()
            );
            assert!(fs::read_to_string(home.join("browser.sb"))
                .unwrap()
                .contains("(deny process-exec*)"));
            let hidden = || running_app(pid).map(|a| a.isHidden()).unwrap_or(false);
            std::thread::sleep(Duration::from_secs(5));
            assert!(running_app(pid).is_some(), "an app for pid {pid}");
            assert!(hidden(), "hidden after launch");
            set_hidden(&home, false);
            std::thread::sleep(Duration::from_secs(1));
            assert!(!hidden(), "shown");
            set_hidden(&home, true);
            std::thread::sleep(Duration::from_secs(1));
            assert!(hidden(), "hidden again");
            let t = Instant::now();
            assert_eq!(
                show_container(&home, "Real Test", "", log.clone()).unwrap(),
                "brought to the front"
            );
            println!("Show browser took {:?}", t.elapsed());
            std::thread::sleep(Duration::from_secs(1));
            assert!(!hidden(), "Show browser shows it");
            assert_eq!(
                ensure(&home, "Real Test", "", true, log.clone()).unwrap(),
                "running"
            );
            // The sandbox: its start page loads; the worker folder's other
            // files, in the owner's home, don't (a page that can't load is
            // titled with its address).
            let open = |url: &str| -> String {
                let r = local_agent(Duration::from_secs(5))
                    .put(&format!("http://127.0.0.1:{port}/json/new?{url}"))
                    .send_empty()
                    .unwrap()
                    .body_mut()
                    .read_to_string()
                    .unwrap();
                let id = serde_json::from_str::<Value>(&r).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                std::thread::sleep(Duration::from_secs(2));
                let list: Value = serde_json::from_str(
                    &get_text(&format!("http://127.0.0.1:{port}/json/list")).unwrap(),
                )
                .unwrap();
                let title = list
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["id"] == id.as_str())
                    .map(|t| t["title"].as_str().unwrap_or("").to_string())
                    .unwrap_or_default();
                println!("{url}: {title:?}");
                title
            };
            let own = open(&file_url(&paths::browser_dir(&home).join("start.html")));
            let sb = file_url(&home.join("browser.sb"));
            assert_eq!(own, "myous · Real Test");
            assert_eq!(open(&sb), sb);
            crate::browser::stop(&home);
            assert!(!json.exists() && !port_answers(port) && !pid_alive(pid));
            fs::remove_dir_all(&home).unwrap();
        }

        /// What this Mac has, printed; the probe answers either way.
        #[test]
        fn this_mac() {
            println!("browsers here: {:?}; sandbox: {}", choices(), can_sandbox());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("myous-br-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn no_which(_: &str) -> Option<PathBuf> {
        None
    }
    fn no_app_path(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn finds_edge_then_chrome_on_windows() {
        let pf = "/fake/Program Files";
        let x86 = "/fake/Program Files (x86)";
        let local = "/fake/AppData/Local";
        let var = |k: &str| match k {
            "ProgramFiles" => Some(pf.to_string()),
            "ProgramFiles(x86)" => Some(x86.to_string()),
            "LOCALAPPDATA" => Some(local.to_string()),
            _ => None,
        };
        let edge = Path::new(x86).join(r"Microsoft\Edge\Application\msedge.exe");
        let chrome = Path::new(local).join(r"Google\Chrome\Application\chrome.exe");
        let both = |p: &Path| p == edge || p == chrome;
        let f = find_browser_with("windows", &no_which, &var, &both, &no_app_path).unwrap();
        assert_eq!((f.id.as_str(), f.name.as_str()), ("msedge", "Edge"));
        assert_eq!(f.path, edge.to_string_lossy());
        let only_chrome = |p: &Path| p == chrome;
        let f = find_browser_with("windows", &no_which, &var, &only_chrome, &no_app_path).unwrap();
        assert_eq!(
            (f.id.as_str(), f.path.clone()),
            ("chrome", chrome.to_string_lossy().to_string())
        );
        // Elsewhere, where the registry's App Paths say.
        let odd = r"D:\Apps\Edge\msedge.exe";
        let reg = |exe: &str| (exe == "msedge.exe").then(|| odd.to_string());
        let there = |p: &Path| p == Path::new(odd);
        let f = find_browser_with("windows", &no_which, &var, &there, &reg).unwrap();
        assert_eq!(f.path, odd);
        assert_eq!(
            find_browser_with("windows", &no_which, &var, &|_| false, &reg),
            None
        );
        // The development override wins.
        let var2 = |k: &str| (k == "MYOUS_BROWSER_BIN").then(|| "/opt/cft/chrome".to_string());
        let f = find_browser_with("windows", &no_which, &var2, &|_| true, &no_app_path).unwrap();
        assert_eq!(
            (f.id.as_str(), f.path.as_str()),
            ("custom", "/opt/cft/chrome")
        );
    }

    #[test]
    fn finds_a_browser_on_a_linux_path_but_not_a_snap() {
        let dir = temp("linux");
        // A Linux PATH lookup, whatever the host (which_in applies the host's rules).
        let which = |n: &str| {
            let p = dir.join(n);
            p.is_file().then_some(p)
        };
        let none = |_: &str| None;
        assert_eq!(
            find_browser_with("linux", &which, &none, &|p| p.is_file(), &no_app_path),
            None
        );
        // Ubuntu's chromium-browser: a script that runs the snap. Passed over.
        fs::write(
            dir.join("chromium-browser"),
            "#!/bin/sh\nexec /snap/bin/chromium \"$@\"\n",
        )
        .unwrap();
        assert_eq!(
            find_browser_with("linux", &which, &none, &|p| p.is_file(), &no_app_path),
            None
        );
        fs::write(dir.join("microsoft-edge"), "binary").unwrap();
        let f = find_browser_with("linux", &which, &none, &|p| p.is_file(), &no_app_path).unwrap();
        assert_eq!((f.id.as_str(), f.name.as_str()), ("msedge", "Edge"));
        fs::write(dir.join("google-chrome-stable"), "binary").unwrap();
        let f = find_browser_with("linux", &which, &none, &|p| p.is_file(), &no_app_path).unwrap();
        assert_eq!((f.id.as_str(), f.name.as_str()), ("chrome", "Chrome"));
        assert_eq!(f.path, dir.join("google-chrome-stable").to_string_lossy());
        fs::remove_dir_all(&dir).unwrap();
        // A Mac: the apps in /Applications, else in ~/Applications, in
        // myous for Mac's order; the profile folder is the bundle id.
        let mac = |p: &Path| p.to_string_lossy().contains("Microsoft Edge.app");
        let f = find_browser_with("macos", &no_which, &none, &mac, &no_app_path).unwrap();
        assert_eq!(
            (f.id.as_str(), f.name.as_str(), f.path.as_str()),
            (
                "com.microsoft.edgemac",
                "Edge",
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"
            )
        );
        let home = |k: &str| (k == "HOME").then(|| "/Users/max".to_string());
        let both = |p: &Path| {
            let p = p.to_string_lossy();
            p == "/Users/max/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
                || p == "/Applications/Chromium.app/Contents/MacOS/Chromium"
        };
        let f = find_browser_with("macos", &no_which, &home, &both, &no_app_path).unwrap();
        assert_eq!(
            (f.id.as_str(), f.name.as_str()),
            ("com.brave.Browser", "Brave")
        );
        assert!(f.path.starts_with("/Users/max/Applications/"));
        assert_eq!(
            find_browser_with("macos", &no_which, &home, &|_| false, &no_app_path),
            None
        );
    }

    #[test]
    fn reads_the_registrys_answer() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\msedge.exe\r\n    (Default)    REG_SZ    C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe\r\n\r\n";
        assert_eq!(
            parse_reg_default(out).unwrap(),
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
        );
        assert_eq!(
            parse_reg_default("    (Standard)    REG_SZ    \"C:\\x\\chrome.exe\""),
            Some(r"C:\x\chrome.exe".into())
        );
        assert_eq!(
            parse_reg_default(
                "ERROR: The system was unable to find the specified registry key or value."
            ),
            None
        );
    }

    #[test]
    fn finds_python_3_9_or_newer() {
        let at = |n: &str| Some(PathBuf::from(format!("/usr/bin/{n}")));
        // Windows: py -3 first; one too old is passed over (and said).
        let probe = |p: &Path, args: &[String]| -> Option<String> {
            match (p.to_string_lossy().as_ref(), args[0].as_str()) {
                ("/usr/bin/py", "-3") => Some("C:\\Python39\\python.exe\n3.9\nTrue".into()),
                ("/usr/bin/python", "-c") => Some("C:\\Python312\\python.exe\n3.12\nTrue".into()),
                _ => None,
            }
        };
        let (py, old) = find_python_with("windows", &at, &probe);
        assert_eq!(
            py.unwrap(),
            Python {
                path: r"C:\Python312\python.exe".into(),
                version: "3.12".into(),
                venv: true
            }
        );
        assert_eq!(old, "3.9");
        // The probe is run as `-c PROBE`, after py's -3.
        let seen = std::cell::RefCell::new(Vec::new());
        let record = |p: &Path, args: &[String]| -> Option<String> {
            seen.borrow_mut().push((p.to_path_buf(), args.to_vec()));
            None
        };
        assert_eq!(
            find_python_with("windows", &at, &record),
            (None, String::new())
        );
        let seen = seen.into_inner();
        assert_eq!(
            seen.iter()
                .map(|(p, _)| p.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            ["/usr/bin/py", "/usr/bin/python", "/usr/bin/python3"]
        );
        assert_eq!(&seen[0].1[..2], &["-3".to_string(), "-c".to_string()]);
        // Elsewhere python3 first; Debian without python3-venv says so.
        let deb = |p: &Path, _: &[String]| -> Option<String> {
            (p == Path::new("/usr/bin/python3")).then(|| "/usr/bin/python3\n3.11\nFalse".into())
        };
        let (py, _) = find_python_with("linux", &at, &deb);
        assert_eq!(
            py.unwrap(),
            Python {
                path: "/usr/bin/python3".into(),
                version: "3.11".into(),
                venv: false
            }
        );
        assert!(
            version_ok("3.10")
                && version_ok("3.13")
                && version_ok("4.0")
                && !version_ok("3.9")
                && !version_ok("2.7")
        );
        // Elsewhere, a versioned name when python3 is too old (an old
        // distribution's 3.9, Homebrew's 3.13).
        let mac = |p: &Path, _: &[String]| -> Option<String> {
            match p.to_string_lossy().as_ref() {
                "/usr/bin/python3" => Some("/usr/bin/python3\n3.9\nTrue".into()),
                "/usr/bin/python3.13" => Some("/opt/py/python3.13\n3.13\nTrue".into()),
                _ => None,
            }
        };
        let (py, old) = find_python_with("linux", &at, &mac);
        assert_eq!(
            (py.unwrap().version.as_str(), old.as_str()),
            ("3.13", "3.9")
        );
        // A Mac never runs /usr/bin/python3 (the Command Line Tools' 3.9,
        // or their install dialog).
        let (py, old) = find_python_with("macos", &at, &mac);
        assert_eq!((py.unwrap().version.as_str(), old.as_str()), ("3.13", ""));
    }

    /// A real Python on a test PATH: a script standing in for it, found by
    /// its full path and run.
    #[cfg(unix)]
    #[test]
    fn runs_python_from_a_test_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("py");
        let fake = dir.join("python3");
        fs::write(
            &fake,
            "#!/bin/sh\necho /opt/py/bin/python3.12\necho 3.12\necho True\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&dir]).unwrap();
        let (py, _) = find_python_with(
            "linux",
            &|n| crate::engine::which_in(n, &path),
            &probe_python,
        );
        assert_eq!(py.unwrap().path, "/opt/py/bin/python3.12");
        // The environment: Playwright there or not, as its Python says.
        let envdir = dir.join("env");
        let envpy = env_python_in(&envdir, "linux");
        assert_eq!(playwright_version(&envpy), "");
        assert!(!env_works(&envpy));
        fs::create_dir_all(envpy.parent().unwrap()).unwrap();
        fs::write(&envpy, "#!/bin/sh\necho 1.63.0\n").unwrap();
        fs::set_permissions(&envpy, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(playwright_version(&envpy), "1.63.0");
        assert!(env_works(&envpy));
        fs::write(
            &envpy,
            "#!/bin/sh\necho 'No module named playwright' >&2; exit 1\n",
        )
        .unwrap();
        assert_eq!(playwright_version(&envpy), "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_makes_the_environment_then_pip() {
        let dir = temp("env");
        let _ = fs::remove_dir_all(&dir);
        let py = Python {
            path: "/usr/bin/python3.12".into(),
            version: "3.12".into(),
            venv: true,
        };
        let steps = install_steps(Some(&py), &dir, "linux", false).unwrap();
        let d = dir.to_string_lossy().to_string();
        assert_eq!(
            steps[0],
            vec!["/usr/bin/python3.12", "-m", "venv", d.as_str()]
        );
        assert_eq!(
            steps[1],
            vec![
                env_python_in(&dir, "linux").to_string_lossy().as_ref(),
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-input",
                "playwright==1.63.0"
            ]
        );
        assert!(
            !steps
                .concat()
                .iter()
                .any(|a| a == "playwright" || a.contains("install chromium")),
            "no browser download"
        );
        // There but broken: made again (--clear); there and working: pip alone.
        fs::create_dir_all(&dir).unwrap();
        assert!(install_steps(Some(&py), &dir, "linux", false).unwrap()[0]
            .contains(&"--clear".to_string()));
        assert_eq!(install_steps(None, &dir, "linux", true).unwrap().len(), 1);
        // Windows: Scripts\python.exe.
        let w = install_steps(Some(&py), &dir, "windows", false).unwrap();
        assert!(w[1][0].ends_with("python.exe") && w[1][0].contains("Scripts"));
        // No Python, or one without venv: said, nothing run.
        assert!(install_steps(None, &dir, "linux", false)
            .unwrap_err()
            .contains("no Python"));
        let deb = Python { venv: false, ..py };
        assert!(install_steps(Some(&deb), &dir, "linux", false)
            .unwrap_err()
            .contains("venv"));
        fs::remove_dir_all(&dir).unwrap();
    }

    /// The pin is the container's, so scripts behave the same in both.
    #[test]
    fn playwright_is_the_containers() {
        let dockerfile = include_str!("../../../Dockerfile");
        assert!(
            dockerfile.contains(&format!("playwright=={PLAYWRIGHT}")),
            "worker/Dockerfile pins another Playwright"
        );
    }

    #[test]
    fn python_lines_and_links() {
        let apt = |n: &str| n == "apt-get";
        assert_eq!(
            python_lines("linux", &apt),
            (
                "sudo apt install python3 python3-venv".into(),
                "sudo apt install python3-venv".into()
            )
        );
        assert_eq!(
            python_lines("linux", &|n| n == "dnf").0,
            "sudo dnf install python3"
        );
        assert_eq!(
            python_lines("linux", &|_| false),
            (String::new(), String::new())
        );
        assert_eq!(
            python_lines("windows", &apt),
            (String::new(), String::new())
        );
        assert_eq!(get_links("windows")[0].name, "Edge");
        assert_eq!(get_links("linux")[0].name, "Chrome");
    }

    #[test]
    fn file_urls() {
        assert_eq!(
            file_url(Path::new("/home/max/.myous-worker/browser/start.html")),
            "file:///home/max/.myous-worker/browser/start.html"
        );
        assert_eq!(
            file_url(Path::new(
                r"C:\Users\Max Felix\AppData\Local\myous\worker\browser\start.html"
            )),
            "file:///C:/Users/Max%20Felix/AppData/Local/myous/worker/browser/start.html"
        );
    }

    #[test]
    fn the_command_line_and_the_files() {
        let home = temp("spec");
        let found = Found {
            id: "msedge".into(),
            name: "Edge".into(),
            path: "/opt/edge/msedge".into(),
        };
        let spec = prepare(
            &home,
            "Max's <PC>",
            &found,
            41234,
            "/env/bin/python",
            vec![],
        )
        .unwrap();
        assert_eq!(spec.profile, home.join("browser").join("msedge"));
        let a = args(&spec, "windows");
        assert_eq!(
            a[0],
            format!(
                "--user-data-dir={}",
                home.join("browser").join("msedge").display()
            )
        );
        assert_eq!(a[1], "--remote-debugging-port=41234");
        for flag in [
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-backgrounding-occluded-windows",
        ] {
            assert!(a.contains(&flag.to_string()), "{flag}");
        }
        assert!(!a.iter().any(|x| x == "--remote-debugging-port=0"
            || x.contains("headless")
            || x == "--use-mock-keychain"));
        assert_eq!(
            &a[a.len() - 2..],
            &["--new-window".to_string(), spec.start_url.clone()]
        );
        assert!(args(&spec, "macos").contains(&"--use-mock-keychain".to_string()));
        // The start page, its name escaped; the profile's first settings.
        let page = fs::read_to_string(home.join("browser").join("start.html")).unwrap();
        assert!(
            page.contains("Max's &lt;PC&gt;'s browser")
                && page.contains("never sees your own browser"),
            "{page}"
        );
        assert!(
            spec.start_url.starts_with("file:///")
                && spec.start_url.ends_with("/browser/start.html")
        );
        let prefs: Value = serde_json::from_str(
            &fs::read_to_string(spec.profile.join("Default/Preferences")).unwrap(),
        )
        .unwrap();
        assert_eq!(prefs["profile"]["name"], "Max's <PC> · myous");
        assert_eq!(prefs["homepage"], spec.start_url.as_str());
        assert_eq!(
            prefs["download"]["default_directory"],
            home.join("work")
                .join("downloads")
                .to_string_lossy()
                .as_ref()
        );
        assert!(home.join("work").join("downloads").is_dir());
        // Seeded once: the owner's later changes stay.
        fs::write(spec.profile.join("Default/Preferences"), "{\"mine\":1}").unwrap();
        prepare(&home, "Other", &found, 1, "", vec![]).unwrap();
        assert_eq!(
            fs::read_to_string(spec.profile.join("Default/Preferences")).unwrap(),
            "{\"mine\":1}"
        );
        // browser.json: the Mac's keys and this one's.
        let j = browser_json(&spec, 777);
        assert_eq!(
            (
                j["host"].as_str(),
                j["port"].as_u64(),
                j["pid"].as_u64(),
                j["app"].as_str(),
                j["kind"].as_str(),
                j["python"].as_str()
            ),
            (
                Some("127.0.0.1"),
                Some(41234),
                Some(777),
                Some("Edge"),
                Some("host"),
                Some("/env/bin/python")
            )
        );
        assert!(j["at"].as_u64().unwrap() > 1_700_000_000);
        fs::remove_dir_all(&home).unwrap();
    }

    /// A `stringWithFormat:` in myous for Mac's Browser.m, after `marker`:
    /// its format (the literals joined and unescaped, comments skipped)
    /// and its arguments as written.
    fn objc_format(src: &str, marker: &str) -> (String, Vec<String>) {
        let body = &src[src.find(marker).expect(marker)..];
        let at = body.find("stringWithFormat:").unwrap() + "stringWithFormat:".len();
        let end = at + body[at..].find("];").unwrap();
        let mut chars = body[at..end].chars().peekable();
        let (mut text, mut rest) = (String::new(), String::new());
        let (mut in_str, mut done) = (false, false);
        while let Some(c) = chars.next() {
            if done {
                rest.push(c);
            } else if in_str {
                match c {
                    '\\' => match chars.next() {
                        Some('n') => text.push('\n'),
                        Some(o) => text.push(o),
                        None => {}
                    },
                    '"' => in_str = false,
                    _ => text.push(c),
                }
            } else if c == '"' {
                in_str = true;
            } else if c == '/' && chars.peek() == Some(&'/') {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            } else if c == ',' {
                done = true;
            }
        }
        (
            text,
            rest.split(',').map(|a| a.trim().to_string()).collect(),
        )
    }

    /// `%@` by `%@`, the values given.
    fn fill(format: &str, values: &[&str]) -> String {
        assert_eq!(format.matches("%@").count(), values.len(), "{format}");
        let mut parts = format.split("%@");
        let mut out = parts.next().unwrap().to_string();
        for (part, v) in parts.zip(values) {
            out.push_str(v);
            out.push_str(part);
        }
        out
    }

    fn browser_m() -> Option<String> {
        let m = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mac/src/Browser.m");
        let src = fs::read_to_string(&m).ok();
        if src.is_none() {
            println!("no {}: the profile here is the one", m.display());
        }
        src
    }

    /// The sandbox is myous for Mac's, word for word: its profile, its
    /// probe, the start page that explains Chrome's bar.
    #[test]
    fn the_sandbox_is_myous_for_macs() {
        let Some(src) = browser_m() else { return };
        let (format, args) = objc_format(&src, "- (NSString *)seatbeltFor:");
        let values: Vec<&str> = args
            .iter()
            .map(|a| match a.as_str() {
                "home" => "/Users/max",
                "[self.paths browserDir]" => "/Users/max/.myous-worker/browser",
                "[self.paths downloads]" => "/Users/max/.myous-worker/downloads",
                "browser[@\"id\"]" => "com.google.Chrome",
                "browser[@\"bundle\"]" => "/Applications/Google Chrome.app",
                other => panic!("Browser.m's profile takes {other}"),
            })
            .collect();
        assert_eq!(
            seatbelt(
                "/Users/max",
                "/Users/max/.myous-worker/browser",
                "/Users/max/.myous-worker/downloads",
                "com.google.Chrome",
                "/Applications/Google Chrome.app"
            ),
            fill(&format, &values)
        );
        // The probe.
        let probe = src
            .split("@\"-p\", @\"")
            .nth(1)
            .and_then(|s| s.split("\", @\"/usr/bin/true\"").next())
            .unwrap()
            .replace("\\\"", "\"");
        assert_eq!(SANDBOX_PROBE, probe);
        // The start page, the name escaped.
        let (format, args) = objc_format(&src, "- (NSString *)startPage");
        assert!(args.iter().all(|a| a == "esc"), "{args:?}");
        let esc = "Max's &lt;Mac&gt;";
        assert_eq!(
            start_page_sandboxed("Max's <Mac>"),
            fill(&format, &[esc, esc, esc])
        );
    }

    #[test]
    fn a_macs_browser_and_why_not() {
        assert_eq!(
            bundle_of(Path::new(
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
            )),
            PathBuf::from("/Applications/Google Chrome.app")
        );
        assert_eq!(
            bundle_of(Path::new("/opt/cft/chrome")),
            PathBuf::from("/opt/cft")
        );
        assert_eq!(
            unavailable_reason(false, true),
            Some("no browser it can run is installed")
        );
        assert_eq!(
            unavailable_reason(true, false),
            Some("this macOS can't sandbox it")
        );
        assert_eq!(unavailable_reason(true, true), None);
        assert_eq!(
            cant_run("this macOS can't sandbox it"),
            "Can't run the browser on this Mac: this macOS can't sandbox it. Install Google Chrome, Microsoft Edge, Brave or Chromium (all free), or choose the browser in the container."
        );
    }

    /// A Mac's browser for a worker in a container: under sandbox-exec with
    /// the profile, myous for Mac's switches and browser.json, downloads
    /// in the worker folder.
    #[test]
    fn a_container_workers_browser_on_a_mac() {
        let home = temp("mac");
        let found = Found {
            id: "com.google.Chrome".into(),
            name: "Google Chrome".into(),
            path: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
        };
        let how = How {
            container: true,
            sandbox: true,
            hidden: true,
        };
        let spec = prepare_with(&home, "Max's Mac", &found, 41234, "", vec![], &how).unwrap();
        let sb = home.join("browser.sb");
        assert_eq!(spec.sandbox.as_deref(), Some(sb.as_path()));
        assert!(spec.container && spec.hidden);
        let real = |p: &Path| p.canonicalize().unwrap().to_string_lossy().to_string();
        assert_eq!(
            fs::read_to_string(&sb).unwrap(),
            seatbelt(
                &paths::user_home()
                    .canonicalize()
                    .unwrap_or_else(|_| paths::user_home())
                    .to_string_lossy(),
                &real(&home.join("browser")),
                &real(&home.join("downloads")),
                "com.google.Chrome",
                "/Applications/Google Chrome.app"
            )
        );
        let cmd = browser_command(&spec);
        assert_eq!(cmd.get_program(), SANDBOX_EXEC);
        let first: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            first,
            [
                "-f".to_string(),
                sb.to_string_lossy().to_string(),
                found.path.clone()
            ]
        );
        let a = args(&spec, "macos");
        for flag in [
            "--no-sandbox".to_string(),
            "--use-mock-keychain".to_string(),
            format!("--disk-cache-dir={}", spec.profile.join("cache").display()),
            "--window-size=1200,800".to_string(),
            "--remote-debugging-port=41234".to_string(),
        ] {
            assert!(a.contains(&flag), "{flag}: {a:?}");
        }
        assert_eq!(a.last(), Some(&spec.start_url));
        // The page that explains Chrome's bar; downloads where the container sees them.
        let page = fs::read_to_string(home.join("browser").join("start.html")).unwrap();
        assert!(page.contains("That bar is expected"), "{page}");
        let prefs: Value = serde_json::from_str(
            &fs::read_to_string(spec.profile.join("Default/Preferences")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            prefs["download"]["default_directory"],
            home.join("downloads").to_string_lossy().as_ref()
        );
        // browser.json: myous for Mac's, for forward.py; no kind, so the
        // worker in the container doesn't take it for a browser beside it.
        let j = browser_json(&spec, 777);
        assert_eq!(j["host"], "host.docker.internal");
        assert_eq!(j["port"].as_u64(), Some(41234));
        assert_eq!(j["pid"].as_u64(), Some(777));
        assert_eq!(j["app"], "Google Chrome");
        assert!(j.get("kind").is_none() && j.get("python").is_none());
        // Linux and Windows: none of it.
        let plain = prepare(&home, "Max's Mac", &found, 41234, "", vec![]).unwrap();
        assert_eq!(plain.sandbox, None);
        assert!(!plain.container && !plain.hidden);
        assert!(!args(&plain, "linux").contains(&"--no-sandbox".to_string()));
        assert_eq!(browser_command(&plain).get_program(), found.path.as_str());
        assert_eq!(browser_json(&plain, 1)["kind"], "host");
        fs::remove_dir_all(&home).unwrap();
    }

    /// Launching, browser.json, Show and Stop, with a stand-in for the
    /// browser: a script that serves the DevTools addresses Show uses on
    /// the port it is given (Python's http.server).
    #[cfg(unix)]
    #[test]
    fn launch_show_and_stop_with_a_stand_in() {
        use std::os::unix::fs::PermissionsExt;
        let Some(python) = crate::engine::which("python3") else {
            println!("no python3 here; skipped");
            return;
        };
        let home = temp("run");
        let www = home.join("www");
        fs::create_dir_all(www.join("json/activate")).unwrap();
        fs::write(www.join("json/version"), r#"{"Browser": "Stand-in/1"}"#).unwrap();
        fs::write(
            www.join("json/list"),
            r#"[{"id": "P1", "type": "page", "url": "file:///start.html"}]"#,
        )
        .unwrap();
        fs::write(www.join("json/activate/P1"), "Target activated").unwrap();
        let exe = home.join("fake-browser");
        fs::write(
            &exe,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --remote-debugging-port=*) port=${{a#*=}};; esac; done\n\
                 echo \"args: $*\"\nexec {} -m http.server \"$port\" --bind 127.0.0.1 --directory {}\n",
                python.display(),
                www.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let found = Found {
            id: "custom".into(),
            name: "Stand-in".into(),
            path: exe.to_string_lossy().to_string(),
        };
        let lines = Arc::new(Mutex::new(Vec::<String>::new()));
        let l2 = lines.clone();
        let log: Log = Arc::new(move |s| l2.lock().unwrap().push(s));
        let st = start_with(&home, "Test PC", &found, "/env/python", vec![], log.clone()).unwrap();
        assert!(running(&home));
        let json_file = paths::browser_json(&home);
        let t = Instant::now();
        while !json_file.exists() && t.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let j: Value =
            serde_json::from_slice(&fs::read(&json_file).expect("browser.json")).unwrap();
        assert_eq!(j["port"].as_u64(), Some(st.port as u64));
        assert_eq!(j["kind"], "host");
        assert!(fs::read_to_string(paths::browser_log(&home))
            .unwrap()
            .contains("--new-window"));
        // A second start while it runs: the same browser.
        assert_eq!(
            start_with(&home, "Test PC", &found, "/env/python", vec![], log.clone())
                .unwrap()
                .port,
            st.port
        );
        // Show: the page is activated.
        assert_eq!(
            show(&home, "Test PC", log.clone()).unwrap(),
            "brought to the front"
        );
        // Stop: gone, and browser.json with it.
        stop(&home);
        assert!(!running(&home) && !json_file.exists());
        assert!(!port_answers(st.port));
        // Show after it was closed starts it again on the same port.
        procs()
            .lock()
            .unwrap()
            .remove(&home.to_string_lossy().to_string());
        let spec = prepare(&home, "Test PC", &found, st.port, "", vec![]).unwrap();
        launch(&home, spec, log.clone()).unwrap();
        let t = Instant::now();
        while !json_file.exists() && t.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(devtools_answers(st.port));
        // The app went away without stopping it: a new start takes it over.
        let taken = procs()
            .lock()
            .unwrap()
            .remove(&home.to_string_lossy().to_string())
            .unwrap();
        let again = start_with(&home, "Test PC", &found, "", vec![], log.clone()).unwrap();
        assert_eq!(again.port, st.port);
        assert!(
            lines
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("from before is still running")),
            "{:?}",
            lines.lock().unwrap()
        );
        stop(&home);
        let mut child = taken.child.unwrap();
        let t = Instant::now();
        while matches!(child.try_wait(), Ok(None)) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !matches!(child.try_wait(), Ok(None)),
            "the adopted browser was ended"
        );
        fs::remove_dir_all(&home).unwrap();
    }

    /// "Install Playwright" for real: an environment made with this
    /// computer's Python in a temporary folder, pip from PyPI, then the
    /// version asked: `cargo test -- --ignored playwright_install`.
    #[test]
    #[ignore]
    fn playwright_install() {
        let dir = temp("install").join("browser-python");
        let os = crate::platform::os();
        let python = match std::env::var("MYOUS_E2E_SYSTEM_PYTHON") {
            Ok(p) => probe_python(Path::new(&p), &["-c".into(), PROBE.into()])
                .map(|out| {
                    let l: Vec<&str> = out.lines().collect();
                    Python {
                        path: l[0].into(),
                        version: l[1].into(),
                        venv: l[2] == "True",
                    }
                })
                .expect("MYOUS_E2E_SYSTEM_PYTHON doesn't run"),
            Err(_) => find_python()
                .0
                .expect("no Python 3.10+ here (set MYOUS_E2E_SYSTEM_PYTHON)"),
        };
        println!("with {python:?}");
        let steps =
            install_steps(Some(&python), &dir, os, env_works(&env_python_in(&dir, os))).unwrap();
        for argv in steps {
            println!("$ {}", argv.join(" "));
            let st =
                crate::engine::run_streaming(&argv, &std::env::temp_dir(), &HashMap::new(), |l| {
                    println!("  {l}")
                });
            assert_eq!(st, 0, "{argv:?}");
        }
        assert_eq!(playwright_version(&env_python_in(&dir, os)), PLAYWRIGHT);
        // Again: the environment is there, pip alone.
        assert_eq!(
            install_steps(None, &dir, os, env_works(&env_python_in(&dir, os)))
                .unwrap()
                .len(),
            1
        );
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    /// What this computer has, printed (in CI, the runner's).
    #[test]
    fn checks_here() {
        let c = check();
        println!("browser check here: {c:?}");
        assert!(c.env_python.contains("browser-python"));
    }
}

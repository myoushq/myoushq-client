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

use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::engine::{hide, run_capture};
use crate::paths;

/// The Playwright for Python the app installs: the container's
/// (worker/Dockerfile), so agents' scripts behave the same in both.
pub const PLAYWRIGHT: &str = "1.63.0";

/// Where the app's lines go (the app log).
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// A browser the app can run.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Found {
    /// Its profile folder's name under `<home>/browser`: "msedge",
    /// "chrome", "chromium", "custom" ($MYOUS_BROWSER_BIN).
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

/// A Mac (only for running the app here while developing it).
const MAC: [(&str, &str, &str); 3] = [
    (
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "chrome",
        "Chrome",
    ),
    (
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "chromium",
        "Chromium",
    ),
    (
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "msedge",
        "Edge",
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
            for (path, id, name) in MAC {
                if is_file(Path::new(path)) {
                    return Some(found(id, name, Path::new(path)));
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
        let mut cmd = Command::new("reg");
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
    let mut cmd = Command::new(p);
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
    let mut cmd = Command::new(python);
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
    let mut cmd = Command::new(python);
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

/// The profile's settings, set once before it exists (the owner's later
/// choices stand): a name, avatar and toolbar colour that say "myous", the
/// start page as home, downloads into the work folder, where the agent
/// can fetch them (`myous cp`).
pub fn seed_preferences(profile: &Path, worker: &str, start_url: &str, downloads: &Path) {
    let def = profile.join("Default");
    let prefs = def.join("Preferences");
    if prefs.exists() {
        return;
    }
    let _ = fs::create_dir_all(&def);
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
    let _ = fs::write(&prefs, p.to_string());
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
    a.extend(s.extra.iter().cloned());
    a.push("--new-window".into());
    a.push(s.start_url.clone());
    a
}

/// Writes the start page and the profile's first settings; the folders
/// it needs. Each browser has its own profile (`<home>/browser/<id>`, as
/// on the Mac), so Edge and Chrome never share one.
pub fn prepare(
    home: &Path,
    worker: &str,
    found: &Found,
    port: u16,
    python: &str,
    extra: Vec<String>,
) -> Result<Spec, String> {
    let dir = paths::browser_dir(home);
    let profile = dir.join(&found.id);
    let downloads = home.join("work").join("downloads");
    for d in [&dir, &profile, &downloads] {
        paths::ensure(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let page = dir.join("start.html");
    fs::write(&page, start_page(worker)).map_err(|e| e.to_string())?;
    let start_url = file_url(&page);
    seed_preferences(&profile, worker, &start_url, &downloads);
    Ok(Spec {
        exe: PathBuf::from(&found.path),
        app: found.name.clone(),
        profile,
        start_url,
        port,
        python: python.into(),
        extra,
    })
}

/// browser.json: the keys myous for Mac writes (host, port, pid, app,
/// at), plus the Python with Playwright and `kind` "host" (on this
/// computer, no container in between).
pub fn browser_json(spec: &Spec, pid: u32) -> Value {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
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

/// The browser a browser.json names, if it is this app's kind and still
/// answers: (port, pid).
fn adoptable(home: &Path) -> Option<(u16, u32)> {
    let v: Value = serde_json::from_slice(&fs::read(paths::browser_json(home)).ok()?).ok()?;
    if v.get("kind")?.as_str()? != "host" || v.get("host")?.as_str()? != "127.0.0.1" {
        return None;
    }
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let pid = v.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    devtools_answers(port).then_some((port, pid))
}

/// Starts the program for `spec`, keeps it as this worker's browser and
/// watches it: browser.json once its port answers, removed when it ends.
pub fn launch(home: &Path, spec: Spec, log: Log) -> Result<u32, String> {
    let key = home.to_string_lossy().to_string();
    fs::create_dir_all(&spec.profile).map_err(|e| e.to_string())?;
    let out = fs::File::create(paths::browser_log(home)).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(&spec.exe);
    cmd.args(args(&spec, crate::platform::os()))
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|e| e.to_string())?)
        .stderr(out);
    hide(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", spec.exe.display()))?;
    let pid = child.id();
    let gen = GEN.fetch_add(1, Ordering::SeqCst);
    log(format!(
        "browser: {} started (pid {pid}), profile {}, DevTools port {}; its output is in browser.log",
        spec.app,
        spec.profile.display(),
        spec.port
    ));
    procs().lock().unwrap().insert(
        key.clone(),
        Proc {
            spec,
            child: Some(child),
            pid,
            gen,
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
                    "browser: {} closed ({why}); Show browser opens it again",
                    spec.app
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
                    "browser: {} answers on 127.0.0.1:{} (browser.json)",
                    spec.app, spec.port
                ));
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
    start_with(home, worker, &found, &envpy.to_string_lossy(), extra, log)
}

pub fn start_with(
    home: &Path,
    worker: &str,
    found: &Found,
    python: &str,
    extra: Vec<String>,
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
    if let Some((port, pid)) = adoptable(home) {
        let spec = prepare(home, worker, found, port, python, extra)?;
        adopt(home, spec, pid, log);
        return Ok(Started {
            port,
            app: found.name.clone(),
            python: python.into(),
        });
    }
    let port = free_port().ok_or("no free port for the browser")?;
    let spec = prepare(home, worker, found, port, python, extra)?;
    launch(home, spec, log)?;
    Ok(Started {
        port,
        app: found.name.clone(),
        python: python.into(),
    })
}

fn pid_alive(pid: u32) -> bool {
    if cfg!(windows) {
        let out = crate::engine::output(
            "tasklist",
            &["/FI", &format!("PID eq {pid}"), "/NH"],
            Duration::from_secs(10),
        );
        out.split_whitespace().any(|w| w == pid.to_string())
    } else {
        let mut cmd = Command::new("kill");
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
        let mut cmd = Command::new(if cfg!(windows) { "taskkill" } else { "kill" });
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
            if let Some((_, pid)) = adoptable(home) {
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
    let key = home.to_string_lossy().to_string();
    let known = procs()
        .lock()
        .unwrap()
        .get(&key)
        .map(|p| (p.spec.clone(), p.pid));
    let port = match &known {
        Some((s, _)) => s.port,
        None => adoptable(home).map(|(p, _)| p).unwrap_or(0),
    };
    if port != 0 && port_answers(port) {
        allow_foreground();
        match activate(port) {
            Ok(true) => return Ok("brought to the front".into()),
            Ok(false) => {
                if let Some((spec, _)) = &known {
                    let mut cmd = Command::new(&spec.exe);
                    cmd.arg(format!("--user-data-dir={}", spec.profile.display()))
                        .arg(&spec.start_url)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());
                    hide(&mut cmd);
                    let mut c = cmd.spawn().map_err(|e| e.to_string())?;
                    // It hands the address to the running browser and ends.
                    std::thread::spawn(move || {
                        let _ = c.wait();
                    });
                    return Ok("opened a window".into());
                }
            }
            Err(e) => log(format!("browser: couldn't bring it forward: {e}")),
        }
    }
    let spec = match known {
        Some((spec, _)) => spec,
        None => {
            let found = find_browser().ok_or("no browser found here")?;
            let envpy = env_python();
            let port = if port != 0 {
                port
            } else {
                free_port().ok_or("no free port for the browser")?
            };
            prepare(
                home,
                worker,
                &found,
                port,
                &envpy.to_string_lossy(),
                Vec::new(),
            )?
        }
    };
    if port_answers(spec.port) {
        return Err(format!(
            "something else answers on the browser's port {}",
            spec.port
        ));
    }
    launch(home, spec, log)?;
    Ok("started it again".into())
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
        // A Mac, for development: the apps in /Applications.
        let mac = |p: &Path| p.to_string_lossy().contains("Microsoft Edge.app");
        let f = find_browser_with("macos", &no_which, &none, &mac, &no_app_path).unwrap();
        assert_eq!(f.name, "Edge");
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
        // Elsewhere, a versioned name when python3 is too old (macOS's 3.9, Homebrew's 3.13).
        let mac = |p: &Path, _: &[String]| -> Option<String> {
            match p.to_string_lossy().as_ref() {
                "/usr/bin/python3" => Some("/usr/bin/python3\n3.9\nTrue".into()),
                "/usr/bin/python3.13" => Some("/opt/py/python3.13\n3.13\nTrue".into()),
                _ => None,
            }
        };
        let (py, old) = find_python_with("macos", &at, &mac);
        assert_eq!(
            (py.unwrap().version.as_str(), old.as_str()),
            ("3.13", "3.9")
        );
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

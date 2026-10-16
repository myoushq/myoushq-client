//! The container runtime: find it (Docker, Podman, Docker Desktop,
//! Rancher Desktop), run compose for a worker, stream a command's output,
//! ask for the browser view's port, list the worker containers and remove
//! the stale ones. Commands are argument lists, never shell strings.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Keeps child processes from flashing a console window on Windows.
pub fn hide(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

pub fn on_path(name: &str) -> bool {
    which(name).is_some()
}

/// The full path of a program on the search path (`search_path`), never
/// this app itself. Run what this returns rather than the bare name:
/// Windows looks in the app's own folder first, where `myous` is the app
/// (myous.exe), so `myous worker` would start another copy of the app,
/// which would do the same. An empty or relative PATH entry (a trailing
/// `;` is common) would also find files in the current folder.
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &search_path())
}

/// A command whose PATH is the search path, so that a bare name
/// (`docker`) is found there and what it runs in turn (`docker compose`,
/// `docker-credential-desktop`, the worker's own lookups) is too. Use it
/// for every program the app starts.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("PATH", search_path());
    cmd
}

/// Where programs are looked for: the PATH the app inherited, then the
/// folders of `extra_dirs` that exist. An app opened from a desktop
/// launcher inherits a short PATH (from the Finder, `/usr/bin:/bin:
/// /usr/sbin:/sbin`), which misses Docker Desktop, Homebrew, `~/.local/bin`
/// and the like; on Windows the PATH may predate an install. Read afresh
/// each time, so a runtime installed while the app runs is found.
pub fn search_path() -> std::ffi::OsString {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    search_path_from(
        std::env::var_os("PATH").as_deref(),
        &extra_dirs(crate::platform::os(), &var),
        &|p: &Path| p.is_dir(),
    )
}

/// `search_path` with what it reads passed in, for the tests: the
/// inherited entries in their order (empty ones and repeats dropped),
/// then each extra folder that is absolute, is a folder, and isn't there
/// yet. Repeats are compared without a trailing separator, and on
/// Windows without case.
pub fn search_path_from(
    inherited: Option<&std::ffi::OsStr>,
    extra: &[PathBuf],
    is_dir: &dyn Fn(&Path) -> bool,
) -> std::ffi::OsString {
    let key = |p: &Path| {
        let full = p.to_string_lossy();
        let trimmed = full.trim_end_matches(['/', '\\']);
        let s = if trimmed.is_empty() {
            full.as_ref()
        } else {
            trimmed
        };
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };
    let mut seen = std::collections::HashSet::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let inherited = inherited.map(std::env::split_paths).into_iter().flatten();
    for dir in inherited {
        if !dir.as_os_str().is_empty() && seen.insert(key(&dir)) {
            dirs.push(dir);
        }
    }
    for dir in extra {
        // A folder whose name has the separator in it can't be listed.
        if dir.is_absolute()
            && std::env::join_paths([dir]).is_ok()
            && !seen.contains(&key(dir))
            && is_dir(dir)
        {
            seen.insert(key(dir));
            dirs.push(dir.clone());
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Folders where runtimes, their helpers and Pythons are installed that
/// a desktop launcher's PATH may lack, for `os` ("macos", "linux",
/// "windows"). `var` reads the environment (HOME, ProgramFiles,
/// LOCALAPPDATA). On Windows the per-user Pythons
/// (`%LOCALAPPDATA%\Programs\Python\Python3XY`) are listed newest first.
pub fn extra_dirs(os: &str, var: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let home = var("HOME").map(PathBuf::from);
    let in_home = |rel: &str| home.as_ref().map(|h| h.join(rel));
    let mut dirs: Vec<PathBuf> = Vec::new();
    match os {
        "macos" => {
            for d in ["/usr/local/bin", "/opt/homebrew/bin", "/opt/homebrew/sbin"] {
                dirs.push(d.into());
            }
            dirs.extend(in_home(".docker/bin"));
            dirs.push("/Applications/Docker.app/Contents/Resources/bin".into());
            dirs.push("/opt/podman/bin".into());
            dirs.extend(in_home(".rd/bin")); // Rancher Desktop
            dirs.extend(in_home(".orbstack/bin"));
        }
        "windows" => {
            let under = |base: &str, parts: &[&str]| {
                var(base).map(|b| parts.iter().fold(PathBuf::from(b), |p, c| p.join(c)))
            };
            dirs.extend(under(
                "ProgramFiles",
                &["Docker", "Docker", "resources", "bin"],
            ));
            dirs.extend(under("ProgramFiles", &["RedHat", "Podman"]));
            dirs.extend(under(
                "LOCALAPPDATA",
                &[
                    "Programs",
                    "Rancher Desktop",
                    "resources",
                    "resources",
                    "win32",
                    "bin",
                ],
            ));
            if let Some(root) = under("LOCALAPPDATA", &["Programs", "Python"]) {
                let mut pythons: Vec<(u32, PathBuf)> = std::fs::read_dir(&root)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        let minor = name.strip_prefix("Python3")?;
                        let digits: String =
                            minor.chars().take_while(char::is_ascii_digit).collect();
                        Some((digits.parse().ok()?, e.path()))
                    })
                    .collect();
                pythons.sort_by(|a, b| b.cmp(a));
                dirs.extend(pythons.into_iter().map(|(_, p)| p));
                dirs.push(root.join("Launcher")); // py.exe, installed for this user
            }
            dirs.extend(under("LOCALAPPDATA", &["Microsoft", "WindowsApps"])); // winget
        }
        _ => {
            dirs.push("/usr/local/bin".into());
            dirs.extend(in_home(".local/bin"));
            dirs.push("/snap/bin".into());
            dirs.push("/var/lib/flatpak/exports/bin".into());
            dirs.extend(in_home(".local/share/flatpak/exports/bin"));
            dirs.extend(in_home(".rd/bin")); // Rancher Desktop
        }
    }
    dirs
}

// --- the PATH of a worker without a container ------------------------------------

/// The PATH a worker without a container runs with, and so its agents'
/// commands: on a Mac the owner's login shell's (`login_path`), as the
/// Objective-C app's `/bin/sh -lc` gave them (`/etc/paths` and
/// `/etc/paths.d` through path_helper, the profile's Homebrew, pyenv and
/// the like), then the search path's folders it lacks; elsewhere the
/// search path.
pub fn worker_path() -> std::ffi::OsString {
    #[cfg(target_os = "macos")]
    if let Some(login) = login_path() {
        return merge_paths(std::ffi::OsStr::new(&login.path), &search_path());
    }
    search_path()
}

/// Where `worker_path` came from, for the log.
pub fn worker_path_source() -> String {
    #[cfg(target_os = "macos")]
    if let Some(login) = login_path() {
        return format!("{}, then myous's own folders", login.from);
    }
    "myous's search path".into()
}

/// `first`'s folders in their order, then those of `then` it lacks
/// (`search_path_from`'s rules: no empty or relative ones, no repeats).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn merge_paths(first: &std::ffi::OsStr, then: &std::ffi::OsStr) -> std::ffi::OsString {
    let more: Vec<PathBuf> = std::env::split_paths(then).collect();
    search_path_from(Some(first), &more, &|_: &Path| true)
}

/// A login PATH and where it came from.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct LoginPath {
    pub path: String,
    pub from: String,
}

/// How long the login shell may take to say its PATH.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5);

/// What the login shell runs: the PATH between two markers, so whatever
/// the profile prints around it is left out.
const PATH_SCRIPT: &str = "printf '\\n@@myous-path@@%s@@myous-path@@\\n' \"$PATH\"";

/// The owner's login PATH, asked once per run of the app: the login
/// shell's (`$SHELL`, else the account's, run as a login shell that isn't
/// interactive, `-l -c`, with nothing to read and LOGIN_TIMEOUT to
/// answer), else path_helper's (`/etc/paths`, `/etc/paths.d`); None if
/// neither gives a sound one.
#[cfg(target_os = "macos")]
pub fn login_path() -> Option<&'static LoginPath> {
    static P: std::sync::OnceLock<Option<LoginPath>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let shell = login_shell(std::env::var_os("SHELL").map(PathBuf::from), account_shell());
        let mut why = Vec::new();
        match shell_path(&shell, &[], LOGIN_TIMEOUT) {
            Ok(p) => {
                return Some(LoginPath {
                    path: p,
                    from: format!("the login PATH of {}", shell.display()),
                })
            }
            Err(e) => why.push(format!("{}: {e}", shell.display())),
        }
        let mut cmd = Command::new("/usr/libexec/path_helper");
        cmd.arg("-s");
        let (status, out) = run_capture(cmd, LOGIN_TIMEOUT);
        match parse_path_helper(&out).filter(|_| status == 0) {
            Some(p) => Some(LoginPath {
                path: p,
                from: format!("path_helper's PATH ({})", why.join("; ")),
            }),
            None => {
                crate::files::append_app_log(&format!(
                    "no login PATH for workers without a container ({}; path_helper: {out}); they get myous's search path",
                    why.join("; ")
                ));
                None
            }
        }
    })
    .as_ref()
}

/// The account's shell (its passwd entry's), if it can be read.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn account_shell() -> Option<PathBuf> {
    // SAFETY: getpwuid's entry is read at once, on this thread, before
    // anything else could ask for another.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            return None;
        }
        let s = std::ffi::CStr::from_ptr((*pw).pw_shell);
        Some(PathBuf::from(s.to_string_lossy().to_string()))
    }
}

#[cfg(not(unix))]
#[allow(dead_code)]
fn account_shell() -> Option<PathBuf> {
    None
}

/// The shell to ask: `$SHELL`, else the account's, when it is a program
/// given by its full path; else zsh, a Mac's.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn login_shell(env: Option<PathBuf>, account: Option<PathBuf>) -> PathBuf {
    [env, account]
        .into_iter()
        .flatten()
        .find(|p| p.is_absolute() && p.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}

/// Runs `shell -l -c PATH_SCRIPT` (with `env` added), nothing on its
/// input, and returns the PATH it printed, if it is sound (`parse_login`).
/// A shell that hasn't answered within `timeout` is ended, with whatever
/// it started in its process group.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn shell_path(shell: &Path, env: &[(&str, &str)], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new(shell);
    cmd.args(["-l", "-c", PATH_SCRIPT])
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    hide(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("no output")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut out).take(1 << 20).read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let begun = std::time::Instant::now();
    let bytes = rx.recv_timeout(timeout);
    let left = timeout.saturating_sub(begun.elapsed());
    let mut exited = false;
    let until = std::time::Instant::now() + left.max(Duration::from_millis(200));
    while std::time::Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            exited = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !exited {
        #[cfg(unix)]
        // SAFETY: a signal to the process group the shell leads (its own,
        // process_group(0) above).
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    let bytes = bytes.map_err(|_| format!("no answer in {} s", timeout.as_secs()))?;
    parse_login(&String::from_utf8_lossy(&bytes))
}

/// The PATH between PATH_SCRIPT's markers, if it is sound: one line of
/// absolute folders, without control characters.
pub fn parse_login(out: &str) -> Result<String, String> {
    let m = "@@myous-path@@";
    let start = out.find(m).ok_or("it printed no PATH")? + m.len();
    let len = out[start..].find(m).ok_or("it printed no PATH")?;
    let path = &out[start..start + len];
    sound_path(path)
        .then(|| path.to_string())
        .ok_or_else(|| format!("its PATH isn't one: {path:?}"))
}

/// path_helper's `-s` answer: `PATH="…"; export PATH;`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_path_helper(out: &str) -> Option<String> {
    let start = out.find("PATH=\"")? + "PATH=\"".len();
    let len = out[start..].find('"')?;
    let path = &out[start..start + len];
    sound_path(path).then(|| path.to_string())
}

fn sound_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(char::is_control)
        && path.split(':').all(|d| d.starts_with('/'))
}

pub fn which_in(name: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or(".EXE;.CMD;.BAT".into())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    let me = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
    for dir in std::env::split_paths(path) {
        if !dir.is_absolute() {
            continue;
        }
        for ext in &exts {
            let p = dir.join(format!("{name}{ext}"));
            // Windows' app execution aliases (winget.exe in WindowsApps)
            // are reparse points that can't be opened as files, only run.
            let alias = cfg!(windows) && p.symlink_metadata().map(|m| !m.is_dir()).unwrap_or(false);
            if !p.is_file() && !alias {
                continue;
            }
            if me.is_some() && p.canonicalize().ok() == me {
                continue;
            }
            return Some(p);
        }
    }
    None
}

/// The runtime command: $MYOUS_DOCKER_BIN, else docker, else (not on
/// Windows) podman when only that is installed.
pub fn bin() -> String {
    if let Ok(b) = std::env::var("MYOUS_DOCKER_BIN") {
        if !b.is_empty() {
            return b;
        }
    }
    if on_path("docker") {
        return "docker".into();
    }
    if !cfg!(windows) && on_path("podman") {
        return "podman".into();
    }
    "docker".into()
}

/// Runs a command with a timeout and returns its trimmed stdout ("" on failure).
pub fn output(name: &str, args: &[&str], timeout: Duration) -> String {
    let mut cmd = command(name);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide(&mut cmd);
    let Ok(mut child) = cmd.spawn() else {
        return String::new();
    };
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_string(&mut s);
        }
        s
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return String::new();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break,
        }
    }
    reader.join().unwrap_or_default().trim().to_string()
}

/// Runs a prepared command with a timeout and returns its exit status
/// (124 on timeout, 127 if it could not start) and its combined output.
pub fn run_capture(mut cmd: Command, timeout: Duration) -> (i32, String) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (127, e.to_string()),
    };
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).to_string()
        })
    })
    .collect();
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break 124;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break -1,
        }
    };
    let texts: Vec<String> = readers
        .into_iter()
        .map(|r| r.join().unwrap_or_default())
        .collect();
    let out = if texts[1].trim().is_empty() {
        texts[0].clone()
    } else {
        format!("{}\n{}", texts[0], texts[1])
    };
    (status, out.trim().to_string())
}

/// What the Set up card needs to know about the runtime: whether it is
/// there and usable, and if not, why, in one word the window turns into a
/// card. `state` is "ok", "missing", "stopped" (installed, its engine
/// isn't up), "permission" (the owner may not use it yet: the docker group
/// on Linux, docker-users on Windows), "nocompose" (no `compose`), "error"
/// (the engine answered with an error of its own, in `detail`) or
/// "unknown" (no answer within the timeout: never a tick or a cross).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RuntimeCheck {
    pub state: String,
    pub name: String,
    pub detail: String,
}

/// How long `docker version` and `docker compose version` may take.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Checks the runtime: on PATH, `version` (the engine answers), `compose
/// version` (the part that starts the worker's containers).
pub fn check() -> RuntimeCheck {
    let b = bin();
    let explicit = std::env::var("MYOUS_DOCKER_BIN")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if !(on_path(&b) || (explicit && Path::new(&b).is_file())) {
        return RuntimeCheck {
            state: "missing".into(),
            name: "Docker".into(),
            detail: String::new(),
        };
    }
    let run = |args: &[&str]| {
        let mut cmd = command(&b);
        cmd.args(args);
        run_capture(cmd, CHECK_TIMEOUT)
    };
    let (status, out) = run(&["version"]);
    let compose = if status == 0 {
        Some(run(&["compose", "version"]))
    } else {
        None
    };
    let (state, detail) = judge(
        (status, &out),
        compose.as_ref().map(|(s, o)| (*s, o.as_str())),
    );
    RuntimeCheck {
        state,
        name: name(),
        detail,
    }
}

/// The verdict from `version`'s and `compose version`'s exit status and
/// output (124 is a timeout): the state and, for an error, the line that
/// says what.
pub fn judge(version: (i32, &str), compose: Option<(i32, &str)>) -> (String, String) {
    let (status, out) = version;
    let verdict = |s: &str, d: String| (s.to_string(), d);
    if status == 124 {
        return verdict("unknown", "no answer within 5 s".into());
    }
    if status == 0 {
        return match compose {
            Some((0, _)) | None => verdict("ok", String::new()),
            Some((124, _)) => verdict("unknown", "no answer within 5 s".into()),
            Some((_, text)) => verdict("nocompose", error_line(text)),
        };
    }
    let t = out.to_lowercase();
    // Not allowed: Docker Engine's socket on Linux, Docker Desktop's pipe
    // on Windows (only the docker-users group may open it).
    if ["permission denied", "access is denied", "docker-users"]
        .iter()
        .any(|p| t.contains(p))
    {
        return verdict("permission", error_line(out));
    }
    // Installed, but its engine (the service, or the desktop app's VM) isn't up.
    const NOT_RUNNING: [&str; 9] = [
        "is the docker daemon running",
        "daemon is not running",
        "if the daemon is running",
        "cannot connect to the docker daemon",
        "cannot connect to podman",
        "unable to connect to podman",
        "connection refused",
        "no such file or directory",
        "cannot find the file specified",
    ];
    if NOT_RUNNING.iter().any(|p| t.contains(p)) {
        return verdict("stopped", error_line(out));
    }
    verdict("error", error_line(out))
}

/// The line of a command's output that says what went wrong: the last one
/// naming an error, else the last line.
fn error_line(out: &str) -> String {
    let lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let wrong = |l: &&&str| {
        let l = l.to_lowercase();
        [
            "error",
            "cannot",
            "denied",
            "unknown",
            "not a docker command",
            "failed",
        ]
        .iter()
        .any(|w| l.contains(w))
    };
    lines
        .iter()
        .rev()
        .find(wrong)
        .or(lines.last())
        .map(|l| l.to_string())
        .unwrap_or_default()
}

/// The product the owner installed, for the screens.
pub fn name() -> String {
    let b = bin();
    let base = Path::new(&b)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if base.starts_with("podman") {
        return "Podman".into();
    }
    let ctx = output(&b, &["context", "show"], CHECK_TIMEOUT).to_lowercase();
    if ctx.contains("rancher") {
        return "Rancher Desktop".into();
    }
    if ctx.contains("podman") {
        return "Podman".into();
    }
    if ctx.contains("colima") {
        return "Colima".into();
    }
    if ctx.contains("orbstack") {
        return "OrbStack".into();
    }
    if ctx.contains("desktop") {
        return "Docker Desktop".into();
    }
    installed_name()
}

#[cfg(windows)]
fn installed_name() -> String {
    let pf = std::env::var("ProgramFiles").unwrap_or_default();
    if Path::new(&pf)
        .join("Docker")
        .join("Docker")
        .join("Docker Desktop.exe")
        .is_file()
    {
        return "Docker Desktop".into();
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if Path::new(&local)
        .join("Programs")
        .join("Rancher Desktop")
        .join("Rancher Desktop.exe")
        .is_file()
    {
        return "Rancher Desktop".into();
    }
    "Docker".into()
}

#[cfg(target_os = "macos")]
fn installed_name() -> String {
    for (p, n) in [
        ("/Applications/Docker.app", "Docker Desktop"),
        ("/Applications/OrbStack.app", "OrbStack"),
        ("/Applications/Rancher Desktop.app", "Rancher Desktop"),
    ] {
        if Path::new(p).exists() {
            return n.into();
        }
    }
    "Docker".into()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn installed_name() -> String {
    if Path::new("/opt/docker-desktop").exists() {
        return "Docker Desktop".into();
    }
    if Path::new("/opt/rancher-desktop").exists() {
        return "Rancher Desktop".into();
    }
    "Docker Engine".into()
}

/// Opens the runtime's own app so its engine comes up (Docker Engine is
/// a system service the owner starts; the screen says so).
pub fn launch(name: &str) {
    let mut cmd;
    #[cfg(windows)]
    {
        let exe = if name == "Rancher Desktop" {
            Path::new(&std::env::var("LOCALAPPDATA").unwrap_or_default())
                .join("Programs")
                .join("Rancher Desktop")
                .join("Rancher Desktop.exe")
        } else {
            Path::new(&std::env::var("ProgramFiles").unwrap_or_default())
                .join("Docker")
                .join("Docker")
                .join("Docker Desktop.exe")
        };
        cmd = command(exe);
    }
    #[cfg(target_os = "macos")]
    {
        cmd = command("open");
        cmd.arg("-a").arg(if name == "Docker Desktop" {
            "Docker"
        } else {
            name
        });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match name {
            "Docker Desktop" => {
                cmd = command("systemctl");
                cmd.args(["--user", "start", "docker-desktop"]);
            }
            "Rancher Desktop" => cmd = command("rancher-desktop"),
            _ => return,
        }
    }
    hide(&mut cmd);
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// `docker compose -f FILE -p PROJECT`, the start of every compose command.
pub fn compose_args(file: &str, project: &str) -> Vec<String> {
    vec![
        bin(),
        "compose".into(),
        "-f".into(),
        file.into(),
        "-p".into(),
        project.into(),
    ]
}

/// Runs a command and streams its output line by line (compose redraws
/// progress with \r, treated as a newline), then returns the exit status
/// (-1 if it could not start). Blocks: call it from a thread.
pub fn run_streaming(
    args: &[String],
    dir: &Path,
    env: &HashMap<String, String>,
    mut line: impl FnMut(String),
) -> i32 {
    let mut cmd = command(&args[0]);
    cmd.args(&args[1..])
        .current_dir(dir)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            line(format!("could not run: {e}"));
            return -1;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut readers = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ] {
        let Some(pipe) = pipe else { continue };
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match read_until_any(&mut reader, &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&buf).trim().to_string();
                        if !text.is_empty() {
                            let _ = tx.send(text);
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
    }
    drop(tx);
    for l in rx {
        line(l);
    }
    for r in readers {
        let _ = r.join();
    }
    match child.wait() {
        Ok(st) => match st.code() {
            Some(0) => 0,
            Some(c) => c,
            None => -1,
        },
        Err(_) => -1,
    }
}

/// Reads up to and including the next \r or \n.
fn read_until_any<R: BufRead>(r: &mut R, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut total = 0;
    loop {
        let available = r.fill_buf()?;
        if available.is_empty() {
            return Ok(total);
        }
        if let Some(i) = available.iter().position(|&b| b == b'\n' || b == b'\r') {
            buf.extend_from_slice(&available[..i]);
            r.consume(i + 1);
            return Ok(total + i + 1);
        }
        let n = available.len();
        buf.extend_from_slice(available);
        r.consume(n);
        total += n;
    }
}

fn last_port(addr: &str) -> String {
    let first = addr.trim().lines().next().unwrap_or("").trim();
    let port = match first.rfind(':') {
        Some(i) => &first[i + 1..],
        None => first,
    };
    if port.is_empty() || port == "0" || !port.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    port.to_string()
}

/// The host port of the browser view (6080 in the container), which
/// Docker picks at each start; for the first worker, which may run a
/// container from before v0.6.0 with no project we started, by label too.
pub fn browser_port(compose: &[String], legacy: bool) -> String {
    let mut args: Vec<&str> = compose[1..].iter().map(String::as_str).collect();
    args.extend(["port", "worker", "6080"]);
    let port = last_port(&output(&compose[0], &args, Duration::from_secs(15)));
    if !port.is_empty() || !legacy {
        return port;
    }
    let b = bin();
    let ids = output(
        &b,
        &["ps", "-q", "--filter", "label=com.myoushq.worker"],
        Duration::from_secs(15),
    );
    let name = ids
        .split_whitespace()
        .next()
        .unwrap_or("myous-worker-worker-1")
        .to_string();
    last_port(&output(
        &b,
        &["port", &name, "6080/tcp"],
        Duration::from_secs(15),
    ))
}

/// A container labelled as a myous worker (`com.myoushq.worker`), whatever
/// project started it, as the runtime's `inspect` describes it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Container {
    pub id: String,
    /// Without Docker's leading `/`.
    pub name: String,
    /// The runtime's word: running, restarting, paused, created, exited,
    /// dead (Podman's own too: stopped, configured, stopping…).
    pub state: String,
    /// Running, restarting or paused: removing it stops a worker.
    pub running: bool,
    pub image: String,
    /// Its compose project (the `com.docker.compose.project` label), "" if none.
    pub project: String,
    /// The tag of the published image (`ghcr.io/myoushq/worker:<version>`),
    /// "" for any other image (a checkout's, built here).
    pub version: String,
}

/// The published image; its tag is the version of the app that ran it.
const PUBLISHED_IMAGE: &str = "ghcr.io/myoushq/worker:";

/// Every worker container, running or not, for Remove stale containers;
/// an error (the runtime's line) when the runtime doesn't answer.
pub fn containers() -> Result<Vec<Container>, String> {
    let b = bin();
    let mut cmd = command(&b);
    cmd.args(["ps", "-aq", "--filter", "label=com.myoushq.worker"]);
    let (status, out) = run_capture(cmd, Duration::from_secs(15));
    if status != 0 {
        return Err(error_line(&out));
    }
    // The ids alone: a warning the runtime printed alongside isn't one.
    let ids: Vec<&str> = out.split_whitespace().filter(|w| is_id(w)).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["container", "inspect"];
    args.extend(ids);
    Ok(parse_containers(&output(
        &b,
        &args,
        Duration::from_secs(15),
    )))
}

fn is_id(word: &str) -> bool {
    word.len() >= 12 && word.bytes().all(|c| c.is_ascii_hexdigit())
}

/// The containers in `container inspect`'s JSON, Docker's or Podman's:
/// both have `Id`, `Name`, `State.Status` and `State.Running`, and
/// `Config.Image` and `Config.Labels` (Podman also `ImageName`).
pub fn parse_containers(json: &str) -> Vec<Container> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    list.iter()
        .map(|v| {
            let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or("");
            let on = |p: &str| v.pointer(p).and_then(|x| x.as_bool()).unwrap_or(false);
            let state = s("/State/Status").to_lowercase();
            let running = ["running", "restarting", "paused", "stopping"].contains(&state.as_str())
                || on("/State/Running")
                || on("/State/Restarting")
                || on("/State/Paused");
            let image = match s("/Config/Image") {
                "" => s("/ImageName"),
                i => i,
            };
            let project = v
                .pointer("/Config/Labels")
                .and_then(|l| l.get("com.docker.compose.project"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            Container {
                id: s("/Id").into(),
                name: s("/Name").trim_start_matches('/').into(),
                state,
                running,
                image: image.into(),
                project: project.into(),
                version: image_version(image),
            }
        })
        .filter(|c| !c.id.is_empty())
        .collect()
}

/// The version in the published image's tag (`0.7.5` in
/// `ghcr.io/myoushq/worker:0.7.5`); "" for another image or a tag that
/// isn't a version (`latest`).
pub fn image_version(image: &str) -> String {
    let Some(tag) = image.strip_prefix(PUBLISHED_IMAGE) else {
        return String::new();
    };
    let tag = tag.split('@').next().unwrap_or("");
    let digits = tag.strip_prefix('v').unwrap_or(tag);
    if digits.starts_with(|c: char| c.is_ascii_digit()) {
        tag.to_string()
    } else {
        String::new()
    }
}

/// What Remove did: the line for the app's log, and the ids it removed.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Removal {
    pub log: String,
    pub removed: Vec<String>,
}

/// Removes the containers the owner picked: the stopped ones with a plain
/// `rm`, so one that started in the meantime is refused rather than
/// stopped; the running ones (each chosen as such) stopped first, as Stop's
/// `compose down` does (the worker answers a request it is running "stopped
/// by the owner"; the runtime kills it after its grace period), then `rm -f`.
pub fn remove(stopped: &[Container], running: &[Container]) -> Removal {
    let b = bin();
    let (mut removed, mut names, mut refused) = (Vec::new(), Vec::new(), Vec::new());
    let picked = stopped.iter().map(|c| (c, false));
    for (c, force) in picked.chain(running.iter().map(|c| (c, true))) {
        if force {
            let mut stop = command(&b);
            stop.args(["stop", &c.id]);
            run_capture(stop, Duration::from_secs(60));
        }
        let mut cmd = command(&b);
        cmd.arg("rm");
        if force {
            cmd.arg("-f");
        }
        cmd.arg(&c.id);
        let (status, out) = run_capture(cmd, Duration::from_secs(60));
        if status == 0 {
            removed.push(c.id.clone());
            names.push(label(c));
        } else {
            refused.push(format!("{} ({})", label(c), error_line(&out)));
        }
    }
    Removal {
        log: removal_log(&names, &refused),
        removed,
    }
}

/// A container's name, else the start of its id.
fn label(c: &Container) -> String {
    if c.name.is_empty() {
        c.id.chars().take(12).collect()
    } else {
        c.name.clone()
    }
}

/// "removed: a b; not removed: c (why)", for the app's log.
fn removal_log(removed: &[String], refused: &[String]) -> String {
    let mut parts = Vec::new();
    if !removed.is_empty() {
        parts.push(format!("removed: {}", removed.join(" ")));
    }
    if !refused.is_empty() {
        parts.push(format!("not removed: {}", refused.join("; ")));
    }
    if parts.is_empty() {
        return "no containers removed".into();
    }
    parts.join("; ")
}

/// The container's recent output.
pub fn logs(compose: &[String]) -> String {
    let mut args: Vec<&str> = compose[1..].iter().map(String::as_str).collect();
    args.extend(["logs", "--no-color", "--tail", "300"]);
    output(&compose[0], &args, Duration::from_secs(20))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worker without a container on a Mac: the login shell's PATH
    /// first, then the search path's folders it lacks, in order, once each.
    #[cfg(unix)]
    #[test]
    fn the_login_path_comes_first() {
        let login = "/opt/homebrew/bin:/usr/local/go/bin:/usr/bin:/bin:/Users/max/.pyenv/shims";
        let search = "/usr/bin:/bin:/usr/sbin:/sbin:relative::/usr/local/bin:/opt/homebrew/bin/:/usr/local/go/bin";
        assert_eq!(
            merge_paths(login.as_ref(), search.as_ref()),
            std::ffi::OsString::from(
                "/opt/homebrew/bin:/usr/local/go/bin:/usr/bin:/bin:/Users/max/.pyenv/shims:/usr/sbin:/sbin:/usr/local/bin"
            )
        );
        // What the shell printed: the PATH between the markers, whatever
        // the profile printed around it; nothing that isn't a PATH.
        let m = "@@myous-path@@";
        assert_eq!(
            parse_login(&format!(
                "Welcome!\nLast login: today\n\n{m}/a/bin:/usr/bin{m}\nbye\n"
            )),
            Ok("/a/bin:/usr/bin".into())
        );
        for bad in [
            "no markers".to_string(),
            format!("{m}{m}"),
            format!("{m}/a:relative{m}"),
            format!("{m}/a\n/b{m}"),
            format!("{m}/a"),
        ] {
            assert!(parse_login(&bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            parse_path_helper(
                "PATH=\"/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin\"; export PATH;\n"
            ),
            Some("/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into())
        );
        assert_eq!(parse_path_helper("PATH=\"\"; export PATH;"), None);
        assert_eq!(parse_path_helper("nothing"), None);
        // The shell: $SHELL, else the account's, by a full path to a program.
        let sh = PathBuf::from("/bin/sh");
        assert_eq!(login_shell(Some(sh.clone()), None), sh);
        assert_eq!(login_shell(Some("zsh".into()), Some(sh.clone())), sh);
        assert_eq!(
            login_shell(Some("/no/such/shell".into()), None),
            PathBuf::from("/bin/zsh")
        );
    }

    /// A real login shell (`/bin/sh -l`, with a home of the test's own): a
    /// profile that prints and adds a folder; one that hangs is ended.
    #[cfg(unix)]
    #[test]
    fn a_login_shell_says_its_path() {
        let home = std::env::temp_dir().join(format!("myous-login-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let h = home.to_string_lossy().to_string();
        std::fs::write(
            home.join(".profile"),
            "echo 'Welcome to this test'\nexport PATH=\"/opt/myous-test/bin:$PATH\"\n",
        )
        .unwrap();
        let p = shell_path(
            Path::new("/bin/sh"),
            &[("HOME", &h)],
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(p.starts_with("/opt/myous-test/bin:"), "{p}");
        assert!(!p.contains("Welcome"));
        std::fs::write(home.join(".profile"), "sleep 30\n").unwrap();
        let begun = std::time::Instant::now();
        let e = shell_path(
            Path::new("/bin/sh"),
            &[("HOME", &h)],
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(e.contains("no answer"), "{e}");
        assert!(
            begun.elapsed() < Duration::from_secs(5),
            "{:?}",
            begun.elapsed()
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn which_skips_relative_entries_and_this_program() {
        let me = std::env::current_exe().unwrap();
        let dir = me.parent().unwrap();
        let file = me.file_name().unwrap().to_string_lossy().to_string();
        let name = file.strip_suffix(".exe").unwrap_or(&file);
        let other = std::env::temp_dir().join(format!("myous-which-{}", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        let exe = if cfg!(windows) { ".exe" } else { "" };
        std::fs::write(other.join(format!("{name}{exe}")), "").unwrap();
        let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        // This program is skipped, a later one with the same name is found.
        assert_eq!(which_in(name, &path(&[dir])), None);
        assert_eq!(
            which_in(name, &path(&[dir, &other])),
            Some(other.join(format!("{name}{exe}")))
        );
        // Relative entries are skipped.
        let rel = std::env::join_paths([Path::new(""), Path::new(".")]).unwrap();
        assert_eq!(which_in(name, &rel), None);
        std::fs::remove_dir_all(&other).unwrap();
    }

    /// The inherited PATH comes first, in its order; the extra folders
    /// that exist follow; repeats, empty entries and missing or relative
    /// folders are dropped.
    #[test]
    fn search_path_merges_inherited_and_extra() {
        let tmp = std::env::temp_dir().join(format!("myous-search-{}", std::process::id()));
        let (a, b, c) = (tmp.join("a"), tmp.join("b"), tmp.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).unwrap();
        }
        let missing = tmp.join("missing");
        let a_slash = PathBuf::from(format!("{}{}", a.display(), std::path::MAIN_SEPARATOR));
        let inherited = std::env::join_paths([&a, Path::new(""), &b, &a_slash]).unwrap();
        let extra = [
            b.clone(),
            missing.clone(),
            PathBuf::from("relative"),
            c.clone(),
            c.clone(),
        ];
        let merged = search_path_from(Some(&inherited), &extra, &|p: &Path| p.is_dir());
        let got: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert_eq!(got, vec![a.clone(), b.clone(), c.clone()]);
        // Inherited folders stay even when missing (the PATH is the owner's).
        let inherited = std::env::join_paths([&missing]).unwrap();
        let merged = search_path_from(Some(&inherited), std::slice::from_ref(&c), &|p: &Path| {
            p.is_dir()
        });
        assert_eq!(
            std::env::split_paths(&merged).collect::<Vec<_>>(),
            vec![missing.clone(), c.clone()]
        );
        // No PATH at all: the extra folders alone.
        let merged = search_path_from(None, &[a.clone(), missing], &|p: &Path| p.is_dir());
        assert_eq!(std::env::split_paths(&merged).collect::<Vec<_>>(), vec![a]);
        // Windows compares without case.
        if cfg!(windows) {
            let upper = PathBuf::from(b.to_string_lossy().to_uppercase());
            let inherited = std::env::join_paths([&b]).unwrap();
            let merged = search_path_from(Some(&inherited), &[upper], &|_: &Path| true);
            assert_eq!(std::env::split_paths(&merged).count(), 1);
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// A program in a folder that only the extra list names is found, with
    /// Windows' PATHEXT still applied (a `.cmd` file runs as `name`).
    #[test]
    fn which_finds_a_program_in_an_extra_folder() {
        let tmp = std::env::temp_dir().join(format!("myous-extra-{}", std::process::id()));
        let (on_path, extra) = (tmp.join("on-path"), tmp.join("extra"));
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        let file = if cfg!(windows) {
            "myous-tool.cmd"
        } else {
            "myous-tool"
        };
        std::fs::write(extra.join(file), "").unwrap();
        let inherited = std::env::join_paths([&on_path]).unwrap();
        assert_eq!(which_in("myous-tool", &inherited), None);
        let merged = search_path_from(
            Some(&inherited),
            std::slice::from_ref(&extra),
            &|p: &Path| p.is_dir(),
        );
        assert_eq!(which_in("myous-tool", &merged), Some(extra.join(file)));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn extra_dirs_per_os() {
        let tmp = std::env::temp_dir().join(format!("myous-extra-dirs-{}", std::process::id()));
        let local = tmp.join("local");
        let pythons = local.join("Programs").join("Python");
        for d in [
            "Python310",
            "Python313",
            "Python312-32",
            "Launcher",
            "Other",
        ] {
            std::fs::create_dir_all(pythons.join(d)).unwrap();
        }
        let local_s = local.to_string_lossy().to_string();
        let var = |k: &str| match k {
            "HOME" => Some("/home/ann".to_string()),
            "ProgramFiles" => Some("C:/Program Files".to_string()),
            "LOCALAPPDATA" => Some(local_s.clone()),
            _ => None,
        };
        let mac = extra_dirs("macos", &var);
        assert_eq!(mac[0], PathBuf::from("/usr/local/bin"));
        assert!(mac.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(mac.contains(&PathBuf::from("/home/ann/.docker/bin")));
        assert!(mac.contains(&PathBuf::from("/home/ann/.orbstack/bin")));
        let linux = extra_dirs("linux", &var);
        assert!(linux.contains(&PathBuf::from("/home/ann/.local/bin")));
        assert!(linux.contains(&PathBuf::from("/snap/bin")));
        let win = extra_dirs("windows", &var);
        let docker = Path::new("C:/Program Files")
            .join("Docker")
            .join("Docker")
            .join("resources")
            .join("bin");
        assert_eq!(win[0], docker);
        // Per-user Pythons newest first, then the launcher; others skipped.
        let py: Vec<&PathBuf> = win.iter().filter(|p| p.starts_with(&pythons)).collect();
        assert_eq!(
            py,
            vec![
                &pythons.join("Python313"),
                &pythons.join("Python312-32"),
                &pythons.join("Python310"),
                &pythons.join("Launcher"),
            ]
        );
        // No HOME: its folders are left out, not made relative. (Checked
        // on the string: on a Windows host a path without a drive letter
        // is not `is_absolute`, and this test runs there too.)
        let none = extra_dirs("linux", &|_: &str| None);
        assert!(none.iter().all(|p| p.to_string_lossy().starts_with('/')));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The runtime's own words, as `docker version` and `docker compose
    /// version` print them, sorted into the Set up card's cases.
    #[test]
    fn judges_the_runtime() {
        let client = "Client:\n Version:           27.5.1\n API version:       1.47\n OS/Arch:           linux/amd64\n";
        let state = |out: &str| judge((1, &format!("{client}{out}")), None);
        let ok = |compose: (i32, &str)| judge((0, client), Some(compose)).0;
        // Linux, Docker Engine's service not started (old and new CLIs).
        assert_eq!(
            state("Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?").0,
            "stopped"
        );
        assert_eq!(
            state("failed to connect to the docker API at unix:///var/run/docker.sock; check if the path is correct and if the daemon is running: dial unix /var/run/docker.sock: connect: no such file or directory").0,
            "stopped"
        );
        // Linux, not in the docker group.
        let (s, d) = state("permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock: Get \"http://%2Fvar%2Frun%2Fdocker.sock/v1.47/version\": dial unix /var/run/docker.sock: connect: permission denied");
        assert_eq!(s, "permission");
        assert!(d.starts_with("permission denied while trying"), "{d}");
        // Windows: Docker Desktop not running; and the owner not in docker-users.
        assert_eq!(
            state("error during connect: this error may indicate that the docker daemon is not running: Get \"http://%2F%2F.%2Fpipe%2Fdocker_engine/v1.24/version\": open //./pipe/docker_engine: The system cannot find the file specified.").0,
            "stopped"
        );
        assert_eq!(
            state("error during connect: Get \"http://%2F%2F.%2Fpipe%2FdockerDesktopLinuxEngine/v1.47/version\": open //./pipe/dockerDesktopLinuxEngine: Access is denied.").0,
            "permission"
        );
        // Podman without its machine (Mac, Windows).
        assert_eq!(
            judge((125, "Cannot connect to Podman. Please verify your connection to the Linux system using `podman system connection list`, or try `podman machine init` and `podman machine start` to manage a new Linux VM\nError: unable to connect to Podman socket"), None).0,
            "stopped"
        );
        // Anything else is the engine's own error, said as it said it.
        let (s, d) = state("Error response from daemon: Docker Desktop is unable to start");
        assert_eq!(
            (s.as_str(), d.as_str()),
            (
                "error",
                "Error response from daemon: Docker Desktop is unable to start"
            )
        );
        // No answer in time: unknown, never a verdict.
        assert_eq!(judge((124, ""), None).0, "unknown");
        assert_eq!(ok((124, "")), "unknown");
        // Compose: there (Docker, Podman with a provider), or missing (Debian's
        // and Ubuntu's docker.io, Podman without one).
        assert_eq!(ok((0, "Docker Compose version v2.29.7")), "ok");
        assert_eq!(ok((0, "podman-compose version 1.0.6")), "ok");
        let (s, d) = judge(
            (0, client),
            Some((
                1,
                "docker: 'compose' is not a docker command.\nSee 'docker --help'",
            )),
        );
        assert_eq!(
            (s.as_str(), d.as_str()),
            ("nocompose", "docker: 'compose' is not a docker command.")
        );
        assert_eq!(
            ok((125, "Error: looking up compose provider failed\n7 errors occurred:\n\t* exec: \"docker-compose\": executable file not found in $PATH")),
            "nocompose"
        );
    }

    /// The check on this machine (in CI, whatever the runner has).
    #[test]
    fn checks_this_runtime() {
        let c = check();
        println!("runtime here: {c:?}");
        assert!([
            "ok",
            "missing",
            "stopped",
            "permission",
            "nocompose",
            "error",
            "unknown"
        ]
        .contains(&c.state.as_str()));
    }

    /// Docker's `container inspect` (the fields read, as Docker Desktop
    /// 4.x prints them): running, exited, created, paused, restarting.
    #[test]
    fn reads_dockers_containers() {
        let json = r#"[
          {"Id": "16cc18df9eacbacee2d78fbf4633403311bf7e42a103cc293f416278a3165362", "Name": "/myous-worker-worker-1",
           "State": {"Status": "running", "Running": true, "Paused": false, "Restarting": false, "Dead": false, "ExitCode": 0},
           "Image": "sha256:256077a3463d44144fc5c8dfe88fe9da5d67bfd48f77f24a6a8f0202004119a1",
           "Config": {"Image": "ghcr.io/myoushq/worker:0.7.5",
             "Labels": {"com.docker.compose.project": "myous-worker", "com.docker.compose.service": "worker", "com.myoushq.worker": "1"}}},
          {"Id": "2b1d7c3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c", "Name": "/myous-worker-2-worker-1",
           "State": {"Status": "exited", "Running": false, "Paused": false, "Restarting": false, "ExitCode": 137},
           "Config": {"Image": "ghcr.io/myoushq/worker:0.6.1", "Labels": {"com.docker.compose.project": "myous-worker-2", "com.myoushq.worker": "1"}}},
          {"Id": "3c2e8d4f5a6b", "Name": "/myous-worker",
           "State": {"Status": "created", "Running": false, "Paused": false, "Restarting": false},
           "Config": {"Image": "myous-worker_worker", "Labels": {"com.myoushq.worker": "1"}}},
          {"Id": "4d3f9e5a6b7c", "Name": "/worker-worker-1",
           "State": {"Status": "paused", "Running": true, "Paused": true, "Restarting": false},
           "Config": {"Image": "worker-worker", "Labels": {"com.docker.compose.project": "worker", "com.myoushq.worker": "1"}}},
          {"Id": "5e4a0f6b7c8d", "Name": "/myous-worker-3-worker-1",
           "State": {"Status": "restarting", "Running": true, "Paused": false, "Restarting": true},
           "Config": {"Image": "ghcr.io/myoushq/worker:0.7.4", "Labels": null}}
        ]"#;
        let got = parse_containers(json);
        let brief: Vec<(&str, &str, bool, &str, &str)> = got
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.state.as_str(),
                    c.running,
                    c.project.as_str(),
                    c.version.as_str(),
                )
            })
            .collect();
        assert_eq!(
            brief,
            vec![
                (
                    "myous-worker-worker-1",
                    "running",
                    true,
                    "myous-worker",
                    "0.7.5"
                ),
                (
                    "myous-worker-2-worker-1",
                    "exited",
                    false,
                    "myous-worker-2",
                    "0.6.1"
                ),
                ("myous-worker", "created", false, "", ""),
                ("worker-worker-1", "paused", true, "worker", ""),
                ("myous-worker-3-worker-1", "restarting", true, "", "0.7.4"),
            ]
        );
        assert_eq!(
            got[0].id,
            "16cc18df9eacbacee2d78fbf4633403311bf7e42a103cc293f416278a3165362"
        );
        assert_eq!(got[0].image, "ghcr.io/myoushq/worker:0.7.5");
        assert_eq!(got[2].image, "myous-worker_worker");
        // Nothing readable: no containers, no panic.
        assert!(parse_containers("").is_empty());
        assert!(parse_containers("Error: no such container").is_empty());
        assert!(parse_containers(r#"[{"Name": "/no-id"}]"#).is_empty());
    }

    /// Podman's `container inspect` (its documented format): the name
    /// without a `/`, its own states (configured: created, not started),
    /// `ImageName` beside or instead of `Config.Image`, podman-compose's
    /// labels.
    #[test]
    fn reads_podmans_containers() {
        let json = r#"[
          {"Id": "99f66530fe9c7249f7cf29f78e8661669d5831cbe4ee80ea757d5e922dd6a8a6", "Created": "2026-10-15T06:09:08.936623325-04:00",
           "Path": "/opt/worker/entrypoint.sh", "Args": [],
           "State": {"OciVersion": "1.2.0", "Status": "running", "Running": true, "Paused": false, "Restarting": false,
             "OOMKilled": false, "Dead": false, "Pid": 4242, "ExitCode": 0, "StartedAt": "2026-10-15T06:09:09.1-04:00"},
           "Image": "d13f40ccb4ac7a8b9a5ae3cb4c1b2e5b5d1c0b3c8e5e2a6f1b3c4d5e6f7a8b9c",
           "ImageName": "ghcr.io/myoushq/worker:0.7.4",
           "Name": "myous-worker_worker_1",
           "Config": {"Hostname": "99f66530fe9c", "Image": "ghcr.io/myoushq/worker:0.7.4",
             "Labels": {"com.docker.compose.project": "myous-worker", "io.podman.compose.project": "myous-worker",
               "com.docker.compose.service": "worker", "com.myoushq.worker": "1"}}},
          {"Id": "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90",
           "State": {"Status": "configured", "Running": false, "Paused": false, "Restarting": false, "Dead": false},
           "ImageName": "ghcr.io/myoushq/worker:0.6.0",
           "Name": "myous-worker-2_worker_1",
           "Config": {"Labels": {"com.docker.compose.project": "myous-worker-2", "com.myoushq.worker": "1"}}},
          {"Id": "b2c3d4e5f6071829", "Name": "myous-old",
           "State": {"Status": "Exited", "Running": false},
           "ImageName": "localhost/worker-worker:latest", "Config": {"Image": "localhost/worker-worker:latest", "Labels": {}}}
        ]"#;
        let got = parse_containers(json);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            Container {
                id: "99f66530fe9c7249f7cf29f78e8661669d5831cbe4ee80ea757d5e922dd6a8a6".into(),
                name: "myous-worker_worker_1".into(),
                state: "running".into(),
                running: true,
                image: "ghcr.io/myoushq/worker:0.7.4".into(),
                project: "myous-worker".into(),
                version: "0.7.4".into(),
            }
        );
        assert_eq!(
            (
                got[1].state.as_str(),
                got[1].running,
                got[1].image.as_str(),
                got[1].version.as_str(),
                got[1].project.as_str()
            ),
            (
                "configured",
                false,
                "ghcr.io/myoushq/worker:0.6.0",
                "0.6.0",
                "myous-worker-2"
            )
        );
        assert_eq!(
            (
                got[2].state.as_str(),
                got[2].running,
                got[2].project.as_str(),
                got[2].version.as_str()
            ),
            ("exited", false, "", "")
        );
    }

    #[test]
    fn versions_from_the_published_image_only() {
        assert_eq!(image_version("ghcr.io/myoushq/worker:0.7.5"), "0.7.5");
        assert_eq!(image_version("ghcr.io/myoushq/worker:v0.8.0"), "v0.8.0");
        assert_eq!(
            image_version("ghcr.io/myoushq/worker:0.7.5@sha256:0123abcd"),
            "0.7.5"
        );
        assert_eq!(image_version("ghcr.io/myoushq/worker:latest"), "");
        assert_eq!(image_version("ghcr.io/myoushq/worker"), "");
        assert_eq!(image_version("myous-worker_worker"), "");
        assert_eq!(image_version("ghcr.io/someone/worker:0.7.5"), "");
        assert_eq!(image_version(""), "");
    }

    #[test]
    fn ids_and_the_log_line() {
        assert!(is_id("16cc18df9eac"));
        assert!(is_id(
            "16cc18df9eacbacee2d78fbf4633403311bf7e42a103cc293f416278a3165362"
        ));
        assert!(!is_id("WARN[0000]"));
        assert!(!is_id("abc"));
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(removal_log(&s(&["a", "b"]), &[]), "removed: a b");
        assert_eq!(
            removal_log(
                &s(&["a"]),
                &s(&["c (Error response from daemon: container is running)"])
            ),
            "removed: a; not removed: c (Error response from daemon: container is running)"
        );
        assert_eq!(removal_log(&[], &[]), "no containers removed");
        let unnamed = Container {
            id: "16cc18df9eacbacee2d78fbf4633403311bf7e42a103cc293f416278a3165362".into(),
            ..Default::default()
        };
        assert_eq!(label(&unnamed), "16cc18df9eac");
    }

    /// The real runtime, with two containers of its own from an image
    /// already here (`MYOUS_TEST_IMAGE`, e.g. alpine; nothing is pulled),
    /// labelled as workers of a project of their own: one created and never
    /// started, one running. A plain `rm` refuses the running one; `rm -f`
    /// removes it. Only its own two containers are touched.
    #[test]
    #[ignore]
    fn real_engine_lists_and_removes() {
        let Ok(image) = std::env::var("MYOUS_TEST_IMAGE") else {
            panic!("set MYOUS_TEST_IMAGE to an image already pulled (alpine:3.22)");
        };
        let b = bin();
        let project = format!("myous-buildtest-{}", std::process::id());
        let make = |name: &str, run: bool| {
            let mut args = vec![if run { "run" } else { "create" }];
            if run {
                args.push("-d");
            }
            let label = format!("com.docker.compose.project={project}");
            let full = format!("{project}-{name}");
            args.extend([
                "--label",
                "com.myoushq.worker=1",
                "--label",
                &label,
                "--name",
                &full,
            ]);
            args.extend(["--entrypoint", "sleep", &image, "600"]);
            let id = output(&b, &args, Duration::from_secs(60));
            assert!(is_id(&id), "{args:?}: {id}");
            id
        };
        let created = make("created", false);
        let running = make("running", true);
        let cleanup = || {
            for id in [&created, &running] {
                let _ = output(&b, &["rm", "-f", id], Duration::from_secs(60));
            }
        };
        let mine = |list: &[Container]| -> Vec<Container> {
            list.iter()
                .filter(|c| c.project == project)
                .cloned()
                .collect()
        };
        let found = mine(&containers().unwrap_or_default());
        let get = |id: &str| found.iter().find(|c| c.id == id).cloned();
        let (c, r) = (get(&created), get(&running));
        if c.is_none() || r.is_none() {
            cleanup();
            panic!("not listed: {found:?}");
        }
        let (c, r) = (c.unwrap(), r.unwrap());
        println!("listed: {c:?}\n{r:?}");
        let ok = c.name == format!("{project}-created")
            && !c.running
            && c.state == "created"
            && r.running
            && r.state == "running"
            && r.image == image
            && r.version.is_empty();
        // Both as stopped: the running one is refused, not stopped.
        let first = remove(&[c.clone(), r.clone()], &[]);
        println!("{}", first.log);
        let refused = first.removed == vec![c.id.clone()] && first.log.contains("not removed");
        let still = mine(&containers().unwrap_or_default());
        let kept = still.len() == 1 && still[0].id == r.id && still[0].running;
        // Picked as running: removed.
        let second = remove(&[], std::slice::from_ref(&r));
        println!("{}", second.log);
        let gone = mine(&containers().unwrap_or_default()).is_empty();
        cleanup();
        assert!(ok, "listed wrongly");
        assert!(refused && kept, "a plain rm stopped a running one");
        assert_eq!(second.removed, vec![r.id.clone()]);
        assert!(gone);
    }

    #[test]
    fn ports() {
        assert_eq!(last_port("0.0.0.0:32768\n[::]:32768"), "32768");
        assert_eq!(last_port(""), "");
        assert_eq!(last_port(":0"), "");
        assert_eq!(last_port("junk"), "");
    }

    #[test]
    fn streams_lines_and_status() {
        if cfg!(windows) {
            return;
        }
        let mut lines = Vec::new();
        let status = run_streaming(
            &[
                "sh".into(),
                "-c".into(),
                "printf 'a\\rb\\nc'; exit 3".into(),
            ],
            Path::new("."),
            &HashMap::new(),
            |l| lines.push(l),
        );
        assert_eq!(lines, vec!["a", "b", "c"]);
        assert_eq!(status, 3);
        assert_eq!(
            run_streaming(
                &["/nonexistent/x".into()],
                Path::new("."),
                &HashMap::new(),
                |_| {}
            ),
            -1
        );
    }
}

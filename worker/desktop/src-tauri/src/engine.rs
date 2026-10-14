//! The container runtime: find it (Docker, Podman, Docker Desktop,
//! Rancher Desktop), run compose for a worker, stream a command's output,
//! ask for the browser view's port, remove stale containers. Commands are
//! argument lists, never shell strings.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Serialize;

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

/// Removes every container labelled as a myous worker, whatever project
/// started it (the sledgehammer behind Advanced).
pub fn remove_all() -> String {
    let b = bin();
    let ids = output(
        &b,
        &["ps", "-aq", "--filter", "label=com.myoushq.worker"],
        Duration::from_secs(15),
    );
    let ids: Vec<&str> = ids.split_whitespace().collect();
    if ids.is_empty() {
        return "no worker containers found".into();
    }
    let mut args = vec!["rm", "-f"];
    args.extend(ids);
    format!(
        "removed: {}",
        output(&b, &args, Duration::from_secs(60)).replace('\n', " ")
    )
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

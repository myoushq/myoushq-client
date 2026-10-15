//! A worker without a container: the Go worker the app carries inside
//! itself (`myous-worker`, built from the client's go/cmd/myous), run on
//! this computer as the owner, as myous for Mac's direct mode does. For
//! computers that can't run containers (Windows without WSL 2) and for
//! owners who choose it. The review rules are built into the worker (no
//! `--review`): the level in review.json, worker.paused, approvals/.

use std::collections::HashMap;
use std::ffi::OsString;
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
    let current = crate::files::read_json(&home.join("settings.json"))
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
    // Held until the worker is in the map: two starts at once run one worker.
    let mut map = children().lock().unwrap();
    if map.contains_key(home) {
        return Err("this worker is already running".into());
    }
    // A quit file the last worker didn't take (killed first) would stop this one.
    let _ = std::fs::remove_file(paths::quit_file(&dir));
    let log = crate::files::open_append(&paths::worker_log(&dir)).map_err(|e| e.to_string())?;
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
    map.insert(home.to_string(), child);
    drop(map);
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

/// How long a worker asked to stop has to end its command, answer the
/// agent and exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// How much later than a worker's recorded start its process may seem to
/// have begun and still be it: the recorded time is whole seconds, `ps`
/// counts whole seconds too, and the clock may have been set meanwhile.
pub(crate) const START_SLACK: i64 = 5;

/// A worker being stopped: `pid`, this app's child for `home` if `ours`;
/// else one started outside this app, whose worker.json says it started
/// at `started` (Unix seconds; None from a worker that doesn't say).
struct Stopping {
    home: String,
    pid: u32,
    ours: bool,
    started: Option<i64>,
}

impl Stopping {
    /// Asks the worker to stop: commands/quit, which it takes within a
    /// quarter second on every system (on Windows, where a worker without
    /// a console can't be sent a signal, the only way), and elsewhere
    /// SIGTERM too. Either way it ends the command it runs (its process
    /// group or tree), answers that request and exits.
    fn ask(&self) {
        let _ = paths::send_command(Path::new(&self.home), paths::QUIT_COMMAND);
        if !cfg!(windows) {
            signal(self.pid, "-TERM");
        }
    }

    fn gone(&self) -> bool {
        if self.ours {
            // start's thread reaps it and drops it from the map once it exits.
            children().lock().unwrap().get(&self.home).map(|c| c.id()) != Some(self.pid)
        } else {
            // Exited, or its pid is another program's now.
            matches!(check(self.pid, self.started), Check::NotWorker(_))
        }
    }

    /// Ends it now, with what it started: SIGKILL, or taskkill /T /F. One
    /// this app didn't start, only while its pid is still that worker.
    /// True if it was ended.
    fn kill(&self) -> bool {
        if !self.ours && check(self.pid, self.started) != Check::Worker {
            return false;
        }
        if cfg!(windows) {
            let mut cmd = crate::engine::command("taskkill");
            cmd.args(["/T", "/F", "/PID", &self.pid.to_string()]);
            let _ = run_capture(cmd, Duration::from_secs(15));
        } else {
            signal(self.pid, "-KILL");
        }
        true
    }
}

/// Asks the workers to stop, waits up to STOP_GRACE for them to exit,
/// then kills the ones still there. The pids it killed.
fn end(workers: Vec<Stopping>) -> Vec<u32> {
    for w in &workers {
        w.ask();
    }
    let begun = std::time::Instant::now();
    let mut left = workers;
    while !left.is_empty() && begun.elapsed() < STOP_GRACE {
        std::thread::sleep(Duration::from_millis(250));
        left.retain(|w| !w.gone());
    }
    left.into_iter()
        .filter(|w| w.kill())
        .map(|w| w.pid)
        .collect()
}

/// `kill <sig> <pid>`: true if it succeeded.
fn signal(pid: u32, sig: &str) -> bool {
    let mut cmd = crate::engine::command("kill");
    cmd.args([sig, &pid.to_string()]);
    run_capture(cmd, Duration::from_secs(15)).0 == 0
}

/// What the system says about a process.
#[derive(Debug, Clone)]
pub(crate) struct Proc {
    /// Its program's file name, in lower case, without `.exe`.
    pub(crate) name: String,
    /// When it started (Unix seconds).
    pub(crate) started: i64,
    /// Exited but not yet waited for by its parent.
    pub(crate) zombie: bool,
}

/// Whether a pid from worker.json is still that worker.
#[derive(Debug, PartialEq)]
enum Check {
    Worker,
    /// Not (any more), and why.
    NotWorker(String),
    /// The system couldn't be asked, and why.
    Unknown(String),
}

/// Whether `pid` is still the worker that recorded it in worker.json with
/// `recorded` as its start: a myous worker program that began no later
/// than that (START_SLACK aside). A pid the system has since given to
/// another program fails one or the other; one that has exited, both.
fn check(pid: u32, recorded: Option<i64>) -> Check {
    if pid == 0 || pid == std::process::id() {
        return Check::NotWorker("that is no worker's pid".into());
    }
    judge(process(pid), recorded)
}

/// `check`'s decision, from what the system said about the process.
fn judge(found: Result<Option<Proc>, String>, recorded: Option<i64>) -> Check {
    let p = match found {
        Err(why) => return Check::Unknown(why),
        Ok(None) => return Check::NotWorker("no process has it".into()),
        Ok(Some(p)) => p,
    };
    if p.zombie {
        return Check::NotWorker("it has exited".into());
    }
    if let Some(r) = recorded {
        if p.started > r + START_SLACK {
            return Check::NotWorker(format!("{} has it, started after the worker", p.name));
        }
    }
    if !worker_program(&p.name) {
        return Check::NotWorker(format!("{} has it", p.name));
    }
    Check::Worker
}

/// A program a worker runs as: the Go worker (`myous-worker`, bundled
/// with this app, or `myous` from the Go client) or the Python client's
/// (`myous`, a script that runs as Python, or `python -m myous`).
fn worker_program(name: &str) -> bool {
    let n = name.to_lowercase();
    let n = n.strip_suffix(".exe").unwrap_or(&n);
    n.starts_with("myous") || n.starts_with("python")
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The process `pid` as the system has it: None if there is none, an
/// error if the system can't be asked. `ps` on a Mac and Linux; on
/// Windows PowerShell's Get-Process (the start time the Python client's
/// pid-reuse guard reads through GetProcessTimes).
pub(crate) fn process(pid: u32) -> Result<Option<Proc>, String> {
    if cfg!(windows) {
        let script = format!(
            "$p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; if (-not $p) {{ exit 0 }}; \
             $t = $p.StartTime; if (-not $t) {{ Write-Output 'no start time for it'; exit 3 }}; \
             Write-Output ('{{0}} {{1}}' -f ([DateTimeOffset]$t).ToUnixTimeSeconds(), $p.ProcessName)"
        );
        let mut cmd = crate::engine::command("powershell.exe");
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]);
        let (st, out) = run_capture(cmd, Duration::from_secs(30));
        parse_get_process(st, &out)
    } else {
        let mut cmd = crate::engine::command("ps");
        cmd.args(["-o", "stat=", "-o", "etime=", "-o", "comm=", "-p"])
            .arg(pid.to_string());
        let (st, out) = run_capture(cmd, Duration::from_secs(15));
        parse_ps(st, &out, unix_now())
    }
}

/// `ps -o stat= -o etime= -o comm= -p <pid>`'s answer, at `now`: nothing
/// and a failure if there is no such process. `comm` is the program's
/// path on a Mac, its name on Linux.
fn parse_ps(status: i32, out: &str, now: i64) -> Result<Option<Proc>, String> {
    let out = out.trim();
    if status == 127 {
        return Err(format!("ps didn't run: {out}"));
    }
    if status != 0 {
        return if out.is_empty() {
            Ok(None)
        } else {
            Err(format!("ps: {out}"))
        };
    }
    let bad = || format!("ps said {out:?}");
    let (stat, rest) = out.split_once(char::is_whitespace).ok_or_else(bad)?;
    let (etime, comm) = rest
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or_else(bad)?;
    let elapsed = parse_etime(etime).ok_or_else(bad)?;
    let comm = comm.trim();
    let name = comm.rsplit('/').next().unwrap_or(comm).to_lowercase();
    Ok(Some(Proc {
        name,
        started: now - elapsed,
        zombie: stat.starts_with('Z'),
    }))
}

/// The command line the process `pid` was started with, as one line (its
/// arguments joined with spaces; on Windows as Windows keeps it, quotes
/// and all): None if there is no such process, an error if the system
/// can't be asked. `ps -ww -o args=` on a Mac and Linux, CIM's
/// Win32_Process on Windows.
pub(crate) fn command_line(pid: u32) -> Result<Option<String>, String> {
    if cfg!(windows) {
        let script = format!(
            "$p = Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}' -ErrorAction SilentlyContinue; \
             if (-not $p) {{ exit 0 }}; Write-Output ('cmd ' + $p.CommandLine)"
        );
        let mut cmd = crate::engine::command("powershell.exe");
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ]);
        let (st, out) = run_capture(cmd, Duration::from_secs(30));
        if st != 0 {
            return Err(format!("PowerShell's Get-CimInstance: {out}"));
        }
        let line = out
            .lines()
            .map(str::trim)
            .find_map(|l| l.strip_prefix("cmd "));
        return Ok(line.map(String::from));
    }
    let mut cmd = crate::engine::command("ps");
    cmd.args(["-ww", "-o", "args=", "-p"]).arg(pid.to_string());
    let (st, out) = run_capture(cmd, Duration::from_secs(15));
    let out = out.trim();
    match st {
        0 if !out.is_empty() => Ok(Some(out.to_string())),
        0 => Err("ps said nothing".into()),
        127 => Err(format!("ps didn't run: {out}")),
        _ if out.is_empty() => Ok(None),
        _ => Err(format!("ps: {out}")),
    }
}

/// `ps`'s elapsed time, `[[dd-]hh:]mm:ss`, in seconds.
fn parse_etime(s: &str) -> Option<i64> {
    let (days, clock) = match s.split_once('-') {
        Some((d, c)) => (d.parse::<i64>().ok()?, c),
        None => (0, s),
    };
    let mut secs = 0;
    let parts: Vec<&str> = clock.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    for p in parts {
        secs = secs * 60 + p.parse::<i64>().ok()?;
    }
    Some(days * 86400 + secs)
}

/// The Windows script's answer: nothing if there is no such process,
/// else `<start, Unix seconds> <process name>`.
fn parse_get_process(status: i32, out: &str) -> Result<Option<Proc>, String> {
    let line = out.lines().map(str::trim).rfind(|l| !l.is_empty());
    if status != 0 {
        return Err(format!(
            "PowerShell's Get-Process: {}",
            line.unwrap_or("no answer")
        ));
    }
    let Some(line) = line else { return Ok(None) };
    let bad = || format!("Get-Process said {line:?}");
    let (secs, name) = line.split_once(' ').ok_or_else(bad)?;
    Ok(Some(Proc {
        name: name.trim().to_lowercase(),
        started: secs.parse().map_err(|_| bad())?,
        zombie: false,
    }))
}

/// Stops the worker this app started for `home`, else the process `pid`
/// (from worker.json, a worker started outside this app), in the
/// background. True if it was this app's: its exit is then reported by
/// `start`'s callback. Else `done` is called once the stop is over: the
/// worker exited or was killed, or there was nothing to stop. A worker
/// this app didn't start is asked to stop (and killed after STOP_GRACE)
/// only while its pid is still that worker (`check`); if that can't be
/// told, it is left alone. `log` says which.
pub fn stop(
    home: &str,
    pid: u32,
    log: crate::browser::Log,
    done: impl FnOnce() + Send + 'static,
) -> bool {
    let ours = children().lock().unwrap().get(home).map(|c| c.id());
    if let Some(ours) = ours {
        let w = Stopping {
            home: home.to_string(),
            pid: ours,
            ours: true,
            started: None,
        };
        std::thread::spawn(move || end(vec![w]));
        return true;
    }
    if pid == 0 {
        done();
        return false;
    }
    let status = crate::files::read_json(&paths::status_file(Path::new(home))).unwrap_or_default();
    if status["pid"].as_u64() != Some(pid as u64) {
        log(format!(
            "worker.json no longer names pid {pid}; nothing to stop"
        ));
        done();
        return false;
    }
    let w = Stopping {
        home: home.to_string(),
        pid,
        ours: false,
        started: status["started"].as_f64().map(|s| s as i64),
    };
    std::thread::spawn(move || {
        match check(pid, w.started) {
            Check::Worker => {}
            Check::NotWorker(why) => {
                log(format!(
                    "pid {pid} in worker.json is no longer a myous worker ({why}); nothing to stop"
                ));
                return done();
            }
            Check::Unknown(why) => {
                log(format!(
                    "can't tell whether pid {pid} in worker.json is still the myous worker ({why}); not stopping it"
                ));
                return done();
            }
        }
        log(format!(
            "stopping myous worker (pid {pid}, started outside this app)"
        ));
        if end(vec![w]).is_empty() {
            log(format!("myous worker (pid {pid}) exited"));
        } else {
            log(format!(
                "myous worker (pid {pid}) didn't stop in {} seconds; killed",
                STOP_GRACE.as_secs()
            ));
        }
        done();
    });
    false
}

/// Stops every worker this app started (when it quits), waiting for them
/// (STOP_GRACE at most) so that none is left behind.
pub fn stop_all() {
    let workers: Vec<Stopping> = children()
        .lock()
        .unwrap()
        .iter()
        .map(|(home, c)| Stopping {
            home: home.clone(),
            pid: c.id(),
            ours: true,
            started: None,
        })
        .collect();
    end(workers);
}

/// Whether this app runs the worker for `home` now.
pub fn running(home: &str) -> bool {
    children().lock().unwrap().contains_key(home)
}

/// The end of worker.log.
pub fn log_tail(home: &str, max: usize) -> String {
    let bytes = crate::files::read_file(&paths::worker_log(Path::new(home))).unwrap_or_default();
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

    /// A stand-in worker: `sh` (PowerShell on Windows) running `posix`
    /// (`windows`), from the worker's folder as start runs it.
    fn script(posix: &str, windows: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["powershell.exe", "-NoProfile", "-Command", windows]
        } else {
            vec!["sh", "-c", posix]
        }
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn wait_for_log(h: &str, line: &str) {
        // PowerShell takes seconds on a cold runner.
        let begun = std::time::Instant::now();
        while !log_tail(h, 1000).contains(line) && begun.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(log_tail(h, 1000).contains(line), "{}", log_tail(h, 1000));
    }

    #[test]
    fn start_and_stop() {
        let home = std::env::temp_dir().join(format!("myous-direct-run-{}", std::process::id()));
        let h = home.to_string_lossy().to_string();
        let quit = crate::paths::quit_file(&home);
        // One the last worker didn't take: gone before the next starts.
        crate::paths::send_command(&home, crate::paths::QUIT_COMMAND).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let args = script(
            "echo started; sleep 30",
            "Write-Output started; Start-Sleep 30",
        );
        start(&h, &args, move |st| tx.send(st).unwrap()).unwrap();
        assert!(!quit.exists());
        assert!(running(&h));
        assert!(start(&h, &args, |_| {}).is_err());
        wait_for_log(&h, "started");
        // SIGTERM ends it; on Windows, where only the quit file asks (and
        // this one doesn't take it), it is killed after STOP_GRACE.
        assert!(stop(&h, 0, quiet(), || {}));
        assert_ne!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), 0);
        assert!(!running(&h));
        assert!(quit.exists(), "the quit file was not written");
        assert!(home.join("work").is_dir());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn two_starts_at_once_run_one_worker() {
        let home = std::env::temp_dir().join(format!("myous-direct-twice-{}", std::process::id()));
        let h = home.to_string_lossy().to_string();
        let args = script(
            "echo started; sleep 30",
            "Write-Output started; Start-Sleep 30",
        );
        let all = std::sync::Arc::new(std::sync::Barrier::new(8));
        let (tx, rx) = std::sync::mpsc::channel();
        let starts: Vec<_> = (0..8)
            .map(|_| {
                let (h, args, all, tx) = (h.clone(), args.clone(), all.clone(), tx.clone());
                std::thread::spawn(move || {
                    all.wait();
                    start(&h, &args, move |st| {
                        let _ = tx.send(st);
                    })
                })
            })
            .collect();
        let results: Vec<_> = starts.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert!(results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| e.contains("already running")));
        wait_for_log(&h, "started");
        assert!(stop(&h, 0, quiet(), || {}));
        rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(log_tail(&h, 1000).matches("started").count(), 1);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_worker_that_takes_the_quit_file_exits_on_its_own() {
        // As the Go worker does: on Windows the only graceful way.
        let home = std::env::temp_dir().join(format!("myous-direct-quit-{}", std::process::id()));
        let h = home.to_string_lossy().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        let args = script(
            "trap '' TERM; echo started; while [ ! -e commands/quit ]; do sleep 0.1; done; rm commands/quit; exit 0",
            "Write-Output started; while (-not (Test-Path commands/quit)) { Start-Sleep -Milliseconds 100 }; \
             Remove-Item commands/quit; exit 0",
        );
        start(&h, &args, move |st| tx.send(st).unwrap()).unwrap();
        wait_for_log(&h, "started");
        let begun = std::time::Instant::now();
        assert!(stop(&h, 0, quiet(), || {}));
        assert_eq!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), 0);
        assert!(begun.elapsed() < STOP_GRACE, "took {:?}", begun.elapsed());
        assert!(!running(&h));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_worker_that_doesnt_stop_is_killed() {
        let home = std::env::temp_dir().join(format!("myous-direct-kill-{}", std::process::id()));
        let h = home.to_string_lossy().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        let args = script(
            "trap '' TERM; echo started; while :; do sleep 1; done",
            "Write-Output started; while ($true) { Start-Sleep 1 }",
        );
        start(&h, &args, move |st| tx.send(st).unwrap()).unwrap();
        wait_for_log(&h, "started");
        let begun = std::time::Instant::now();
        assert!(stop(&h, 0, quiet(), || {}));
        assert_ne!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), 0);
        assert!(begun.elapsed() >= STOP_GRACE, "took {:?}", begun.elapsed());
        assert!(!running(&h));
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn quiet() -> crate::browser::Log {
        std::sync::Arc::new(|_| {})
    }

    /// A log that keeps its lines (for the tests that don't run on Windows).
    #[cfg(unix)]
    type Lines = std::sync::Arc<Mutex<Vec<String>>>;
    #[cfg(unix)]
    fn recorder() -> (crate::browser::Log, Lines) {
        let lines: Lines = std::sync::Arc::new(Mutex::new(Vec::new()));
        let keep = lines.clone();
        let log: crate::browser::Log = std::sync::Arc::new(move |l| keep.lock().unwrap().push(l));
        (log, lines)
    }

    fn found(name: &str, started: i64) -> Result<Option<Proc>, String> {
        Ok(Some(Proc {
            name: name.into(),
            started,
            zombie: false,
        }))
    }

    #[test]
    fn a_pid_is_stopped_only_while_it_is_still_the_worker() {
        let t = 1_800_000_000;
        // A worker's programs, begun before it recorded its start.
        for name in [
            "myous-worker",
            "myous-worker.exe",
            "myous",
            "myous.exe",
            "python3.13",
            "python",
            "pythonw.exe",
        ] {
            assert_eq!(judge(found(name, t - 1), Some(t)), Check::Worker, "{name}");
        }
        // A moment after, by the clock's grain; or a worker.json without a start.
        assert_eq!(
            judge(found("myous-worker", t + START_SLACK), Some(t)),
            Check::Worker
        );
        assert_eq!(judge(found("myous", t + 3600), None), Check::Worker);
        // The pid given to another program since.
        assert!(matches!(
            judge(found("chrome", t - 100), Some(t)),
            Check::NotWorker(why) if why.contains("chrome")
        ));
        // Or to a worker program that began after the worker did.
        assert!(matches!(
            judge(found("python3", t + START_SLACK + 1), Some(t)),
            Check::NotWorker(_)
        ));
        // Gone, or exited and not yet waited for.
        assert!(matches!(judge(Ok(None), Some(t)), Check::NotWorker(_)));
        let zombie = Proc {
            name: "myous-worker".into(),
            started: t,
            zombie: true,
        };
        assert!(matches!(
            judge(Ok(Some(zombie)), Some(t)),
            Check::NotWorker(_)
        ));
        // No way to tell: left alone.
        assert_eq!(
            judge(Err("ps didn't run".into()), Some(t)),
            Check::Unknown("ps didn't run".into())
        );
        // Never pid 0, nor this app itself (myous.exe on Windows).
        assert!(matches!(check(0, None), Check::NotWorker(_)));
        assert!(matches!(
            check(std::process::id(), None),
            Check::NotWorker(_)
        ));
    }

    #[test]
    fn what_ps_and_get_process_say() {
        let now = 1_800_000_000;
        // A Mac: the program's path, spaces and all.
        let p = parse_ps(0, "Ss   01:02:03 /Applications/My Apps/myous-worker", now)
            .unwrap()
            .unwrap();
        assert_eq!(
            (p.name.as_str(), p.started, p.zombie),
            ("myous-worker", now - 3723, false)
        );
        let p = parse_ps(
            0,
            "SN 2-00:00:05 /usr/local/Cellar/python@3.13/3.13.8/Frameworks/Python.framework/Versions/3.13/Resources/Python.app/Contents/MacOS/Python",
            now,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            (p.name.as_str(), p.started),
            ("python", now - 2 * 86400 - 5)
        );
        // Linux: the name; one that has exited.
        let p = parse_ps(0, "Z+      00:07 myous-worker", now)
            .unwrap()
            .unwrap();
        assert!(p.zombie);
        assert_eq!(p.started, now - 7);
        // No such process; no ps, or an answer that isn't one.
        assert!(parse_ps(1, "", now).unwrap().is_none());
        assert!(parse_ps(127, "No such file or directory", now).is_err());
        assert!(parse_ps(1, "ps: illegal option", now).is_err());
        assert!(parse_ps(0, "garbage", now).is_err());
        assert!(parse_ps(0, "S 1:2:3:4 myous", now).is_err());
        // Windows.
        let p = parse_get_process(0, "1800000000 myous-worker\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(
            (p.name.as_str(), p.started),
            ("myous-worker", 1_800_000_000)
        );
        assert!(parse_get_process(0, "").unwrap().is_none());
        assert!(parse_get_process(3, "no start time for it").is_err());
        assert!(parse_get_process(127, "program not found").is_err());
        assert!(parse_get_process(0, "soon python").is_err());
    }

    /// What the system says about a process this test starts.
    #[test]
    fn a_real_process_and_its_start() {
        let begun = unix_now();
        let (c, name) = if cfg!(windows) {
            let c = Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", "Start-Sleep 60"])
                .spawn();
            (c, "powershell")
        } else {
            (Command::new("sleep").arg("60").spawn(), "sleep")
        };
        let mut c = c.unwrap();
        let p = process(c.id()).unwrap().expect("no such process");
        assert_eq!(p.name, name);
        assert!(!p.zombie);
        assert!(
            p.started >= begun - 2 && p.started <= unix_now() + 2,
            "started {} at {}",
            p.started,
            unix_now()
        );
        assert!(matches!(
            check(c.id(), Some(unix_now())),
            Check::NotWorker(why) if why.contains(name)
        ));
        let _ = c.kill();
        let _ = c.wait();
        if !cfg!(windows) {
            // (Windows hands pids out again soon.)
            assert!(process(c.id()).unwrap().is_none());
        }
    }

    #[cfg(unix)]
    fn write_status(home: &Path, pid: u32, started: i64) {
        std::fs::write(
            crate::paths::status_file(home),
            format!(r#"{{"pid": {pid}, "started": {started}, "phase": "running"}}"#),
        )
        .unwrap();
    }

    /// Python, a program a worker runs as, standing in for a worker
    /// started outside this app (running `code` after `import signal,
    /// time`); None without a python3.
    #[cfg(unix)]
    fn outside_worker_running(code: &str) -> Option<Child> {
        crate::engine::which("python3")?;
        Some(
            Command::new("python3")
                .args(["-c", &format!("import signal, time\n{code}")])
                .spawn()
                .unwrap(),
        )
    }

    #[cfg(unix)]
    fn outside_worker() -> Option<Child> {
        outside_worker_running("time.sleep(60)")
    }

    /// Stops `pid` as the window does for a worker this app didn't start;
    /// what the log said by the time the stop was over.
    #[cfg(unix)]
    fn stop_outside(h: &str, pid: u32) -> String {
        let (log, lines) = recorder();
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(!stop(h, pid, log, move || tx.send(()).unwrap()));
        rx.recv_timeout(Duration::from_secs(30)).unwrap();
        let said = lines.lock().unwrap().join("\n");
        said
    }

    #[cfg(unix)]
    #[test]
    fn a_worker_started_outside_the_app_is_stopped_through_worker_json() {
        let Some(mut c) = outside_worker() else {
            println!("no python3; skipped");
            return;
        };
        let home = temp("outside");
        let h = home.to_string_lossy().to_string();
        // It records its start a moment after its process began.
        std::thread::sleep(Duration::from_millis(300));
        write_status(&home, c.id(), unix_now());
        let lines = stop_outside(&h, c.id());
        // SIGTERM ended it.
        assert!(c.try_wait().unwrap().is_some(), "still running");
        assert!(lines.contains("started outside this app"), "{lines}");
        assert!(lines.contains("exited"), "{lines}");
        assert!(crate::paths::quit_file(&home).exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_worker_started_outside_the_app_that_doesnt_stop_is_killed() {
        let Some(mut c) = outside_worker_running(
            "signal.signal(signal.SIGTERM, signal.SIG_IGN)\nprint('started', flush=True)\ntime.sleep(60)",
        ) else {
            println!("no python3; skipped");
            return;
        };
        let home = temp("outside-kill");
        let h = home.to_string_lossy().to_string();
        // Past Python's start (and its SIGTERM handler) before the stop.
        std::thread::sleep(Duration::from_millis(1000));
        write_status(&home, c.id(), unix_now());
        let begun = std::time::Instant::now();
        let lines = stop_outside(&h, c.id());
        assert!(begun.elapsed() >= STOP_GRACE, "took {:?}", begun.elapsed());
        assert!(c.try_wait().unwrap().is_some(), "still running");
        assert!(
            lines.contains("didn't stop in 10 seconds; killed"),
            "{lines}"
        );
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_pid_that_is_no_longer_the_worker_is_left_alone() {
        let home = temp("reused");
        let h = home.to_string_lossy().to_string();
        // Another program has the pid now.
        let mut other = Command::new("sleep").arg("60").spawn().unwrap();
        write_status(&home, other.id(), unix_now());
        let lines = stop_outside(&h, other.id());
        assert!(
            lines.contains("no longer a myous worker (sleep has it)"),
            "{lines}"
        );
        // worker.json names another pid by now.
        let lines = stop_outside(&h, other.id() + 1);
        assert!(lines.contains("no longer names"), "{lines}");
        assert!(other.try_wait().unwrap().is_none(), "it was signalled");
        let _ = other.kill();
        let _ = other.wait();
        // A worker program that began after the worker did.
        if let Some(mut later) = outside_worker() {
            write_status(&home, later.id(), unix_now() - 3600);
            let lines = stop_outside(&h, later.id());
            assert!(lines.contains("started after the worker"), "{lines}");
            assert!(later.try_wait().unwrap().is_none(), "it was signalled");
            let _ = later.kill();
            let _ = later.wait();
        }
        // Not asked through the quit file either.
        assert!(!crate::paths::quit_file(&home).exists());
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

        assert!(stop(&h, 0, quiet(), || {}));
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

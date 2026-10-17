//! The files the worker and the app write in a worker's folder, read as
//! plain JSON: the window decides what they mean.
//!
//! A worker in a container has its folder mounted read and write
//! (`/home/worker/.myous`), so anything in it may be a symbolic link an
//! agent's command made, to the owner's `~/.zshrc` say, or a named pipe.
//! The app runs as the owner, unconfined, so it never writes, appends,
//! changes the mode of or reads a file there through a link: a file is
//! written under a temporary name of its own (O_EXCL, which never follows
//! a link) and renamed into place, which replaces a link rather than
//! following it (`write_atomic`, `fresh_file`); a log is opened with
//! O_NOFOLLOW (`open_append`); a read takes only a plain file
//! (`read_file`); a folder the app writes into is refused when it, or a
//! folder between it and the worker folder, is a link (`dir_in`).

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde_json::Value;

use crate::paths;

/// Everything the window needs about one worker, read in one go.
#[derive(Serialize)]
pub struct WorkerFiles {
    pub home: String,
    pub folder: String,
    pub is_default: bool,
    pub project: String,
    pub config: Value,
    pub status: Value,
    pub status_modified: f64,
    pub requests: Vec<Value>,
    pub approvals: Vec<Value>,
    pub paused: bool,
    pub review: String,
    /// browser.json: the worker's browser on this computer while it runs.
    pub browser: Value,
}

pub fn read_worker(home: &Path) -> WorkerFiles {
    let (status, status_modified) = read_status(home);
    WorkerFiles {
        home: home.to_string_lossy().to_string(),
        folder: home
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        is_default: paths::is_default(home),
        project: paths::project(home),
        config: read_json(&paths::config_file(home)).unwrap_or(Value::Null),
        status,
        status_modified,
        requests: load_records(&paths::requests_dir(home), "at", true, 200),
        approvals: load_records(&paths::approvals_dir(home), "asked_at", false, 50),
        paused: paths::is_paused(home),
        review: review_level(home),
        browser: read_json(&paths::browser_json(home)).unwrap_or(Value::Null),
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    let data = read_file(path).ok()?;
    serde_json::from_slice(&data).ok()
}

fn read_status(home: &Path) -> (Value, f64) {
    let path = paths::status_file(home);
    let modified = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    (read_json(&path).unwrap_or(Value::Null), modified)
}

fn num(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Reads every *.json record in a folder, sorted by `key` (newest first
/// when `desc`), at most `limit`.
fn load_records(dir: &Path, key: &str, desc: bool, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            if let Some(v) = read_json(&e.path()) {
                if v.is_object() {
                    out.push(v);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        let (x, y) = (num(a, key), num(b, key));
        if desc {
            y.partial_cmp(&x)
        } else {
            x.partial_cmp(&y)
        }
        .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(limit);
    out
}

// --- files in a worker's folder, never through a link ---------------------------

/// Whether `p` itself is a symbolic link (or a Windows junction).
pub fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn link_error(p: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{} is a symbolic link; myous doesn't write through links in a worker's folder",
            p.display()
        ),
    )
}

/// What a failed open says when the name is a link (O_NOFOLLOW's ELOOP)
/// or a named pipe nobody reads (O_NONBLOCK's ENXIO): something to take
/// away and replace, not to open.
fn not_a_plain_file(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(e.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENXIO))
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Opens with these options, never through a link at the last name
/// (O_NOFOLLOW; on Windows a link there is refused first) and never
/// waiting on a named pipe (O_NONBLOCK, cleared again once the file is
/// known to be a plain one, so a program given it writes as usual).
fn open_nofollow(o: &mut fs::OpenOptions, path: &Path) -> io::Result<fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if is_link(path) {
        return Err(link_error(path));
    }
    let f = o.open(path)?;
    if !f.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a plain file", path.display()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: fcntl on a descriptor this File owns.
        unsafe {
            let fd = f.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }
    Ok(f)
}

/// A file's contents, if it is a plain file: never a link's target (a
/// link to the owner's files would carry them into what the window shows,
/// and from there back into the folder), never a pipe's (it would block).
pub fn read_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut f = open_nofollow(fs::OpenOptions::new().read(true), path)?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)?;
    Ok(data)
}

static TMP: AtomicU64 = AtomicU64::new(0);

/// A temporary name in `dir` for `path`, this process's own.
fn tmp_name(dir: &Path, path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let n = TMP.fetch_add(1, Ordering::SeqCst);
    dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()))
}

/// A new file under a name that wasn't there: O_EXCL never opens a link
/// (or anything else already there).
fn create_new(path: &Path) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NOFOLLOW);
    }
    o.open(path)
}

/// Writes a file through a temporary name, so a reader never sees half of
/// it, and so that a link at `path` is replaced, never written through.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    write_atomic_via(dir, path, data)
}

/// `write_atomic` with the temporary name in `dir` (on the same disk):
/// for a folder the worker reads every name of (commands/), where a
/// temporary name would be taken for a command.
pub fn write_atomic_via(dir: &Path, path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = tmp_name(dir, path);
    let result = (|| {
        let mut f = create_new(&tmp)?;
        f.write_all(data)?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A new, empty file at `path`, open for writing (a log a program writes
/// from its start): made under a temporary name and renamed into place.
pub fn fresh_file(path: &Path) -> io::Result<fs::File> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let tmp = tmp_name(dir, path);
    let f = create_new(&tmp)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(f)
}

/// Opens a log to append to, made if it isn't there. A link or a pipe in
/// its place is taken away (the link, never what it points to) and the
/// log starts afresh.
pub fn open_append(path: &Path) -> io::Result<fs::File> {
    let mut last = None;
    for _ in 0..3 {
        if cfg!(windows) && is_link(path) {
            let _ = fs::remove_file(path);
        }
        match open_nofollow(fs::OpenOptions::new().append(true).create(true), path) {
            Ok(f) => return Ok(f),
            Err(e) if not_a_plain_file(&e) || e.kind() == io::ErrorKind::InvalidInput => {
                let _ = fs::remove_file(path);
                last = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| link_error(path)))
}

/// Makes the folder `dir` inside the worker folder `home`, and each one
/// between them, refusing any that is a link (or not a folder): what the
/// app writes there stays in the worker folder. The worker folder itself
/// (the owner's, never the container's to replace) is made if missing.
pub fn dir_in(home: &Path, dir: &Path) -> io::Result<()> {
    let rel = dir.strip_prefix(home).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not in {}", dir.display(), home.display()),
        )
    })?;
    fs::create_dir_all(home)?;
    let mut p = home.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(n) => p.push(n),
            Component::CurDir => continue,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} leaves {}", dir.display(), home.display()),
                ))
            }
        }
        match fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => return Err(link_error(&p)),
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a folder", p.display()),
                ))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&p) {
                Ok(()) => {}
                // Made meanwhile: checked again.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if is_link(&p) || !p.is_dir() {
                        return Err(link_error(&p));
                    }
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `dir_in`, and the folder private to the owner (0700), its mode set on
/// the folder itself, never through a link.
pub fn private_dir_in(home: &Path, dir: &Path) -> io::Result<()> {
    dir_in(home, dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let d = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(dir)?;
        d.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Stores app.json (pretty, sorted keys, like the Mac app).
pub fn write_config(home: &Path, config: &Value) -> io::Result<()> {
    paths::ensure(home)?;
    let mut text = serde_json::to_string_pretty(config)?;
    text.push('\n');
    write_atomic(&paths::config_file(home), text.as_bytes())
}

/// Whether a request id is a plain name: 1 to 64 characters from A-Z,
/// a-z, 0-9, - and _. The workers and review.py hold ids to the same rule
/// (they refuse a request with any other id), and ids name the files the
/// app writes (approvals/<id>.answer, commands/stop-<id>).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

/// Whether `name` is a command the worker takes from commands/: new-code,
/// unpair, or stop-<id> for a request id.
pub fn valid_command(name: &str) -> bool {
    matches!(name, "new-code" | "unpair") || name.strip_prefix("stop-").is_some_and(valid_id)
}

/// Writes approvals/<id>.answer with "allow" or "refuse".
pub fn answer_approval(home: &Path, id: &str, verdict: &str) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a request id: {id:?}"),
        ));
    }
    let dir = paths::approvals_dir(home);
    dir_in(home, &dir)?;
    write_atomic(&dir.join(format!("{id}.answer")), verdict.as_bytes())
}

/// review.json's level as the workers read it: trust, changes or all.
/// No file is a worker never given a level, which asks before changes (a
/// container's default since 0.8.1, and what the app writes without a
/// container). A file that is there but can't be read (a link or a pipe
/// included: the app reads only a plain file), isn't JSON, isn't an
/// object or has no valid level is all, as the workers fail closed: a
/// truncated or torn file asks about everything rather than running it.
pub fn review_level(home: &Path) -> String {
    const FAIL_CLOSED: &str = "all";
    let data = match read_file(&paths::review_file(home)) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return "changes".to_string(),
        Err(_) => return FAIL_CLOSED.to_string(),
    };
    let v: Value = serde_json::from_slice(&data).unwrap_or(Value::Null);
    match v.get("level").and_then(Value::as_str) {
        Some(l @ ("trust" | "changes" | "all")) => l.to_string(),
        _ => FAIL_CLOSED.to_string(),
    }
}

pub fn set_review_level(home: &Path, level: &str) -> io::Result<()> {
    paths::ensure(home)?;
    let data = serde_json::json!({ "level": level }).to_string();
    write_atomic(&paths::review_file(home), data.as_bytes())
}

/// Whether a worker folder holds a worker that was set up (the window's
/// `isSetUp`): app.json names it, or the worker has written worker.json.
pub fn is_set_up(home: &Path) -> bool {
    let named = read_json(&paths::config_file(home))
        .and_then(|c| c.get("name").and_then(Value::as_str).map(|n| !n.is_empty()))
        .unwrap_or(false);
    named || read_json(&paths::status_file(home)).is_some_and(|s| !s.is_null())
}

/// The level a set-up worker without a review.json is given at the app's
/// start: the one the window shows for it (`review_level`). Written once,
/// it is what every worker acts on, whatever it does without the file: a
/// container from before 0.8.1 runs everything, and a container's `.env`
/// may set another default the app doesn't read. Settings changes it as
/// any other level. A worker not set up yet is left to Set up, which
/// writes the level the owner chooses; a review.json that is there, even
/// one that can't be read (shown as all), is left as it is. True when it
/// wrote.
pub fn write_missing_review_level(home: &Path) -> io::Result<bool> {
    if !is_set_up(home) {
        return Ok(false);
    }
    match fs::symlink_metadata(paths::review_file(home)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        _ => return Ok(false),
    }
    set_review_level(home, "changes")?;
    Ok(true)
}

/// Appends a line to app.log in the default worker folder.
pub fn append_app_log(line: &str) {
    let home = paths::default_home();
    if paths::ensure(&home).is_err() {
        return;
    }
    if let Ok(mut f) = open_append(&paths::app_log(&home)) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_sorted_and_answers_written() {
        let home = std::env::temp_dir().join(format!("myous-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let req = paths::requests_dir(&home);
        fs::create_dir_all(&req).unwrap();
        fs::write(req.join("a.json"), r#"{"id":"a","at":10}"#).unwrap();
        fs::write(req.join("b.json"), r#"{"id":"b","at":20}"#).unwrap();
        fs::write(req.join("junk.txt"), "x").unwrap();
        fs::write(req.join("bad.json"), "{").unwrap();
        let w = read_worker(&home);
        assert_eq!(w.requests.len(), 2);
        assert_eq!(w.requests[0]["id"], "b");
        assert!(w.config.is_null());
        assert!(w.status.is_null());
        assert_eq!(w.review, "changes");
        answer_approval(&home, "r-5_x", "allow").unwrap();
        assert_eq!(
            fs::read_to_string(paths::approvals_dir(&home).join("r-5_x.answer")).unwrap(),
            "allow"
        );
        answer_approval(&home, "r5/../x", "allow").unwrap_err();
        let answers = fs::read_dir(paths::approvals_dir(&home)).unwrap().count();
        assert_eq!(answers, 1);
        set_review_level(&home, "all").unwrap();
        assert_eq!(review_level(&home), "all");
        write_config(&home, &serde_json::json!({"name": "x"})).unwrap();
        assert_eq!(read_worker(&home).config["name"], "x");
        fs::remove_dir_all(&home).unwrap();
    }

    /// What an agent's command in a container can put in the mounted
    /// worker folder: links to the owner's files and named pipes. Nothing
    /// is written, appended, read or chmodded through them.
    #[cfg(unix)]
    #[test]
    fn nothing_goes_through_a_link_in_the_worker_folder() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let base = std::env::temp_dir().join(format!("myous-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let home = base.join("worker");
        fs::create_dir_all(&home).unwrap();
        // The owner's files, outside the worker folder.
        let zshrc = base.join("zshrc");
        fs::write(&zshrc, "owner's\n").unwrap();
        let secret = base.join("secret.json");
        fs::write(&secret, r#"{"name": "secret", "token": "s3cret"}"#).unwrap();
        let owners = || fs::read_to_string(&zshrc).unwrap();
        let plain = |p: &Path| !is_link(p) && p.is_file();

        // Written whole (compose.yml, app.json, review.json, worker.paused):
        // the link is replaced by the file, its target untouched.
        let compose = paths::compose_file(&home);
        symlink(&zshrc, &compose).unwrap();
        write_atomic(&compose, b"services: {}\n").unwrap();
        assert_eq!(owners(), "owner's\n");
        assert!(plain(&compose));
        assert_eq!(fs::read_to_string(&compose).unwrap(), "services: {}\n");
        for f in [paths::config_file(&home), paths::review_file(&home)] {
            symlink(&secret, &f).unwrap();
        }
        // Read: a link to the owner's JSON reads as nothing, so it never
        // reaches the window (or app.json again when the window saves it).
        let w = read_worker(&home);
        assert!(w.config.is_null(), "{}", w.config);
        // review.json is there but not read (a link): it shows all, failing closed as the workers do.
        assert_eq!(w.review, "all");
        write_config(&home, &serde_json::json!({"name": "x"})).unwrap();
        set_review_level(&home, "all").unwrap();
        assert!(plain(&paths::config_file(&home)) && plain(&paths::review_file(&home)));
        assert!(fs::read_to_string(&secret).unwrap().contains("s3cret"));
        symlink(&zshrc, paths::paused_file(&home)).unwrap();
        paths::set_paused(&home, true).unwrap();
        assert!(plain(&paths::paused_file(&home)) && owners() == "owner's\n");

        // Logs: appended to a new file in the link's place.
        let log = paths::worker_log(&home);
        symlink(&zshrc, &log).unwrap();
        writeln!(open_append(&log).unwrap(), "a line").unwrap();
        assert_eq!(owners(), "owner's\n");
        assert!(plain(&log));
        assert_eq!(fs::read_to_string(&log).unwrap(), "a line\n");
        writeln!(open_append(&log).unwrap(), "another").unwrap();
        assert_eq!(fs::read_to_string(&log).unwrap(), "a line\nanother\n");
        // A dangling link: nothing made where it points.
        let gone = base.join("made-through-a-link");
        let blog = paths::browser_log(&home);
        symlink(&gone, &blog).unwrap();
        let mut f = fresh_file(&blog).unwrap();
        writeln!(f, "fresh").unwrap();
        assert!(!gone.exists() && plain(&blog));
        // A named pipe neither blocks a read nor a log.
        let fifo = paths::app_log(&home);
        let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(read_file(&fifo).is_err());
        writeln!(open_append(&fifo).unwrap(), "logged").unwrap();
        assert!(plain(&fifo));
        // Nor a link to the owner's file read.
        assert!(read_file(&zshrc).is_ok() && read_file(&compose).is_ok());
        symlink(&zshrc, home.join("settings.json")).unwrap();
        assert!(read_file(&home.join("settings.json")).is_err());

        // Command files: renamed into commands/, never through a link
        // there, and no temporary name ever in commands/ (the worker takes
        // every name there for a command).
        let cmds = paths::commands_dir(&home);
        fs::create_dir_all(&cmds).unwrap();
        symlink(&zshrc, cmds.join("new-code")).unwrap();
        paths::send_command(&home, "new-code").unwrap();
        assert!(plain(&cmds.join("new-code")) && owners() == "owner's\n");
        let names: Vec<String> = fs::read_dir(&cmds)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["new-code"]);
        // Folders that are links: refused, nothing written or chmodded there.
        let elsewhere = base.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&cmds).unwrap();
        symlink(&elsewhere, &cmds).unwrap();
        assert!(paths::send_command(&home, "unpair")
            .unwrap_err()
            .to_string()
            .contains("symbolic link"));
        symlink(&elsewhere, paths::approvals_dir(&home)).unwrap();
        assert!(answer_approval(&home, "r1", "allow").is_err());
        let browser = paths::browser_dir(&home);
        symlink(&elsewhere, &browser).unwrap();
        assert!(private_dir_in(&home, &browser.join("chrome")).is_err());
        assert!(private_dir_in(&home, &browser).is_err());
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
        assert_eq!(
            fs::metadata(&elsewhere).unwrap().permissions().mode() & 0o777,
            0o755
        );
        // A plain folder is made, private.
        let dl = home.join("work").join("downloads");
        private_dir_in(&home, &dl).unwrap();
        assert_eq!(
            fs::metadata(&dl).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(dir_in(&home, &base.join("other")).is_err());
        assert!(dir_in(&home, &home.join("..").join("other")).is_err());
        // A new name never opens what is there.
        assert!(create_new(&compose).is_err());
        fs::remove_dir_all(&base).unwrap();
    }

    /// The level shown is the one the workers act on (review.py's and the
    /// Go worker's `level`): none means a worker never given a level, which
    /// asks before changes; a file that is there but isn't a valid level
    /// fails closed to all.
    #[test]
    fn review_level_reads_review_json_as_the_workers_do() {
        let home = std::env::temp_dir().join(format!("myous-review-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        assert_eq!(review_level(&home), "changes", "no worker folder");
        fs::create_dir_all(&home).unwrap();
        assert_eq!(review_level(&home), "changes", "no review.json");
        for level in ["trust", "changes", "all"] {
            set_review_level(&home, level).unwrap();
            assert_eq!(review_level(&home), level);
        }
        let file = paths::review_file(&home);
        for (data, what) in [
            ("", "empty"),
            ("{", "not JSON"),
            ("trust", "not JSON either"),
            (r#"["changes"]"#, "a JSON array"),
            (r#""trust""#, "a JSON string"),
            ("null", "null"),
            ("{}", "no level"),
            (r#"{"level": "bogus"}"#, "a bad level"),
            (r#"{"level": "Trust"}"#, "a level in other letters"),
            (r#"{"level": 1}"#, "a number"),
            (r#"{"level": ["trust"]}"#, "an array"),
            (r#"{"level": null}"#, "null level"),
        ] {
            fs::write(&file, data).unwrap();
            assert_eq!(review_level(&home), "all", "{what}: {data:?}");
        }
        // Not a file: unreadable, so all.
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        assert_eq!(review_level(&home), "all", "a folder");
        fs::remove_dir(&file).unwrap();
        // Written again, it is that level.
        set_review_level(&home, "trust").unwrap();
        assert_eq!(review_level(&home), "trust");
        fs::remove_dir_all(&home).unwrap();
    }

    /// At the app's start a set-up worker without a review.json gets the
    /// level the window shows (changes), once; a worker not set up yet and
    /// a review.json that is there, valid or not, are left alone.
    #[test]
    fn a_set_up_worker_without_review_json_is_given_changes_once() {
        let base =
            std::env::temp_dir().join(format!("myous-default-review-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let file = |h: &Path| paths::review_file(h);
        // Not set up: no folder, an empty app.json, a name that is empty, worker.json null.
        let fresh = base.join("fresh");
        assert!(!write_missing_review_level(&fresh).unwrap());
        assert!(!fresh.exists());
        write_config(&fresh, &serde_json::json!({})).unwrap();
        assert!(!write_missing_review_level(&fresh).unwrap());
        write_config(&fresh, &serde_json::json!({"name": ""})).unwrap();
        fs::write(paths::status_file(&fresh), "null").unwrap();
        assert!(!write_missing_review_level(&fresh).unwrap());
        assert!(!file(&fresh).exists());
        assert_eq!(review_level(&fresh), "changes");
        // Named in app.json (container or direct alike): written, then the file is the truth.
        let named = base.join("named");
        write_config(
            &named,
            &serde_json::json!({"name": "Max's PC", "mode": "image"}),
        )
        .unwrap();
        assert_eq!(review_level(&named), "changes");
        assert!(write_missing_review_level(&named).unwrap());
        assert_eq!(
            fs::read_to_string(file(&named)).unwrap(),
            r#"{"level":"changes"}"#
        );
        assert!(!write_missing_review_level(&named).unwrap(), "once");
        set_review_level(&named, "trust").unwrap();
        assert!(!write_missing_review_level(&named).unwrap());
        assert_eq!(review_level(&named), "trust");
        let direct = base.join("direct");
        write_config(
            &direct,
            &serde_json::json!({"name": "Max's PC", "mode": "direct"}),
        )
        .unwrap();
        assert!(write_missing_review_level(&direct).unwrap());
        assert_eq!(review_level(&direct), "changes");
        // A worker that has run (worker.json) without a name in app.json: set up too.
        let ran = base.join("ran");
        fs::create_dir_all(&ran).unwrap();
        fs::write(paths::status_file(&ran), r#"{"phase": "running"}"#).unwrap();
        assert!(write_missing_review_level(&ran).unwrap());
        assert_eq!(review_level(&ran), "changes");
        // One that is there but not valid stays as it is, shown as all.
        let torn = base.join("torn");
        write_config(&torn, &serde_json::json!({"name": "x"})).unwrap();
        fs::write(file(&torn), "{").unwrap();
        assert!(!write_missing_review_level(&torn).unwrap());
        assert_eq!(fs::read_to_string(file(&torn)).unwrap(), "{");
        assert_eq!(review_level(&torn), "all");
        // A link, even one to nothing, is there: left alone (and shown as all).
        #[cfg(unix)]
        {
            let linked = base.join("linked");
            write_config(&linked, &serde_json::json!({"name": "x"})).unwrap();
            std::os::unix::fs::symlink(base.join("nowhere"), file(&linked)).unwrap();
            assert!(!write_missing_review_level(&linked).unwrap());
            assert!(is_link(&file(&linked)) && !base.join("nowhere").exists());
            assert_eq!(review_level(&linked), "all");
        }
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn ids_follow_the_workers_rule() {
        let x64 = "x".repeat(64);
        for id in [
            "a",
            "a-b_C9",
            "0123456789abcdef0123456789abcdef",
            x64.as_str(),
        ] {
            assert!(valid_id(id), "{id}");
        }
        let x65 = "x".repeat(65);
        for id in [
            "",
            "caf\u{e9}",
            "a.b",
            "a b",
            "a/b",
            "a\\b",
            "../x",
            "a\n",
            x65.as_str(),
        ] {
            assert!(!valid_id(id), "{id:?}");
        }
        let stop_x64 = format!("stop-{x64}");
        for name in ["new-code", "unpair", "stop-a-b_C9", stop_x64.as_str()] {
            assert!(valid_command(name), "{name}");
        }
        for name in ["", "stop-", "stop-a.b", "stop-../x", "new_code", "rm"] {
            assert!(!valid_command(name), "{name:?}");
        }
    }
}

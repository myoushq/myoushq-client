//! The files the worker and the app write in a worker's folder, read as
//! plain JSON: the window decides what they mean.

use std::fs;
use std::io;
use std::path::Path;
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

fn read_json(path: &Path) -> Option<Value> {
    let data = fs::read(path).ok()?;
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

/// Writes a file through a temporary name, so a reader never sees half of it.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)
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
    fs::create_dir_all(&dir)?;
    write_atomic(&dir.join(format!("{id}.answer")), verdict.as_bytes())
}

/// review.json's level: trust (the default), changes or all.
pub fn review_level(home: &Path) -> String {
    let v = read_json(&paths::review_file(home)).unwrap_or(Value::Null);
    match v.get("level").and_then(Value::as_str) {
        Some(l @ ("trust" | "changes" | "all")) => l.to_string(),
        _ => "trust".to_string(),
    }
}

pub fn set_review_level(home: &Path, level: &str) -> io::Result<()> {
    paths::ensure(home)?;
    let data = serde_json::json!({ "level": level }).to_string();
    write_atomic(&paths::review_file(home), data.as_bytes())
}

/// Appends a line to app.log in the default worker folder.
pub fn append_app_log(line: &str) {
    use std::io::Write;
    let home = paths::default_home();
    if paths::ensure(&home).is_err() {
        return;
    }
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::app_log(&home))
    {
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
        assert_eq!(w.review, "trust");
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

    #[test]
    fn ids_follow_the_workers_rule() {
        let x64 = "x".repeat(64);
        for id in ["a", "a-b_C9", "0123456789abcdef0123456789abcdef", x64.as_str()] {
            assert!(valid_id(id), "{id}");
        }
        let x65 = "x".repeat(65);
        for id in ["", "caf\u{e9}", "a.b", "a b", "a/b", "a\\b", "../x", "a\n", x65.as_str()] {
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

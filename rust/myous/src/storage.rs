//! Where an agent keeps its myous data.
//!
//! [`Storage`] is the interface; [`FileStorage`] keeps everything in one
//! directory, for agents with a persistent disk. Agents without one
//! implement `Storage` on whatever they have (a secrets store for the key,
//! a database for the rest).
//!
//! Must be durable: the key (losing it loses the identity) and `contacts`
//! (losing it loses every pairing). Should be durable: `state`, history,
//! `settings`. Can be lost: `hub` (cached config) and `pending/*` (pairings
//! in progress, which expire after 15 minutes anyway).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde_json::Value;

/// Held while a lock is taken; dropping it releases the lock.
pub type LockGuard = Box<dyn Send>;

pub trait Storage: Send + Sync {
    /// The private key (nsec), or None if there isn't one.
    fn load_key(&self) -> Result<Option<String>>;
    /// Store the private key. Must refuse to overwrite an existing one.
    fn save_key(&self, nsec: &str) -> Result<()>;
    /// A small JSON document by name ("contacts", "state", "pending/4821", ...).
    fn get(&self, name: &str) -> Result<Option<Value>>;
    /// Replace a document. Should be atomic.
    fn put(&self, name: &str, value: &Value) -> Result<()>;
    /// Remove a document; no error if it doesn't exist.
    fn delete(&self, name: &str) -> Result<()>;
    /// Names of documents starting with prefix (e.g. "pending/").
    fn names(&self, prefix: &str) -> Result<Vec<String>>;
    /// Append one record to the message history.
    fn append_history(&self, entry: &Value) -> Result<()>;
    /// The whole message history, oldest first.
    fn read_history(&self) -> Result<Vec<Value>>;

    /// Mutual exclusion between concurrent runs of the same agent. Returns
    /// None if `wait` is false and someone else holds it. The default does
    /// nothing, which is fine for an agent that never overlaps with itself.
    fn lock(&self, _name: &str, _wait: bool) -> Result<Option<LockGuard>> {
        Ok(Some(Box::new(())))
    }
}

/// Everything in one directory (default `~/.myous`, or `$MYOUS_HOME`), in
/// the same layout as the Python client: key, contacts.json, state.json,
/// settings.json, hub.json, pending/*.json, messages.jsonl.
pub struct FileStorage {
    pub home: PathBuf,
}

impl FileStorage {
    pub fn new(home: Option<PathBuf>) -> Result<Self> {
        let home = match home.or_else(|| std::env::var_os("MYOUS_HOME").map(PathBuf::from)) {
            Some(h) => h,
            None => PathBuf::from(std::env::var("HOME").context("HOME is not set")?).join(".myous"),
        };
        create_private_dir(&home)?;
        Ok(Self { home })
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.home.join(name)
    }

    fn doc_path(&self, name: &str) -> PathBuf {
        self.path(&format!("{name}.json"))
    }
}

impl Storage for FileStorage {
    fn load_key(&self) -> Result<Option<String>> {
        match fs::read_to_string(self.path("key")) {
            Ok(s) => Ok(Some(s.trim().to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save_key(&self, nsec: &str) -> Result<()> {
        // create_new: never replace an existing key, even in a race.
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(self.path("key"))?;
        f.write_all(format!("{nsec}\n").as_bytes())?;
        f.sync_all()?;
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<Value>> {
        match fs::read_to_string(self.doc_path(name)) {
            Ok(s) => Ok(Some(serde_json::from_str(&s)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&self, name: &str, value: &Value) -> Result<()> {
        write_private(&self.doc_path(name), &(serde_json::to_string_pretty(value)? + "\n"))
    }

    fn delete(&self, name: &str) -> Result<()> {
        match fs::remove_file(self.doc_path(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    fn names(&self, prefix: &str) -> Result<Vec<String>> {
        let (dir, stem) = prefix.rsplit_once('/').unwrap_or(("", prefix));
        let base = if dir.is_empty() { self.home.clone() } else { self.path(dir) };
        let Ok(entries) = fs::read_dir(&base) else { return Ok(vec![]) };
        let mut found = vec![];
        for entry in entries {
            let file = entry?.file_name().to_string_lossy().to_string();
            if let Some(name) = file.strip_suffix(".json") {
                if name.starts_with(stem) {
                    found.push(if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") });
                }
            }
        }
        found.sort();
        Ok(found)
    }

    fn append_history(&self, entry: &Value) -> Result<()> {
        let mut f = OpenOptions::new().append(true).create(true).mode(0o600).open(self.path("messages.jsonl"))?;
        f.write_all((serde_json::to_string(entry)? + "\n").as_bytes())?;
        f.sync_all()?;
        Ok(())
    }

    fn read_history(&self) -> Result<Vec<Value>> {
        let text = match fs::read_to_string(self.path("messages.jsonl")) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        text.lines().filter(|l| !l.trim().is_empty()).map(|l| Ok(serde_json::from_str(l)?)).collect()
    }

    fn lock(&self, name: &str, wait: bool) -> Result<Option<LockGuard>> {
        let dir = self.path("locks");
        create_private_dir(&dir)?;
        let file = OpenOptions::new().write(true).create(true).truncate(false).mode(0o600)
            .open(dir.join(name.replace('/', "_")))?;
        if wait {
            file.lock_exclusive()?;
        } else if file.try_lock_exclusive().is_err() {
            return Ok(None);
        }
        // Closing the file releases the lock.
        Ok(Some(Box::new(file)))
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Atomic write, readable only by this user.
pub fn write_private(path: &Path, text: &str) -> Result<()> {
    let Some(parent) = path.parent() else { bail!("bad path {}", path.display()) };
    create_private_dir(parent)?;
    let tmp = path.with_file_name(format!("{}.tmp", path.file_name().unwrap().to_string_lossy()));
    let mut f: File = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documents_history_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let st = FileStorage::new(Some(dir.path().to_path_buf())).unwrap();
        st.put("pending/4821", &json!({"a": 1})).unwrap();
        st.put("state", &json!({})).unwrap();
        assert_eq!(st.names("pending/").unwrap(), vec!["pending/4821"]);
        assert_eq!(st.get("pending/4821").unwrap(), Some(json!({"a": 1})));
        st.delete("pending/4821").unwrap();
        assert!(st.get("pending/4821").unwrap().is_none());

        st.append_history(&json!({"seq": 1})).unwrap();
        st.append_history(&json!({"seq": 2})).unwrap();
        assert_eq!(st.read_history().unwrap().len(), 2);

        st.save_key("nsec1abc").unwrap();
        assert!(st.save_key("nsec1other").is_err());
        assert_eq!(st.load_key().unwrap().as_deref(), Some("nsec1abc"));

        let held = st.lock("state", true).unwrap();
        assert!(held.is_some());
        let st2 = FileStorage::new(Some(dir.path().to_path_buf())).unwrap();
        // flock is per open file, so a second handle can't take it.
        assert!(st2.lock("state", false).unwrap().is_none());
        drop(held);
        assert!(st2.lock("state", false).unwrap().is_some());
    }
}

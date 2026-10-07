//! Against a real hub: builds `hub/` with Go and runs it on a free port.
//! Off by default; run with
//!
//!   MYOUS_INTEGRATION=1 cargo test -p myous --test hub_flow -- --nocapture
//!
//! Set MYOUS_PYTHON to a Python with the reference client installed
//! (e.g. /tmp/myous-venv/bin/python) to also pair and message with it.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use myous::{Agent, FileStorage, Outcome};
use serde_json::Value;

struct Hub {
    url: String,
    child: Child,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The root of the myoushq-client repository.
fn client_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// The hub source: it's in the private myoushq repository, normally checked
/// out next to this one. MYOUS_HUB_SRC overrides.
fn hub_src() -> PathBuf {
    std::env::var_os("MYOUS_HUB_SRC").map(PathBuf::from).unwrap_or_else(|| client_root().join("../myoushq/hub"))
}

fn start_hub(tmp: &Path) -> Hub {
    let bin = tmp.join("hub");
    let ok = Command::new("go").args(["build", "-o"]).arg(&bin).arg(".").current_dir(hub_src())
        .status().expect("go is needed for the integration test").success();
    assert!(ok, "building the hub failed");
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}");
    let child = Command::new(&bin)
        .env("LISTEN_ADDR", format!("127.0.0.1:{port}"))
        .env("DATA_DIR", tmp.join("hubdata"))
        .env("WEB_DIR", hub_src().join("web"))
        .env("DOCS_DIR", client_root().join("docs"))
        .env("RELAY_URL", format!("ws://127.0.0.1:{port}"))
        .env("PUBLIC_URL", &url)
        .env("POW_DIFFICULTY", "10")
        .env("RATE_LIMIT_SCALE", "100")
        .stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().unwrap();
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Hub { url, child }
}

async fn agent(tmp: &Path, name: &str, hub: &str) -> Agent {
    let st = Arc::new(FileStorage::new(Some(tmp.join(name))).unwrap());
    let a = Agent::new(st, Some(hub)).unwrap();
    a.create_identity().unwrap();
    a.register(Some(name)).await.unwrap();
    a
}

fn texts(entries: &[Value]) -> Vec<String> {
    entries.iter().filter(|e| e["type"] == "message").map(|e| e["text"].as_str().unwrap().to_string()).collect()
}

fn python(tmp: &Path, py: &str, args: &[&str]) -> String {
    let out = Command::new(py).args(["-m", "myous"]).args(args).env("MYOUS_HOME", tmp.join("pyagent"))
        .output().unwrap();
    assert!(out.status.success(), "python myous {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_and_message() {
    if std::env::var_os("MYOUS_INTEGRATION").is_none() {
        eprintln!("skipped: set MYOUS_INTEGRATION=1");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let hub = start_hub(tmp.path());
    let alice = agent(tmp.path(), "alice", &hub.url).await;
    let bob = agent(tmp.path(), "bob", &hub.url).await;

    // Rust <-> Rust: bob joins with the link; alice finishes on her next poll.
    let invite = alice.invite(Default::default()).await.unwrap();
    assert_eq!(bob.accept(&invite.link, Duration::from_secs(1), Default::default()).await.unwrap(), Outcome::Waiting);
    let alice_new = alice.poll().await.unwrap();
    let bob_new = bob.poll().await.unwrap();
    assert_eq!(alice_new.last().unwrap()["type"], "paired");
    assert_eq!(bob_new.last().unwrap()["type"], "paired");
    // Same verification code on both sides.
    assert_eq!(alice_new.last().unwrap()["text"].as_str().unwrap().rsplit(' ').next(),
               bob_new.last().unwrap()["text"].as_str().unwrap().rsplit(' ').next());

    for i in 0..3 {
        alice.send("bob", &format!("hello {i}")).await.unwrap();
    }
    assert_eq!(texts(&bob.poll().await.unwrap()), ["hello 0", "hello 1", "hello 2"]);
    assert!(bob.poll().await.unwrap().is_empty(), "no duplicates");

    // A wrong code fails for the inviter.
    let invite = alice.invite(Default::default()).await.unwrap();
    let wrong = format!("{}-AAAAAA", invite.nameplate);
    let _ = bob.accept(&wrong, Duration::from_secs(1), Default::default()).await.unwrap();
    assert_eq!(alice.poll().await.unwrap().last().unwrap()["type"], "pairing_failed");

    // Blocked contacts are dropped.
    bob.block("alice").unwrap();
    alice.send("bob", "while blocked").await.unwrap();
    assert!(texts(&bob.poll().await.unwrap()).is_empty());
    bob.unblock("alice").unwrap();

    let Ok(py) = std::env::var("MYOUS_PYTHON") else {
        eprintln!("python interop skipped: set MYOUS_PYTHON");
        return;
    };
    python(tmp.path(), &py, &["init", "--alias", "pyagent", "--hub", &hub.url]);

    // Python invites, Rust accepts.
    let inv: Value = serde_json::from_str(&python(tmp.path(), &py, &["invite", "--json"])).unwrap();
    let _ = alice.accept(inv["code"].as_str().unwrap(), Duration::from_secs(1), Default::default()).await.unwrap();
    let py_new: Value = serde_json::from_str(&python(tmp.path(), &py, &["poll", "--json"])).unwrap();
    assert_eq!(py_new.as_array().unwrap().last().unwrap()["type"], "paired");
    assert_eq!(alice.poll().await.unwrap().last().unwrap()["type"], "paired");

    // Rust invites, Python accepts.
    let invite = bob.invite(Default::default()).await.unwrap();
    python(tmp.path(), &py, &["accept", &invite.code, "--wait", "1"]);
    assert_eq!(bob.poll().await.unwrap().last().unwrap()["type"], "paired");
    python(tmp.path(), &py, &["poll"]);

    // Messages both ways.
    alice.send("pyagent", "rust to python").await.unwrap();
    python(tmp.path(), &py, &["poll"]);
    let unread: Value = serde_json::from_str(&python(tmp.path(), &py, &["inbox", "--json"])).unwrap();
    assert_eq!(texts(unread.as_array().unwrap()), ["rust to python"]);
    python(tmp.path(), &py, &["send", "alice", "python to rust"]);
    assert_eq!(texts(&alice.poll().await.unwrap()), ["python to rust"]);
}

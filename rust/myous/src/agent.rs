//! The library API: everything an agent does with myous, without assuming
//! how it runs (VM, container, serverless) or when it wakes up.
//!
//! ```no_run
//! # async fn demo() -> anyhow::Result<()> {
//! use std::sync::Arc;
//! use myous::{Agent, FileStorage};
//! let agent = Agent::new(Arc::new(FileStorage::new(None)?), None)?;
//! agent.create_identity()?;                 // once, ever
//! agent.register(Some("Sam's Agent")).await?;
//! let invite = agent.invite().await?;       // share invite.link / invite.code
//! agent.poll().await?;                      // pairings + new messages
//! agent.send("Alex's Agent", "hi").await?;
//! agent.unread(true)?;
//! # Ok(()) }
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use futures::StreamExt;
use nostr_sdk::prelude::{Keys, PublicKey, ToBech32};
use serde_json::{json, Value};

use crate::contacts::{self, Contact, Contacts};
use crate::hub::Hub;
use crate::pairing::{Invite, Outcome, Pairing, Pending};
use crate::relay::Connection;
use crate::storage::Storage;

/// This client's release.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
use crate::{inbox, now};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct IdentityError(pub String);

const LOST: &str = "the private key is missing but contacts exist, so the identity was lost. \
    Do NOT create a new key: tell your owner. Restore the key from wherever it was kept, or, \
    if it is truly gone, the owner must clear the contacts and pair with everyone again.";

pub struct Agent {
    pub storage: Arc<dyn Storage>,
    pub hub: Hub,
    keys: Mutex<Option<Keys>>,
}

impl Agent {
    pub fn new(storage: Arc<dyn Storage>, hub_url: Option<&str>) -> Result<Self> {
        if let Some(url) = hub_url {
            let mut settings = storage.get("settings")?.unwrap_or(json!({}));
            settings["hub"] = json!(url.trim_end_matches('/'));
            storage.put("settings", &settings)?;
        }
        let hub = Hub::new(storage.clone())?;
        Ok(Self { storage, hub, keys: Mutex::new(None) })
    }

    // --- identity ----------------------------------------------------------

    pub fn keys(&self) -> Result<Keys> {
        let mut cached = self.keys.lock().unwrap();
        if let Some(k) = cached.as_ref() {
            return Ok(k.clone());
        }
        let Some(nsec) = self.storage.load_key()? else {
            let lost = contacts::load(&*self.storage)?.len() > 0;
            return Err(IdentityError(if lost { LOST.into() } else { "no identity yet; create one first".into() }).into());
        };
        let keys = Keys::parse(&nsec)?;
        *cached = Some(keys.clone());
        Ok(keys)
    }

    pub fn has_identity(&self) -> Result<bool> {
        Ok(self.storage.load_key()?.is_some())
    }

    /// Generate the agent's key. Refuses if one exists or if it looks like
    /// a previous key was lost.
    pub fn create_identity(&self) -> Result<Keys> {
        if self.storage.load_key()?.is_some() {
            return Err(IdentityError("a key already exists; refusing to replace the agent's identity".into()).into());
        }
        if !contacts::load(&*self.storage)?.is_empty() {
            return Err(IdentityError(LOST.into()).into());
        }
        let keys = Keys::generate();
        self.storage.save_key(&keys.secret_key().to_bech32()?)?;
        *self.keys.lock().unwrap() = Some(keys.clone());
        Ok(keys)
    }

    pub fn alias(&self) -> Result<String> {
        let settings = self.storage.get("settings")?.unwrap_or(json!({}));
        Ok(settings["alias"].as_str().unwrap_or("agent").to_string())
    }

    /// Publish profile and inbox relays. The first time, with proof of
    /// work, this registers the key with the hub. Safe to repeat.
    pub async fn register(&self, alias: Option<&str>) -> Result<()> {
        if let Some(alias) = alias {
            let mut settings = self.storage.get("settings")?.unwrap_or(json!({}));
            settings["alias"] = json!(alias);
            self.storage.put("settings", &settings)?;
        }
        let cfg = self.hub.config(true).await?;
        let alias = self.alias()?;
        let conn = Connection::open(&self.keys()?, &cfg.relays).await?;
        let result = async {
            let difficulty = if self.is_registered()? { 0 } else { cfg.pow_difficulty };
            if let Err(e) = conn.register(&alias, difficulty).await {
                if !e.to_string().contains("pow:") {
                    return Err(e);
                }
                // The hub forgot us (e.g. rebuilt): register again with proof of work.
                conn.register(&alias, cfg.pow_difficulty).await?;
            }
            conn.publish_inbox_relays(&cfg.relays).await
        }.await;
        conn.close().await;
        result?;
        let _lock = self.storage.lock("state", true)?;
        let mut state = inbox::state(&*self.storage)?;
        state.insert("registered".into(), json!(true));
        inbox::save_state(&*self.storage, state)
    }

    pub fn is_registered(&self) -> Result<bool> {
        Ok(inbox::state(&*self.storage)?.get("registered").and_then(Value::as_bool).unwrap_or(false))
    }

    // --- pairing -----------------------------------------------------------

    /// Start a pairing. It finishes during a later poll(), listen() or
    /// advance_pairings().
    pub async fn invite(&self) -> Result<Invite> {
        let keys = self.keys()?;
        Pairing::new(self.storage.clone(), &self.hub, &keys, self.alias()?).invite().await
    }

    /// Join a pairing. Returns Waiting if the other side didn't answer
    /// within `wait`; a later poll finishes it.
    pub async fn accept(&self, code_or_link: &str, wait: Duration) -> Result<Outcome> {
        let keys = self.keys()?;
        Pairing::new(self.storage.clone(), &self.hub, &keys, self.alias()?).accept(code_or_link, wait).await
    }

    /// Advance one pending pairing, waiting up to `wait` for the peer.
    pub async fn advance_pairing(&self, nameplate: &str, wait: Duration) -> Result<Outcome> {
        let keys = self.keys()?;
        Pairing::new(self.storage.clone(), &self.hub, &keys, self.alias()?).advance(nameplate, wait, true).await
    }

    pub async fn advance_pairings(&self) -> Result<Vec<Outcome>> {
        let keys = self.keys()?;
        Pairing::new(self.storage.clone(), &self.hub, &keys, self.alias()?).advance_all().await
    }

    pub fn pending_pairings(&self) -> Result<Vec<Pending>> {
        let keys = self.keys()?;
        Pairing::new(self.storage.clone(), &self.hub, &keys, self.alias()?).pending()
    }

    // --- messages ----------------------------------------------------------

    pub async fn send(&self, name: &str, text: &str) -> Result<Value> {
        let (pubkey, contact) = contacts::find(&*self.storage, name)?;
        if contact.status != contacts::APPROVED {
            bail!("{} is {}", contact.alias, contact.status);
        }
        let conn = self.connect().await?;
        let sent = conn.send_message(PublicKey::from_hex(&pubkey)?, text).await;
        conn.close().await;
        sent?;
        let _lock = self.storage.lock("state", true)?;
        inbox::record(&*self.storage, json!({
            "type": "message", "direction": "out", "peer": contact.npub, "alias": contact.alias, "text": text,
        }))
    }

    /// Pass on what the hub announces, once each: a newer client release (an
    /// "update" entry) and notices (a "notice" entry each). Both are
    /// information only; what to do about them is up to the agent.
    pub async fn check_notices(&self) {
        let Ok(cfg) = self.hub.config(false).await else { return };
        let latest = cfg.latest_release.filter(|l| newer(l, VERSION));
        let notices: Vec<&Value> = cfg.notices.iter().filter(|n| notice_applies(n)).collect();
        if latest.is_none() && notices.is_empty() {
            return;
        }
        let Ok(_lock) = self.storage.lock("state", true) else { return };
        let Ok(mut state) = inbox::state(&*self.storage) else { return };
        let url = &self.hub.url;
        let mut entries = vec![];
        if let Some(latest) = latest {
            if state.get("announced_release").and_then(Value::as_str) != Some(latest.as_str()) {
                state.insert("announced_release".into(), json!(latest));
                entries.push(json!({
                    "type": "update", "version": latest,
                    "text": format!("myous {latest} is available (this client is v{VERSION}). Consider upgrading: get \
                        the release, verify its signature and build it as in {url}/skill.md. What changed: {url}/changelog.md"),
                }));
            }
        }
        let mut seen: Vec<Value> = state.get("seen_notices").and_then(Value::as_array).cloned().unwrap_or_default();
        let hub = url::Url::parse(url).ok();
        let host = hub.as_ref().map(|h| h.authority().to_string()).unwrap_or_default();
        for n in notices {
            let id = n["id"].as_str().unwrap_or_default();
            if seen.iter().any(|s| s == id) {
                continue;
            }
            seen.push(json!(id));
            let text: String = n["text"].as_str().unwrap_or_default().chars().filter(|c| !c.is_control()).take(500).collect();
            let mut entry = json!({"type": "notice", "id": id, "text": format!("Notice from {host}: {text}")});
            if let (Some(link), Some(hub)) = (n["url"].as_str(), &hub) {
                if let Ok(u) = url::Url::parse(link) {
                    if u.scheme() == hub.scheme() && u.authority() == hub.authority() {
                        entry["url"] = json!(link);
                        entry["text"] = json!(format!("Notice from {host}: {text} (more: {link})"));
                    }
                }
            }
            entries.push(entry);
        }
        if entries.is_empty() {
            return;
        }
        let keep = seen.len().saturating_sub(200);
        state.insert("seen_notices".into(), Value::Array(seen.split_off(keep)));
        if inbox::save_state(&*self.storage, state).is_err() {
            return;
        }
        for entry in entries {
            let _ = inbox::record(&*self.storage, entry);
        }
    }

    /// Advance pairings and fetch waiting messages, once. Returns new
    /// history entries (messages and pairing results).
    pub async fn poll(&self) -> Result<Vec<Value>> {
        let before = self.next_seq()?;
        self.advance_pairings().await?;
        self.check_notices().await;
        let conn = self.connect().await?;
        let wraps = conn.fetch_wraps().await;
        conn.close().await;
        let wraps = wraps?;
        {
            let _lock = self.storage.lock("state", true)?;
            inbox::handle_wraps(&*self.storage, &self.keys()?, &wraps)?;
            let mut state = inbox::state(&*self.storage)?;
            state.insert("last_poll".into(), json!(now()));
            inbox::save_state(&*self.storage, state)?;
        }
        self.entries_since(before)
    }

    /// Stay connected and handle messages as they arrive, until the
    /// connection is lost for two minutes. Calls `on_new` with new messages
    /// and pairing results, and `on_tick` every `tick`. Pending pairings are
    /// advanced every few seconds, so ones started meanwhile finish quickly.
    pub async fn listen(&self, mut on_new: impl FnMut(Vec<Value>), mut on_tick: impl FnMut(), tick: Duration) -> Result<()> {
        const STEP: Duration = Duration::from_secs(3);
        let keys = self.keys()?;
        let conn = self.connect().await?;
        let mut wraps = Box::pin(conn.stream_wraps().await?);
        let mut steps = tokio::time::interval(STEP);
        let (mut down_for, mut since_tick) = (Duration::ZERO, Duration::ZERO);
        let result = loop {
            tokio::select! {
                wrap = wraps.next() => {
                    let Some(wrap) = wrap else { break Ok(()) };
                    let stored = {
                        let _lock = self.storage.lock("state", true)?;
                        inbox::handle_wraps(&*self.storage, &keys, &[wrap])?
                    };
                    if !stored.is_empty() {
                        on_new(stored);
                    }
                }
                _ = steps.tick() => {
                    let before = self.next_seq()?;
                    self.check_notices().await;
                    if !self.pending_pairings()?.is_empty() {
                        self.advance_pairings().await?;
                    }
                    let new = self.entries_since(before)?;
                    if !new.is_empty() {
                        on_new(new);
                    }
                    down_for = if conn.connected().await { Duration::ZERO } else { down_for + STEP };
                    if down_for > Duration::from_secs(120) {
                        break Err(anyhow::anyhow!("relay connection lost"));
                    }
                    since_tick += STEP;
                    if since_tick >= tick {
                        since_tick = Duration::ZERO;
                        on_tick();
                    }
                }
            }
        };
        drop(wraps);
        conn.close().await;
        result
    }

    pub fn unread(&self, mark_read: bool) -> Result<Vec<Value>> {
        inbox::unread(&*self.storage, mark_read)
    }

    pub fn history(&self, contact: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        let mut entries = self.storage.read_history()?;
        if let Some(name) = contact {
            let (_, c) = contacts::find(&*self.storage, name)?;
            entries.retain(|e| e["peer"] == c.npub.as_str());
        }
        let skip = entries.len().saturating_sub(limit);
        Ok(entries.split_off(skip))
    }

    // --- contacts ----------------------------------------------------------

    pub fn contacts(&self) -> Result<Contacts> {
        contacts::load(&*self.storage)
    }

    pub fn block(&self, name: &str) -> Result<Contact> {
        let _lock = self.storage.lock("state", true)?;
        contacts::set_status(&*self.storage, name, contacts::BLOCKED)
    }

    pub fn unblock(&self, name: &str) -> Result<Contact> {
        let _lock = self.storage.lock("state", true)?;
        contacts::set_status(&*self.storage, name, contacts::APPROVED)
    }

    pub fn rename(&self, name: &str, new_alias: &str) -> Result<Contact> {
        let _lock = self.storage.lock("state", true)?;
        contacts::rename(&*self.storage, name, new_alias)
    }

    // --- internals ---------------------------------------------------------

    async fn connect(&self) -> Result<Connection> {
        let cfg = self.hub.config(false).await?;
        Connection::open(&self.keys()?, &cfg.relays).await
    }

    fn next_seq(&self) -> Result<u64> {
        Ok(inbox::state(&*self.storage)?.get("next_seq").and_then(Value::as_u64).unwrap_or(1))
    }

    fn entries_since(&self, seq: u64) -> Result<Vec<Value>> {
        if self.next_seq()? == seq {
            return Ok(vec![]);
        }
        Ok(self.storage.read_history()?.into_iter()
            .filter(|e| e["seq"].as_u64().unwrap_or(0) >= seq && e["direction"] != "out")
            .collect())
    }
}

/// Whether release tag `a` ("v1.2.3") is newer than version `b` ("1.2.0").
fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> {
        let parts: Vec<u64> = v.trim().trim_start_matches('v').split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        (parts.len() == 3).then_some(parts)
    };
    matches!((parse(a), parse(b)), (Some(x), Some(y)) if x > y)
}

/// Whether a notice from the hub is well-formed, current, and meant for this
/// client's version.
fn notice_applies(n: &Value) -> bool {
    let has_text = n["text"].as_str().is_some_and(|t| !t.trim().is_empty());
    if n["id"].as_str().is_none_or(str::is_empty) || !has_text {
        return false;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if n["expires"].as_u64().is_some_and(|e| e <= now) {
        return false;
    }
    if n["min_version"].as_str().is_some_and(|v| newer(v, VERSION)) {
        return false;
    }
    if n["max_version"].as_str().is_some_and(|v| newer(VERSION, v)) {
        return false;
    }
    true
}

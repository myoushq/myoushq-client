//! Message history and processing of incoming gift wraps.
//!
//! History records, oldest first:
//!   {"seq", "type": "message", "direction": "in"|"out", "peer", "alias", "text", "at", "sent_at"?}
//!   {"seq", "type": "file", "direction": "in"|"out", "peer", "alias", "name", "mime", "size",
//!        "x", "ox", "url", "key", "nonce", "w"?, "at", "sent_at"?}   (protocol section 6)
//!   {"seq", "type": "result"|"ack", "direction": "in", "peer", "alias", "id", "text", ...fields, "at"}
//!        (worker replies, protocol section 7; consumed by the request that waits for them)
//!   {"seq", "type": "paired"|"pairing_failed", "peer"?, "alias"?, "text", "at"}
//!   {"seq", "type": "card", "direction": "in"|"out", "peer", "alias", "name", "about", "text", "at", "sent_at"?}
//!        (a contact's card, what it says about itself, or ours sent to it; protocol section 4)
//! The "state" document keeps the last seq read and the gift wraps already
//! handled. Callers hold the storage lock around `record` and `handle_wraps`.

use anyhow::Result;
use nostr_sdk::prelude::{Event, Keys};
use serde_json::{json, Map, Value};

use crate::storage::Storage;
use crate::{contacts, now, parts, relay};

/// Longer than relay retention plus timestamp jitter.
const SEEN_RETENTION: u64 = 3 * 86400;

pub fn state(st: &dyn Storage) -> Result<Map<String, Value>> {
    Ok(match st.get("state")? {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    })
}

pub fn save_state(st: &dyn Storage, state: Map<String, Value>) -> Result<()> {
    st.put("state", &Value::Object(state))
}

pub fn record(st: &dyn Storage, entry: Value) -> Result<Value> {
    let mut s = state(st)?;
    let seq = s.get("next_seq").and_then(Value::as_u64).unwrap_or(1);
    let mut entry = entry;
    entry["seq"] = json!(seq);
    entry["at"] = json!(now());
    st.append_history(&entry)?;
    s.insert("next_seq".into(), json!(seq + 1));
    save_state(st, s)?;
    Ok(entry)
}

/// Store messages from approved contacts, drop everything else.
pub fn handle_wraps(st: &dyn Storage, keys: &Keys, wraps: &[Event]) -> Result<Vec<Value>> {
    let mut s = state(st)?;
    let mut seen = s.get("seen").and_then(Value::as_object).cloned().unwrap_or_default();
    let now = now();
    let mut fresh = vec![];
    for wrap in wraps {
        let id = wrap.id.to_hex();
        if seen.contains_key(&id) {
            continue;
        }
        seen.insert(id, json!(now));
        fresh.push(wrap);
    }
    seen.retain(|_, t| now - t.as_u64().unwrap_or(0) < SEEN_RETENTION);
    s.insert("seen".into(), Value::Object(seen));
    save_state(st, s)?;

    // Wrap timestamps are randomized, so put messages in the order they were written.
    let mut messages: Vec<relay::Unwrapped> = fresh.iter().filter_map(|w| relay::unwrap(keys, w)).collect();
    messages.sort_by_key(|m| (m.sent_at, m.ms, m.part.as_ref().map(|p| p.index).unwrap_or(0)));
    let mut buf: parts::Buffer = match st.get("partials")? {
        Some(v) => serde_json::from_value(v).unwrap_or_default(),
        None => parts::Buffer::new(),
    };
    let blob_api = st.get("hub")?.map(|c| crate::hub::blob_api(&c, c["url"].as_str().unwrap_or_default())).unwrap_or_default();
    let mut changed = false;
    let mut stored = vec![];
    for m in messages {
        let sender = m.sender.to_hex();
        // Not paired, or blocked: drop silently.
        let Some(contact) = contacts::approved(st, &sender)? else { continue };
        if m.kind == crate::files::KIND_FILE {
            if let Some(entry) = file_entry(&m, &blob_api) {
                stored.push(record(st, json!({
                    "type": "file", "direction": "in", "peer": contact.npub, "alias": contact.alias,
                    "sent_at": m.sent_at, "name": entry.name, "mime": entry.mime, "size": entry.size,
                    "x": entry.x, "ox": entry.ox, "url": entry.url, "key": entry.key, "nonce": entry.nonce, "w": entry.w,
                }))?);
            }
            continue;
        }
        if let Some((kind, fields)) = worker_reply(&m.text) {
            let mut entry = json!({
                "type": kind, "direction": "in", "peer": contact.npub, "alias": contact.alias,
                "sent_at": m.sent_at, "text": m.text,
            });
            for (k, v) in fields {
                if k != "myous" {
                    entry[k] = v;
                }
            }
            stored.push(record(st, entry)?);
            continue;
        }
        let (text, sent_at) = match &m.part {
            Some(part) => {
                changed = true;
                match parts::add(&mut buf, &sender, part, &m.text, m.sent_at, m.ms, now) {
                    Some((text, sent_at, _)) => (text, sent_at),
                    None => continue, // waiting for the other parts
                }
            }
            None => (m.text, m.sent_at),
        };
        // A card is stored on the contact and noted; anything else is a message.
        if let Some((name, about)) = contacts::parse_card(&text) {
            let (_, line) = contacts::receive_card(st, &sender, &name, &about, now)?;
            stored.push(record(st, json!({
                "type": "card", "direction": "in", "peer": contact.npub, "alias": contact.alias,
                "name": name, "about": about, "text": line, "sent_at": sent_at,
            }))?);
            continue;
        }
        stored.push(record(st, json!({
            "type": "message", "direction": "in", "peer": contact.npub,
            "alias": contact.alias, "text": text, "sent_at": sent_at,
        }))?);
    }
    for (sender, text, sent_at) in parts::expire(&mut buf, now) {
        changed = true;
        if let Some(contact) = contacts::approved(st, &sender)? {
            stored.push(record(st, json!({
                "type": "message", "direction": "in", "peer": contact.npub,
                "alias": contact.alias, "text": text, "sent_at": sent_at, "incomplete": true,
            }))?);
        }
    }
    if changed {
        st.put("partials", &serde_json::to_value(&buf)?)?;
    }
    Ok(stored)
}

/// Whether a history entry is a worker reply, consumed by the request
/// that waited for it rather than shown as new mail.
pub fn is_reply(e: &Value) -> bool {
    e["type"] == "result" || e["type"] == "ack" || (e["type"] == "file" && e["w"][0] == "file")
}

pub fn unread(st: &dyn Storage, mark_read: bool) -> Result<Vec<Value>> {
    let _lock = st.lock("state", true)?;
    let mut s = state(st)?;
    let last = s.get("read_seq").and_then(Value::as_u64).unwrap_or(0);
    let mut entries: Vec<Value> = st.read_history()?.into_iter()
        .filter(|e| e["seq"].as_u64().unwrap_or(0) > last && e["direction"] != "out" && !is_reply(e))
        .collect();
    // The contact's current relationship context and what it says of itself,
    // so the agent has them when it answers.
    for e in entries.iter_mut() {
        if let Some(peer) = e["peer"].as_str().map(String::from) {
            let (relationship, sharing, about) = contacts::context_of(st, &peer);
            e["relationship"] = json!(relationship);
            e["sharing"] = json!(sharing);
            e["about"] = json!(about);
        }
    }
    if mark_read {
        if let Some(last) = entries.last() {
            s.insert("read_seq".into(), last["seq"].clone());
            save_state(st, s)?;
        }
    }
    Ok(entries)
}

/// The fields of a received file message, checked.
pub struct FileEntry {
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub x: String,
    pub ox: String,
    pub url: String,
    pub key: String,
    pub nonce: String,
    /// The `w` tag's values after the name, if any (protocol section 7).
    pub w: Option<Vec<String>>,
}

/// Validate a kind-15 message: the tags of protocol 6.3, and a URL under
/// the hub's own blob API. Anything else is dropped.
fn file_entry(m: &relay::Unwrapped, blob_api: &str) -> Option<FileEntry> {
    let tag = |name: &str| m.tags.iter().find(|t| t.first().map(String::as_str) == Some(name)).and_then(|t| t.get(1)).cloned();
    if tag("encryption-algorithm")?.as_str() != "aes-gcm" {
        return None;
    }
    let (key, nonce, x, ox) = (tag("decryption-key")?, tag("decryption-nonce")?, tag("x")?, tag("ox")?);
    let is_hex = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit());
    if !is_hex(&key, 64) || !is_hex(&nonce, 24) || !is_hex(&x, 64) || !is_hex(&ox, 64) {
        return None;
    }
    let url = m.text.trim().to_string();
    if blob_api.is_empty() || !url.starts_with(&format!("{blob_api}/")) || url != format!("{blob_api}/{x}") {
        return None;
    }
    let name = crate::files::sanitize_name(&tag("name").unwrap_or_default()).unwrap_or_else(|| format!("file-{}", &x[..8]));
    let w = m.tags.iter().find(|t| t.first().map(String::as_str) == Some("w")).map(|t| t[1..].to_vec());
    Some(FileEntry {
        name, mime: tag("file-type").unwrap_or_else(|| "application/octet-stream".into()),
        size: tag("size").and_then(|s| s.parse().ok()).unwrap_or(0), x, ox, url, key, nonce, w,
    })
}

/// A kind-14 text that is a worker reply ({"myous": "result"|"ack", ...}).
fn worker_reply(text: &str) -> Option<(String, Map<String, Value>)> {
    let v: Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object()?;
    let kind = obj.get("myous")?.as_str()?;
    if kind != "result" && kind != "ack" {
        return None;
    }
    obj.get("id")?.as_str()?;
    Some((kind.to_string(), obj.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileStorage;
    use nostr::nips::nip59::GiftWrapBuilder;
    use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, FinalizeUnsignedEvent, Kind, Tag};

    /// A kind-14 gift wrap from `sender` to `me`, as the relay would deliver it.
    fn wrap(sender: &Keys, me: &Keys, text: &str) -> Event {
        let rumor = EventBuilder::new(Kind::PrivateDirectMessage, text)
            .tags([Tag::public_key(me.public_key())])
            .finalize_unsigned(sender.public_key());
        GiftWrapBuilder::new(me.public_key(), rumor).finalize(sender).unwrap()
    }

    #[test]
    fn cards() {
        let dir = tempfile::tempdir().unwrap();
        let st = FileStorage::new(Some(dir.path().to_path_buf())).unwrap();
        let (me, peer) = (Keys::generate(), Keys::generate());
        let peer_hex = peer.public_key().to_hex();
        contacts::add(&st, &peer_hex, "peer").unwrap();
        let handle = |text: &str| -> Value {
            let mut got = handle_wraps(&st, &me, &[wrap(&peer, &me, text)]).unwrap();
            assert_eq!(got.len(), 1, "{text}");
            got.remove(0)
        };
        let card = |v: Value| v.to_string();

        let e = handle(&card(json!({"myous": "card", "name": "peer", "about": "Sam's own Mac; the 'myous browser' is its browser"})));
        assert_eq!(e["type"], "card");
        assert_eq!(e["direction"], "in");
        assert_eq!(e["about"], "Sam's own Mac; the 'myous browser' is its browser");
        assert_eq!(e["text"], "peer describes itself: Sam's own Mac; the 'myous browser' is its browser");
        assert!(e["sent_at"].as_u64().is_some() && e["seq"].as_u64().is_some());
        assert_eq!(contacts::load(&st).unwrap()[&peer_hex].card.as_ref().unwrap().about, "Sam's own Mac; the 'myous browser' is its browser");
        // A rename is announced, never applied: the alias is ours.
        let renamed = card(json!({"myous": "card", "name": "Sam's Mac", "about": "Sam's own Mac; the 'myous browser' is its browser"}));
        let e = handle(&renamed);
        assert_eq!(e["text"], "peer now calls itself \"Sam's Mac\"; you call it \"peer\" \
                               (keep that, or follow it: myous rename \"peer\" \"Sam's Mac\")");
        let c = &contacts::load(&st).unwrap()[&peer_hex];
        assert_eq!((c.alias.as_str(), c.card.as_ref().unwrap().name.as_str()), ("peer", "Sam's Mac"));
        // The same card again: noted as unchanged. A cleared description is noted too.
        assert_eq!(handle(&renamed)["text"], "peer sent its card again, unchanged");
        let e = handle(&card(json!({"myous": "card", "name": "Sam's Mac", "about": ""})));
        assert_eq!(e["text"], "peer cleared its description");
        // Incoming messages carry the contact's description, like the owner's context.
        handle(&card(json!({"myous": "card", "name": "Sam's Mac", "about": "a Mac"})));
        handle("hello");
        let shown = unread(&st, true).unwrap();
        assert_eq!(shown.last().unwrap()["text"], "hello");
        assert_eq!(shown.last().unwrap()["about"], "a Mac");
        assert_eq!(shown.iter().filter(|e| e["type"] == "card").count(), 5, "cards show as new items");
        // Malformed cards are plain messages: no name, a long name, a long about, wrong types.
        for bad in [
            json!({"myous": "card", "about": "x"}),
            json!({"myous": "card", "name": "n".repeat(65), "about": ""}),
            json!({"myous": "card", "name": "n", "about": "a".repeat(501)}),
            json!({"myous": "card", "name": 3, "about": ""}),
        ] {
            let e = handle(&card(bad.clone()));
            assert_eq!(e["type"], "message", "{bad}");
            assert_eq!(e["text"], bad.to_string());
        }
        assert_eq!(contacts::load(&st).unwrap()[&peer_hex].card.as_ref().unwrap().about, "a Mac");
    }
}

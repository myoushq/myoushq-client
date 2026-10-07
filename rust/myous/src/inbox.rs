//! Message history and processing of incoming gift wraps.
//!
//! History records, oldest first:
//!   {"seq", "type": "message", "direction": "in"|"out", "peer", "alias", "text", "at", "sent_at"?}
//!   {"seq", "type": "paired"|"pairing_failed", "peer"?, "alias"?, "text", "at"}
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
    let mut changed = false;
    let mut stored = vec![];
    for m in messages {
        let sender = m.sender.to_hex();
        // Not paired, or blocked: drop silently.
        let Some(contact) = contacts::approved(st, &sender)? else { continue };
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

pub fn unread(st: &dyn Storage, mark_read: bool) -> Result<Vec<Value>> {
    let _lock = st.lock("state", true)?;
    let mut s = state(st)?;
    let last = s.get("read_seq").and_then(Value::as_u64).unwrap_or(0);
    let entries: Vec<Value> = st.read_history()?.into_iter()
        .filter(|e| e["seq"].as_u64().unwrap_or(0) > last && e["direction"] != "out")
        .collect();
    if mark_read {
        if let Some(last) = entries.last() {
            s.insert("read_seq".into(), last["seq"].clone());
            save_state(st, s)?;
        }
    }
    Ok(entries)
}

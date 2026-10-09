//! Paired peers. Only `approved` contacts can reach this agent.
//!
//! Statuses: approved, blocked. `pending` is reserved for future contact
//! requests that the owner approves. Callers hold the storage lock around
//! changes.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Result};
use nostr_sdk::prelude::{PublicKey, ToBech32};
use serde::{Deserialize, Serialize};

use crate::storage::Storage;
use crate::now;

pub const APPROVED: &str = "approved";
pub const BLOCKED: &str = "blocked";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    pub alias: String,
    pub npub: String,
    pub status: String,
    pub paired_at: u64,
    /// How the owner knows this contact (one of RELATIONSHIPS), kept only here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<String>,
    /// The owner's guidance on what may be shared with this contact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<String>,
    /// Who made this pairing on the agent's behalf ("owner": through the desktop app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
}

pub const RELATIONSHIPS: [&str; 6] = ["family", "friend", "colleague", "business", "service", "other"];
const MAX_SHARING: usize = 500;

/// Relationship context to record on a contact; `None` fields are left as they are.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ContactContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<String>,
}

impl ContactContext {
    pub fn validate(&self) -> Result<()> {
        if let Some(r) = &self.relationship {
            if !RELATIONSHIPS.contains(&r.as_str()) {
                bail!("relationship must be one of: {}", RELATIONSHIPS.join(", "));
            }
        }
        if self.sharing.as_ref().is_some_and(|s| s.chars().count() > MAX_SHARING) {
            bail!("sharing guidance is limited to {MAX_SHARING} characters");
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.relationship.is_none() && self.sharing.is_none() && self.added_by.is_none()
    }
}

/// Record relationship context on a contact.
pub fn set_context(st: &dyn Storage, name: &str, cc: &ContactContext) -> Result<Contact> {
    cc.validate()?;
    let (key, _) = find(st, name)?;
    let mut contacts = load(st)?;
    let c = contacts.get_mut(&key).unwrap();
    if let Some(r) = &cc.relationship {
        c.relationship = Some(r.clone());
    }
    if let Some(s) = cc.sharing.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        c.sharing = Some(s.to_string());
    }
    if let Some(a) = cc.added_by.as_ref().filter(|a| !a.is_empty()) {
        c.added_by = Some(a.clone());
    }
    save(st, &contacts)?;
    Ok(contacts[&key].clone())
}

/// (relationship, sharing) of the contact with this npub.
pub fn context_of(st: &dyn Storage, npub: &str) -> (Option<String>, Option<String>) {
    load(st).ok()
        .and_then(|cs| cs.into_values().find(|c| c.npub == npub))
        .map(|c| (c.relationship, c.sharing))
        .unwrap_or((None, None))
}

/// Contacts keyed by hex public key.
pub type Contacts = BTreeMap<String, Contact>;

pub fn load(st: &dyn Storage) -> Result<Contacts> {
    Ok(match st.get("contacts")? {
        Some(v) => serde_json::from_value(v)?,
        None => Contacts::new(),
    })
}

fn save(st: &dyn Storage, contacts: &Contacts) -> Result<()> {
    st.put("contacts", &serde_json::to_value(contacts)?)
}

/// Pin a peer as approved. Re-pairing with a known key keeps its alias.
pub fn add(st: &dyn Storage, pubkey_hex: &str, alias: &str) -> Result<Contact> {
    let mut contacts = load(st)?;
    if let Some(c) = contacts.get_mut(pubkey_hex) {
        c.status = APPROVED.into();
    } else {
        let alias = unique_alias(&contacts, if alias.is_empty() { "peer" } else { alias });
        let npub = PublicKey::from_hex(pubkey_hex)?.to_bech32()?;
        contacts.insert(pubkey_hex.into(), Contact {
            alias, npub, status: APPROVED.into(), paired_at: now(), relationship: None, sharing: None, added_by: None,
        });
    }
    save(st, &contacts)?;
    Ok(contacts[pubkey_hex].clone())
}

/// Look up a contact by alias, npub or hex key.
pub fn find(st: &dyn Storage, name: &str) -> Result<(String, Contact)> {
    let contacts = load(st)?;
    contacts.iter()
        .find(|(k, c)| name == c.alias || name == c.npub || name == k.as_str())
        .or_else(|| contacts.iter().find(|(_, c)| c.alias.to_lowercase() == name.to_lowercase()))
        .map(|(k, c)| (k.clone(), c.clone()))
        .ok_or_else(|| anyhow!("no contact named {name:?}"))
}

pub fn set_status(st: &dyn Storage, name: &str, status: &str) -> Result<Contact> {
    let (key, _) = find(st, name)?;
    let mut contacts = load(st)?;
    contacts.get_mut(&key).unwrap().status = status.into();
    save(st, &contacts)?;
    Ok(contacts[&key].clone())
}

pub fn rename(st: &dyn Storage, name: &str, new_alias: &str) -> Result<Contact> {
    let (key, _) = find(st, name)?;
    let mut contacts = load(st)?;
    if contacts.iter().any(|(k, c)| k != &key && c.alias == new_alias) {
        bail!("alias {new_alias:?} is already used");
    }
    contacts.get_mut(&key).unwrap().alias = new_alias.into();
    save(st, &contacts)?;
    Ok(contacts[&key].clone())
}

pub fn approved(st: &dyn Storage, pubkey_hex: &str) -> Result<Option<Contact>> {
    Ok(load(st)?.remove(pubkey_hex).filter(|c| c.status == APPROVED))
}

fn unique_alias(contacts: &Contacts, alias: &str) -> String {
    let taken = |a: &str| contacts.values().any(|c| c.alias == a);
    let mut candidate = alias.to_string();
    let mut n = 2;
    while taken(&candidate) {
        candidate = format!("{alias}-{n}");
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::FileStorage;

    const HEX: &str = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";

    #[test]
    fn added_by_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let st = FileStorage::new(Some(dir.path().to_path_buf())).unwrap();
        let c = add(&st, HEX, "peer").unwrap();
        assert_eq!(c.added_by, None);
        assert!(!serde_json::to_string(&c).unwrap().contains("added_by"), "unset fields are omitted");

        let cc: ContactContext = serde_json::from_value(serde_json::json!({"added_by": "owner"})).unwrap();
        assert!(!cc.is_empty());
        let c = set_context(&st, "peer", &cc).unwrap();
        assert_eq!(c.added_by.as_deref(), Some("owner"));
        assert_eq!(load(&st).unwrap()[HEX].added_by.as_deref(), Some("owner"), "stored");

        // Other context leaves it alone.
        let cc = ContactContext { relationship: Some("friend".into()), ..Default::default() };
        let c = set_context(&st, "peer", &cc).unwrap();
        assert_eq!((c.relationship.as_deref(), c.added_by.as_deref()), (Some("friend"), Some("owner")));
    }
}

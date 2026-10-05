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
        contacts.insert(pubkey_hex.into(), Contact { alias, npub, status: APPROVED.into(), paired_at: now() });
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

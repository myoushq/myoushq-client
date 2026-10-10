//! Paired peers. Only `approved` contacts can reach this agent.
//!
//! Statuses: approved, blocked. `pending` is reserved for future contact
//! requests that the owner approves.
//!
//! Cards (protocol.md, section 4): `card` is the latest {"name", "about",
//! "at"} the contact sent about itself; `peer_knows` is {"name", "about"} as
//! this agent last told the contact (the alias at pairing, then each card
//! sent), so a change is sent once. The contact's `alias` is this agent's own
//! label for it and never follows a card.
//!
//! Callers hold the storage lock around changes.

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
    /// The latest card the contact sent about itself (protocol section 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
    /// What this agent last told the contact about itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_knows: Option<PeerKnows>,
}

/// What a contact says about itself: its own alias and self-description.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub name: String,
    pub about: String,
    /// When it was received.
    pub at: u64,
}

/// This agent's alias and self-description as a contact last heard them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerKnows {
    pub name: String,
    pub about: String,
}

pub const RELATIONSHIPS: [&str; 6] = ["family", "friend", "colleague", "business", "service", "other"];
const MAX_SHARING: usize = 500;
/// An alias, as in the pairing payload.
pub const MAX_NAME: usize = 64;
/// A card's self-description.
pub const MAX_ABOUT: usize = 500;

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

/// (relationship, sharing, about) of the contact with this npub: the
/// owner's context, and what the contact says about itself (its card).
pub fn context_of(st: &dyn Storage, npub: &str) -> (Option<String>, Option<String>, Option<String>) {
    load(st).ok()
        .and_then(|cs| cs.into_values().find(|c| c.npub == npub))
        .map(|c| (c.relationship, c.sharing, c.card.map(|k| k.about).filter(|a| !a.is_empty())))
        .unwrap_or((None, None, None))
}

/// The (name, about) of a card message (protocol.md, section 4), or None
/// when the text is not a valid card: fields missing, of the wrong type or
/// too long. Such a text stays a plain message.
pub fn parse_card(text: &str) -> Option<(String, String)> {
    if !text.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object()?;
    if obj.get("myous")?.as_str()? != "card" {
        return None;
    }
    let name = obj.get("name")?.as_str()?.trim();
    let about = match obj.get("about") {
        None => "",
        Some(a) => a.as_str()?.trim(),
    };
    if !(1..=MAX_NAME).contains(&name.chars().count()) || about.chars().count() > MAX_ABOUT {
        return None;
    }
    Some((name.to_string(), about.to_string()))
}

/// The JSON of this agent's card.
pub fn card_text(name: &str, about: &str) -> String {
    serde_json::json!({"myous": "card", "name": name, "about": about}).to_string()
}

/// Store a contact's card; returns the contact and a line for the history
/// saying what changed: a new or changed description, a new name
/// (announced, never applied: the alias is ours), or both.
pub fn receive_card(st: &dyn Storage, pubkey_hex: &str, name: &str, about: &str, at: u64) -> Result<(Contact, String)> {
    let mut contacts = load(st)?;
    let c = contacts.get_mut(pubkey_hex).ok_or_else(|| anyhow!("no contact with key {pubkey_hex}"))?;
    let old = c.card.take();
    let known_name = old.as_ref().map(|k| k.name.as_str()).unwrap_or(&c.alias);
    let mut bits = vec![];
    if name != known_name {
        bits.push(format!("now calls itself \"{name}\"; you call it \"{0}\" (keep that, or follow it: myous rename \"{0}\" \"{name}\")",
            c.alias));
    }
    if about != old.as_ref().map(|k| k.about.as_str()).unwrap_or("") {
        bits.push(if about.is_empty() { "cleared its description".to_string() } else { format!("describes itself: {about}") });
    }
    if bits.is_empty() {
        bits.push("sent its card again, unchanged".to_string());
    }
    c.card = Some(Card { name: name.to_string(), about: about.to_string(), at });
    let line = format!("{} {}", c.alias, bits.join("; "));
    save(st, &contacts)?;
    Ok((contacts[pubkey_hex].clone(), line))
}

/// Record what this agent has told a contact about itself.
pub fn peer_knows(st: &dyn Storage, pubkey_hex: &str, name: &str, about: &str) -> Result<()> {
    let mut contacts = load(st)?;
    if let Some(c) = contacts.get_mut(pubkey_hex) {
        c.peer_knows = Some(PeerKnows { name: name.to_string(), about: about.to_string() });
        save(st, &contacts)?;
    }
    Ok(())
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
            card: None, peer_knows: None,
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
    use serde_json::json;

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

    #[test]
    fn cards_are_checked() {
        let card = |v: serde_json::Value| parse_card(&v.to_string());
        assert_eq!(card(json!({"myous": "card", "name": " Sam's Mac ", "about": " a Mac "})),
                   Some(("Sam's Mac".into(), "a Mac".into())), "trimmed");
        assert_eq!(card(json!({"myous": "card", "name": "n"})), Some(("n".into(), String::new())), "about may be absent");
        for bad in [
            json!({"myous": "card", "about": "x"}),
            json!({"myous": "card", "name": "n".repeat(65), "about": ""}),
            json!({"myous": "card", "name": "n", "about": "a".repeat(501)}),
            json!({"myous": "card", "name": 3, "about": ""}),
            json!({"myous": "card", "name": "n", "about": 3}),
            json!({"myous": "card", "name": "  ", "about": ""}),
            json!({"myous": "exec", "name": "n", "about": ""}),
            json!(["card"]),
        ] {
            assert_eq!(card(bad.clone()), None, "{bad}");
        }
        assert_eq!(parse_card("hello"), None);
        assert_eq!(parse_card("{not json"), None);
        let text = card_text("Max's Muse", "an assistant");
        assert_eq!(parse_card(&text), Some(("Max's Muse".into(), "an assistant".into())), "round trip: {text}");
    }

    #[test]
    fn a_received_card_is_kept_and_described() {
        let dir = tempfile::tempdir().unwrap();
        let st = FileStorage::new(Some(dir.path().to_path_buf())).unwrap();
        add(&st, HEX, "peer").unwrap();
        assert!(receive_card(&st, "ab".repeat(32).as_str(), "x", "", 1).is_err(), "unknown key");

        let (c, line) = receive_card(&st, HEX, "peer", "Sam's own Mac", 1).unwrap();
        assert_eq!(line, "peer describes itself: Sam's own Mac");
        assert_eq!(c.card, Some(Card { name: "peer".into(), about: "Sam's own Mac".into(), at: 1 }));
        // A rename is announced, never applied: the alias is ours.
        let (c, line) = receive_card(&st, HEX, "Sam's Mac", "Sam's own Mac", 2).unwrap();
        assert_eq!(line, "peer now calls itself \"Sam's Mac\"; you call it \"peer\" \
                          (keep that, or follow it: myous rename \"peer\" \"Sam's Mac\")");
        assert_eq!((c.alias.as_str(), c.card.as_ref().unwrap().name.as_str()), ("peer", "Sam's Mac"));
        let (_, line) = receive_card(&st, HEX, "Sam's Mac", "Sam's own Mac", 3).unwrap();
        assert_eq!(line, "peer sent its card again, unchanged");
        let (_, line) = receive_card(&st, HEX, "Sam's Mac", "", 4).unwrap();
        assert_eq!(line, "peer cleared its description");
        let (_, line) = receive_card(&st, HEX, "Sam", "a Mac", 5).unwrap();
        assert_eq!(line, "peer now calls itself \"Sam\"; you call it \"peer\" \
                          (keep that, or follow it: myous rename \"peer\" \"Sam\"); describes itself: a Mac");
        let stored = &load(&st).unwrap()[HEX];
        assert_eq!(stored.card.as_ref().unwrap().at, 5);
        assert_eq!(context_of(&st, &stored.npub).2.as_deref(), Some("a Mac"));
        assert!(stored.peer_knows.is_none());
        assert!(!serde_json::to_string(stored).unwrap().contains("peer_knows"), "unset fields are omitted");

        peer_knows(&st, HEX, "me", "mine").unwrap();
        assert_eq!(load(&st).unwrap()[HEX].peer_knows, Some(PeerKnows { name: "me".into(), about: "mine".into() }));
        peer_knows(&st, "ab".repeat(32).as_str(), "me", "").unwrap(); // unknown key: nothing to record
    }
}

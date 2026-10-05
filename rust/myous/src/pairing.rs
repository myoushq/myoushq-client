//! Pairing two agents through the hub's mailbox (protocol.md §5).
//!
//! A pairing code looks like 4821-K7F3QX. The nameplate (4821) names a
//! mailbox on the hub; the secret (K7F3QX) never leaves the two agents. Both
//! sides run SPAKE2 with the full code as password, relayed through the
//! mailbox, and get a shared key the hub can't learn. Each side then sends
//! its identity (public key and alias) encrypted with that key. A wrong code,
//! or a hub that tampers, makes decryption fail and the pairing aborts.
//!
//! Mailbox messages (base64 of JSON):
//!   {"t": "pake", "v": 1, "m": <spake2 message>}
//!   {"t": "payload", "v": 1, "n": <nonce>, "c": <ciphertext>}

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use hkdf::Hkdf;
use myous_pake::Role;
use nostr_sdk::prelude::{Keys, PublicKey};
use rand::{Rng, RngCore};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::contacts::{self, Contact};
use crate::hub::{Hub, HubError};
use crate::storage::Storage;
use crate::{inbox, now};

pub const VERSION: u64 = 1;
pub const SECRET_LEN: usize = 6;
/// Crockford base32: no I, L, O, U, so codes survive being read aloud.
pub const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PairingError(pub String);

fn fail<T>(msg: &str) -> Result<T> {
    Err(PairingError(msg.into()).into())
}

// --- codes ---------------------------------------------------------------

pub fn make_secret() -> String {
    let alphabet = ALPHABET.as_bytes();
    let mut rng = rand::rngs::OsRng;
    (0..SECRET_LEN).map(|_| alphabet[rng.gen_range(0..alphabet.len())] as char).collect()
}

pub fn format_code(nameplate: &str, secret: &str) -> String {
    format!("{nameplate}-{secret}")
}

pub fn make_link(link_base: &str, nameplate: &str, secret: &str) -> String {
    format!("{link_base}{nameplate}#{secret}")
}

/// Accept a pairing link, or a code like "4821-K7F3QX" or "4821 k7f 3qx".
pub fn parse_code(text: &str, link_base: &str) -> Result<(String, String)> {
    let text = text.trim();
    if text.contains("://") {
        let url = url::Url::parse(text)?;
        let expected = url::Url::parse(link_base)?;
        let (got, want) = (host_port(&url), host_port(&expected));
        if got != want {
            return fail(&format!("this link is for {got}, but this agent uses {want}"));
        }
        let nameplate = url.path().strip_prefix("/p/").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        let (Some(nameplate), Some(fragment)) = (nameplate, url.fragment().filter(|f| !f.is_empty())) else {
            return fail("that doesn't look like a complete pairing link");
        };
        return Ok((nameplate.to_string(), normalize_secret(fragment)?));
    }
    let re = Regex::new(r"^(\d+)[\s-]+([0-9A-Za-z\s-]+)$").unwrap();
    let Some(caps) = re.captures(text) else { return fail("pairing codes look like 4821-K7F3QX") };
    Ok((caps[1].to_string(), normalize_secret(&caps[2])?))
}

fn host_port(url: &url::Url) -> String {
    match url.port() {
        Some(p) => format!("{}:{p}", url.host_str().unwrap_or("")),
        None => url.host_str().unwrap_or("").to_string(),
    }
}

pub fn normalize_secret(raw: &str) -> Result<String> {
    let s: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            c => c,
        })
        .collect();
    if s.chars().count() != SECRET_LEN || !s.chars().all(|c| ALPHABET.contains(c)) {
        return fail("the secret part of the code is not valid");
    }
    Ok(s)
}

// --- crypto --------------------------------------------------------------

pub fn derive(key: &[u8], info: &str, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    Hkdf::<Sha256>::new(None, key)
        .expand(format!("myous pairing v{VERSION} {info}").as_bytes(), &mut out)
        .expect("valid HKDF length");
    out
}

/// Six digits both owners can compare by eye, if they want to.
pub fn verify_code(key: &[u8]) -> String {
    let b: [u8; 8] = derive(key, "verify", 8).try_into().unwrap();
    format!("{:06}", u64::from_be_bytes(b) % 1_000_000)
}

/// The payload laid out like Python's json.dumps(sort_keys=True) (sorted
/// keys, ", " and ": " separators), so the test vectors match byte for byte.
/// Receivers just parse the JSON, so other layouts would interoperate too.
fn payload_json(payload: &Value) -> String {
    let Value::Object(map) = payload else { return payload.to_string() };
    let mut fields: Vec<(&String, &Value)> = map.iter().collect();
    fields.sort_by_key(|(k, _)| k.as_str());
    let parts: Vec<String> = fields.iter().map(|(k, v)| format!("{}: {}", Value::from(k.as_str()), v)).collect();
    format!("{{{}}}", parts.join(", "))
}

pub fn seal(key: &[u8], role: &str, nameplate: &str, payload: &Value, nonce: Option<[u8; 12]>) -> Value {
    let nonce = nonce.unwrap_or_else(|| {
        let mut n = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut n);
        n
    });
    let cipher = ChaCha20Poly1305::new_from_slice(&derive(key, &format!("from {role}"), 32)).unwrap();
    let plain = payload_json(payload);
    let sealed = cipher
        .encrypt((&nonce).into(), Payload { msg: plain.as_bytes(), aad: nameplate.as_bytes() })
        .expect("encryption");
    json!({"t": "payload", "v": VERSION, "n": B64.encode(nonce), "c": B64.encode(sealed)})
}

pub fn unseal(key: &[u8], peer_role: &str, nameplate: &str, msg: &Value) -> Result<Value> {
    let open = || -> Option<Value> {
        let nonce: [u8; 12] = B64.decode(msg["n"].as_str()?).ok()?.try_into().ok()?;
        let sealed = B64.decode(msg["c"].as_str()?).ok()?;
        let cipher = ChaCha20Poly1305::new_from_slice(&derive(key, &format!("from {peer_role}"), 32)).ok()?;
        let plain = cipher.decrypt((&nonce).into(), Payload { msg: &sealed, aad: nameplate.as_bytes() }).ok()?;
        let payload: Value = serde_json::from_slice(&plain).ok()?;
        PublicKey::from_hex(payload["pubkey"].as_str()?).ok()?;
        Some(payload)
    };
    open().ok_or_else(|| PairingError("the code didn't match (or the exchange was tampered with)".into()).into())
}

// --- the exchange --------------------------------------------------------
// A pairing spans a few round trips, so its state is saved as a
// "pending/<nameplate>" document and finished by whichever call gets there
// first: accept (which waits a while), or any later advance().

/// A pairing in progress, as stored in "pending/<nameplate>".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    pub role: String,
    pub nameplate: String,
    pub secret: String,
    pub token: String,
    pub expires_at: u64,
    /// Hex seed the SPAKE2 instance is recreated from (see myous-pake).
    pub pake_seed: String,
    /// How many of the peer's mailbox messages we've handled.
    pub after: u64,
    /// "wait_pake", then "wait_payload".
    pub stage: String,
    /// Shared key, base64, once the PAKE is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Still waiting for the other side.
    Waiting,
    Done { contact: Contact, verify: String },
    Failed(String),
    /// Another run finished or dropped it.
    Elsewhere,
}

/// A new invite, to share with the other owner.
#[derive(Debug, Clone, Serialize)]
pub struct Invite {
    pub code: String,
    pub link: String,
    pub nameplate: String,
    pub expires_at: u64,
}

pub struct Pairing<'a> {
    st: Arc<dyn Storage>,
    hub: &'a Hub,
    keys: &'a Keys,
    alias: String,
}

impl<'a> Pairing<'a> {
    pub fn new(st: Arc<dyn Storage>, hub: &'a Hub, keys: &'a Keys, alias: String) -> Self {
        Self { st, hub, keys, alias }
    }

    pub fn pending(&self) -> Result<Vec<Pending>> {
        let mut out = vec![];
        for name in self.st.names("pending/")? {
            if let Some(v) = self.st.get(&name)? {
                out.push(serde_json::from_value(v)?);
            }
        }
        Ok(out)
    }

    /// Open a mailbox and post our half of the PAKE. Returns immediately.
    pub async fn invite(&self) -> Result<Invite> {
        let box_ = self.hub.request("POST", "/api/pair", Some(json!({})), None).await?;
        let nameplate = box_["nameplate"].as_str().unwrap_or_default().to_string();
        let token = box_["token"].as_str().unwrap_or_default().to_string();
        let secret = make_secret();
        let p = self.begin(Role::A, nameplate, secret, token, box_["expires_at"].as_u64().unwrap_or(0)).await?;
        let link_base = self.hub.config(false).await?.pair_link_base;
        Ok(Invite {
            code: format_code(&p.nameplate, &p.secret),
            link: make_link(&link_base, &p.nameplate, &p.secret),
            nameplate: p.nameplate,
            expires_at: p.expires_at,
        })
    }

    /// Join someone else's invite. Finishes now if the other side answers
    /// within `wait`; otherwise a later advance() finishes it.
    pub async fn accept(&self, code: &str, wait: Duration) -> Result<Outcome> {
        let (nameplate, secret) = parse_code(code, &self.hub.config(false).await?.pair_link_base)?;
        let claim = match self.hub.request("POST", &format!("/api/pair/{nameplate}/claim"), Some(json!({})), None).await {
            Ok(c) => c,
            Err(e) => match e.downcast::<HubError>() {
                Ok(h) => return fail(&h.message),
                Err(e) => return Err(e),
            },
        };
        let token = claim["token"].as_str().unwrap_or_default().to_string();
        let p = self.begin(Role::B, nameplate, secret, token, claim["expires_at"].as_u64().unwrap_or(0)).await?;
        self.advance(&p.nameplate, wait, true).await
    }

    async fn begin(&self, role: Role, nameplate: String, secret: String, token: String, expires_at: u64) -> Result<Pending> {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let message = myous_pake::start(role, &format_code(&nameplate, &secret), seed);
        self.post(&nameplate, &token, &json!({"t": "pake", "v": VERSION, "m": B64.encode(message)})).await?;
        let p = Pending {
            role: if role == Role::A { "a" } else { "b" }.into(),
            nameplate, secret, token, expires_at,
            pake_seed: hex(&seed), after: 0, stage: "wait_pake".into(), key: None,
        };
        self.save(&p)?;
        Ok(p)
    }

    /// Move every pending pairing forward without waiting. Returns the ones
    /// that finished or failed.
    pub async fn advance_all(&self) -> Result<Vec<Outcome>> {
        let mut finished = vec![];
        for p in self.pending()? {
            // Hub unreachable: try again next time.
            if let Ok(o @ (Outcome::Done { .. } | Outcome::Failed(_))) = self.advance(&p.nameplate, Duration::ZERO, false).await {
                finished.push(o);
            }
        }
        Ok(finished)
    }

    /// Move one pairing forward with whatever the peer has posted. With
    /// block=false, returns Waiting if another run is working on it.
    pub async fn advance(&self, nameplate: &str, wait: Duration, block: bool) -> Result<Outcome> {
        let name = format!("pending/{nameplate}");
        let Some(_lock) = self.st.lock(&name, block)? else { return Ok(Outcome::Waiting) };
        let Some(current) = self.st.get(&name)? else { return Ok(Outcome::Elsewhere) };
        let mut p: Pending = serde_json::from_value(current)?;
        let deadline = Instant::now() + wait;
        loop {
            if now() > p.expires_at {
                return self.fail(&p, "pairing invite expired").await;
            }
            let wait_secs = deadline.saturating_duration_since(Instant::now()).as_secs().min(25);
            let endpoint = format!("/api/pair/{}/messages?after={}&wait={wait_secs}", p.nameplate, p.after);
            let got = match self.hub.request("GET", &endpoint, None, Some(&p.token)).await {
                Ok(g) => g,
                Err(e) => match e.downcast_ref::<HubError>() {
                    Some(h) if h.status == 403 || h.status == 404 => {
                        return self.fail(&p, "pairing invite expired or was closed").await;
                    }
                    _ => return Err(e),
                },
            };
            for body in got["messages"].as_array().cloned().unwrap_or_default() {
                p.after += 1;
                let msg: Option<Value> = body.as_str()
                    .and_then(|b| B64.decode(b).ok())
                    .and_then(|b| serde_json::from_slice(&b).ok());
                match self.step(&mut p, msg.unwrap_or(Value::Null)).await {
                    Ok(Some(done)) => return Ok(done),
                    Ok(None) => {}
                    Err(e) => match e.downcast::<PairingError>() {
                        Ok(pe) => return self.fail(&p, &pe.0).await,
                        Err(e) => return Err(e),
                    },
                }
            }
            self.save(&p)?;
            if Instant::now() >= deadline {
                return Ok(Outcome::Waiting);
            }
        }
    }

    async fn step(&self, p: &mut Pending, msg: Value) -> Result<Option<Outcome>> {
        let role = Role::parse(&p.role).unwrap();
        let peer_role = if role == Role::A { "b" } else { "a" };
        if p.stage == "wait_pake" && msg["t"] == "pake" {
            let seed: [u8; 32] = unhex(&p.pake_seed).try_into().map_err(|_| PairingError("corrupt pairing state".into()))?;
            let peer = msg["m"].as_str().and_then(|m| B64.decode(m).ok());
            let key = peer
                .and_then(|m| myous_pake::finish(role, &format_code(&p.nameplate, &p.secret), seed, &m).ok())
                .ok_or_else(|| PairingError("bad message from the other side".into()))?;
            p.key = Some(B64.encode(&key));
            p.stage = "wait_payload".into();
            self.save(p)?;
            let mine = json!({"v": VERSION, "pubkey": self.keys.public_key().to_hex(), "alias": self.alias});
            self.post(&p.nameplate, &p.token, &seal(&key, &p.role, &p.nameplate, &mine, None)).await?;
            Ok(None)
        } else if p.stage == "wait_payload" && msg["t"] == "payload" {
            let key = B64.decode(p.key.as_deref().unwrap_or_default())?;
            let payload = unseal(&key, peer_role, &p.nameplate, &msg)?;
            let pubkey = payload["pubkey"].as_str().unwrap_or_default();
            if pubkey == self.keys.public_key().to_hex() {
                return fail("that's this agent's own invite");
            }
            let alias: String = payload["alias"].as_str().unwrap_or_default().chars().take(64).collect();
            let verify = verify_code(&key);
            let _lock = self.st.lock("state", true)?;
            let contact = contacts::add(&*self.st, pubkey, &alias)?;
            self.st.delete(&format!("pending/{}", p.nameplate))?;
            inbox::record(&*self.st, json!({
                "type": "paired", "peer": contact.npub, "alias": contact.alias,
                "text": format!("paired with {} (verification code {verify})", contact.alias),
            }))?;
            // Don't close the mailbox: the peer may not have read our payload
            // yet. It only holds ciphertext and expires on its own.
            Ok(Some(Outcome::Done { contact, verify }))
        } else {
            fail("unexpected message from the other side")
        }
    }

    async fn fail(&self, p: &Pending, error: &str) -> Result<Outcome> {
        self.st.delete(&format!("pending/{}", p.nameplate))?;
        let _ = self.hub.request("DELETE", &format!("/api/pair/{}", p.nameplate), None, Some(&p.token)).await;
        let _lock = self.st.lock("state", true)?;
        inbox::record(&*self.st, json!({
            "type": "pairing_failed", "text": format!("pairing {} failed: {error}", p.nameplate),
        }))?;
        Ok(Outcome::Failed(error.into()))
    }

    fn save(&self, p: &Pending) -> Result<()> {
        self.st.put(&format!("pending/{}", p.nameplate), &serde_json::to_value(p)?)
    }

    async fn post(&self, nameplate: &str, token: &str, msg: &Value) -> Result<()> {
        let body = B64.encode(serde_json::to_vec(msg)?);
        self.hub.request("POST", &format!("/api/pair/{nameplate}/messages"), Some(json!({"body": body})), Some(token)).await?;
        Ok(())
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published vectors (docs/test-vectors.json) must hold.
    #[test]
    fn test_vectors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/test-vectors.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let base = "https://myoushq.com/p/";

        for c in v["code_parsing"].as_array().unwrap() {
            let (n, s) = parse_code(c["input"].as_str().unwrap(), base).unwrap();
            assert_eq!(n, c["nameplate"].as_str().unwrap());
            assert_eq!(s, c["secret"].as_str().unwrap());
            assert_eq!(format_code(&n, &s), c["password"].as_str().unwrap());
        }

        let kd = &v["key_derivation"];
        let k = unhex(kd["K_hex"].as_str().unwrap());
        assert_eq!(hex(&derive(&k, "from a", 32)), kd["from_a_hex"].as_str().unwrap());
        assert_eq!(hex(&derive(&k, "from b", 32)), kd["from_b_hex"].as_str().unwrap());
        assert_eq!(hex(&derive(&k, "verify", 8)), kd["verify_bytes_hex"].as_str().unwrap());
        assert_eq!(verify_code(&k), kd["verify_code"].as_str().unwrap());

        let ps = &v["payload_seal"];
        let k = unhex(ps["K_hex"].as_str().unwrap());
        let nonce: [u8; 12] = unhex(ps["nonce_hex"].as_str().unwrap()).try_into().unwrap();
        let payload: Value = serde_json::from_str(ps["plaintext"].as_str().unwrap()).unwrap();
        assert_eq!(payload_json(&payload), ps["plaintext"].as_str().unwrap());
        let (role, np) = (ps["role"].as_str().unwrap(), ps["nameplate"].as_str().unwrap());
        assert_eq!(seal(&k, role, np, &payload, Some(nonce)), ps["message"]);
        assert_eq!(unseal(&k, role, np, &ps["message"]).unwrap(), payload);
    }

    #[test]
    fn bad_codes() {
        let base = "https://myoushq.com/p/";
        assert!(parse_code("4821-K7F3Q", base).is_err());
        assert!(parse_code("K7F3QX", base).is_err());
        assert!(parse_code("https://evil.example/p/4821#K7F3QX", base).is_err());
        assert!(parse_code("https://myoushq.com/p/4821", base).is_err());
    }
}

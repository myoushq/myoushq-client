//! Pairing two agents through the hub's mailbox (protocol.md §5).
//!
//! A pairing code looks like 4821-K7F3QX. The nameplate (4821) names a
//! mailbox on the hub; the secret (K7F3QX) never leaves the two agents. Both
//! sides run SPAKE2 with the full code as password, relayed through the
//! mailbox, and get a shared key the hub can't learn. Each side then sends
//! its identity (public key and alias) encrypted with that key, with a
//! signature by that key over a value derived from the shared key, which
//! proves the sender holds it. A wrong code, or a hub that tampers, makes
//! decryption fail and the pairing aborts. A pairing never changes an
//! existing contact: a key that is already one stays as it is (blocked
//! stays blocked).
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
use nostr_sdk::prelude::{Event, EventId, Keys, Kind, PublicKey, Signature, Timestamp};
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
        valid_pubkey(payload["pubkey"].as_str()?).then_some(payload)
    };
    open().ok_or_else(|| PairingError("the code didn't match (or the exchange was tampered with)".into()).into())
}

fn lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A key as protocol.md writes it: 64 lowercase hex characters, the x of a
/// point on secp256k1. PublicKey::from_hex alone also takes uppercase and
/// x values off the curve.
pub fn valid_pubkey(text: &str) -> bool {
    lower_hex(text, 64) && PublicKey::from_hex(text).is_ok_and(|pk| pk.xonly().is_ok())
}

/// The 32 bytes a side signs with its identity key, so the other side knows
/// it holds the key in its payload (protocol.md, section 5.4). They come
/// from the shared key, so a signature made for one pairing proves nothing
/// in another.
pub fn key_proof(key: &[u8], role: &str) -> [u8; 32] {
    derive(key, &format!("key proof from {role}"), 32).try_into().unwrap()
}

pub fn sign_proof(keys: &Keys, key: &[u8], role: &str) -> String {
    keys.sign_schnorr(key_proof(key, role)).to_hex()
}

/// Whether `sig` is a BIP-340 signature of `message` by `pubkey_hex`.
/// nostr has no bare Schnorr check, but Event::verify_signature checks the
/// signature over the event id as given (it doesn't recompute the id), so
/// an event whose id is `message` checks exactly that.
pub fn proof_holds(pubkey_hex: &str, message: [u8; 32], sig: &str) -> bool {
    if !lower_hex(pubkey_hex, 64) || !lower_hex(sig, 128) {
        return false;
    }
    let (Ok(pk), Ok(sig)) = (PublicKey::from_hex(pubkey_hex), Signature::from_hex(sig)) else { return false };
    Event::new(EventId::from_byte_array(message), pk, Timestamp::from(0), Kind::from(0u16), [], "", sig).verify_signature()
}

/// Whether the payload proves its key: false if it has no proof (clients
/// up to 0.7.5 send none; a later version will require it), an error if
/// the proof doesn't hold.
fn check_proof(key: &[u8], peer_role: &str, payload: &Value) -> Result<bool> {
    match &payload["sig"] {
        Value::Null => Ok(false),
        sig => match sig.as_str() {
            Some(s) if proof_holds(payload["pubkey"].as_str().unwrap_or_default(), key_proof(key, peer_role), s) => Ok(true),
            _ => fail("the other side didn't prove it holds the key it sent"),
        },
    }
}

/// Why a pairing that named a contact which isn't approved changed nothing.
fn known_refused(c: &Contact, proven: bool) -> String {
    let who = if proven {
        format!("answered by your contact \"{}\" ({})", c.alias, c.status)
    } else {
        format!("answered with the key of your contact \"{}\" ({}), which the other side didn't prove it holds", c.alias, c.status)
    };
    format!("{who}; nothing changed: if that is right, unblock it (myous unblock \"{}\")", c.alias)
}

// --- the exchange --------------------------------------------------------
// A pairing spans a few round trips, so its state is saved as a
// "pending/<nameplate>" document and finished by whichever call gets there
// first: accept (which waits a while), or any later advance().

/// Claiming retries when the answer is lost, after CLAIM_RETRY_DELAY, then
/// twice that.
const CLAIM_ATTEMPTS: u32 = 3;
const CLAIM_RETRY_DELAY: Duration = if cfg!(test) { Duration::from_millis(1) } else { Duration::from_secs(1) };

/// Whether a hub status turned the request down for good (gone, taken,
/// bad), as opposed to "not now" (429) or a proxy that couldn't reach the
/// hub (5xx).
fn refused(status: u16) -> bool {
    (400..500).contains(&status) && status != 429
}

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
    /// "posting" (our PAKE message saved, not yet confirmed posted), then
    /// "wait_pake", then "wait_payload".
    pub stage: String,
    /// Our PAKE message (the base64 body) while stage is "posting".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Shared key, base64, once the PAKE is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Relationship context to record on the contact once paired.
    #[serde(default)]
    pub context: crate::contacts::ContactContext,
    /// The handshake so far, for the owner to read when it fails.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace: Vec<TraceStep>,
}

/// One step of a pairing's handshake, kept in the pairing's own record (and
/// in "pairing-trace" when it fails) so the owner can see what happened; the
/// hub logs nothing. It never leaves this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceStep {
    pub at: u64,
    /// claim, post, post (repeat), wait, done, failed
    pub step: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// A resumed claim: when the hub gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<u64>,
}

const TRACE_LEN: usize = 20; // steps kept per pairing

/// The last failed pairing's nameplate and steps, oldest first, if it failed
/// at or after `since` (Unix seconds).
pub fn pairing_trace(st: &dyn Storage, since: u64) -> Option<(String, Vec<TraceStep>)> {
    let t = st.get("pairing-trace").ok()??;
    let steps: Vec<TraceStep> = serde_json::from_value(t["trace"].clone()).ok()?;
    (steps.last()?.at >= since).then(|| (t["nameplate"].as_str().unwrap_or_default().to_string(), steps))
}

// One result per pairing, never stored in bulk: the Contact's size is fine here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Still waiting for the other side.
    Waiting,
    /// `known`: the key was already a contact, which the pairing left as it was.
    Done { contact: Contact, verify: String, known: bool },
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
    pub async fn invite(&self, context: crate::contacts::ContactContext) -> Result<Invite> {
        let box_ = self.hub.request("POST", "/api/pair", Some(json!({})), None).await?;
        let nameplate = box_["nameplate"].as_str().unwrap_or_default().to_string();
        let token = box_["token"].as_str().unwrap_or_default().to_string();
        let secret = make_secret();
        let mut p = self.begin(Role::A, nameplate, secret, token, box_["expires_at"].as_u64().unwrap_or(0), context, vec![])?;
        self.post_pake(&mut p).await?;
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
    pub async fn accept(&self, code: &str, wait: Duration, context: crate::contacts::ContactContext) -> Result<Outcome> {
        let (nameplate, secret) = parse_code(code, &self.hub.config(false).await?.pair_link_base)?;
        if let Some(mine) = self.st.get(&format!("pending/{nameplate}"))? {
            let mine: Pending = serde_json::from_value(mine)?;
            if mine.role == "b" && mine.secret == secret {
                // Accepted before (e.g. the connection dropped); carry on with it.
                self.st.delete(&format!("claiming/{nameplate}"))?;
                return self.advance(&nameplate, wait, true).await;
            }
        }
        let claiming = format!("claiming/{nameplate}");
        let saved = self.st.get(&claiming)?.unwrap_or(Value::Null);
        let mut trace: Vec<TraceStep> = serde_json::from_value(saved["trace"].clone()).unwrap_or_default();
        let claim = match self.claim(&nameplate, saved["claim"].as_str(), &mut trace).await {
            Ok(c) => c,
            Err(e) => {
                self.keep_trace(&nameplate, &trace);
                return Err(e);
            }
        };
        let token = claim["token"].as_str().unwrap_or_default().to_string();
        let expires_at = claim["expires_at"].as_u64().unwrap_or(0);
        let p = self.begin(Role::B, nameplate, secret, token, expires_at, context, trace)?;
        self.st.delete(&claiming)?;
        self.advance(&p.nameplate, wait, true).await
    }

    /// Claim the invite's second side, retrying when the answer is lost
    /// (the connection drops, or a proxy answers 5xx).
    ///
    /// The claim carries a random id, saved in "claiming/<nameplate>" before
    /// the first attempt: the hub answers a repeat of the same id with the
    /// same token, so neither a retry here nor running the same accept again
    /// (after a crash, or when every attempt failed) burns the code. The
    /// record goes once the pairing is saved or the hub refuses the code; it
    /// stays while the hub can't be reached. `saved` is that record's id, if
    /// any; `trace` gets a step per attempt and is saved with the id.
    async fn claim(&self, nameplate: &str, saved: Option<&str>, trace: &mut Vec<TraceStep>) -> Result<Value> {
        let name = format!("claiming/{nameplate}");
        let id = match saved.filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))) {
            Some(id) => id.to_string(),
            None => {
                let mut b = [0u8; 16];
                rand::rngs::OsRng.fill_bytes(&mut b);
                hex(&b)
            }
        };
        let save = |trace: &Vec<TraceStep>| self.st.put(&name, &json!({"nameplate": nameplate, "claim": id, "trace": trace}));
        let mut lost = None;
        for attempt in 0..CLAIM_ATTEMPTS {
            save(trace)?;
            let answer = self.hub.request("POST", &format!("/api/pair/{nameplate}/claim"), Some(json!({"claim": id})), None).await;
            match &answer {
                Ok(c) => {
                    let claimed_at = if c["repeat"] == true { c["claimed_at"].as_u64() } else { None };
                    self.trace(trace, "claim", None, claimed_at);
                }
                Err(e) => self.trace(trace, "claim", Some(format!("{e:#}")), None),
            }
            match answer {
                Ok(c) => return Ok(c),
                Err(e) => match e.downcast::<HubError>() {
                    // Turned down; a 429 keeps the record for later.
                    Ok(h) if h.status < 500 => {
                        if refused(h.status) {
                            self.st.delete(&name)?;
                        }
                        return fail(&h.message);
                    }
                    // A proxy that couldn't get the hub's answer.
                    Ok(h) => lost = Some(anyhow::Error::from(h)),
                    Err(e) if e.is::<reqwest::Error>() => lost = Some(e),
                    Err(e) => return Err(e),
                },
            }
            if attempt + 1 < CLAIM_ATTEMPTS {
                tokio::time::sleep(CLAIM_RETRY_DELAY * (attempt + 1)).await;
            }
        }
        save(trace)?;
        Err(lost.expect("CLAIM_ATTEMPTS > 0")
            .context("could not reach the hub to accept the code (accepting the same code again carries on where this stopped)"))
    }

    /// Make our PAKE message and save it with the pending state, stage
    /// "posting", before anything is posted: if the answer to the post is
    /// lost, post_pake posts exactly this message again, never a new one.
    #[allow(clippy::too_many_arguments)]
    fn begin(&self, role: Role, nameplate: String, secret: String, token: String, expires_at: u64,
             context: crate::contacts::ContactContext, trace: Vec<TraceStep>) -> Result<Pending> {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let message = myous_pake::start(role, &format_code(&nameplate, &secret), seed);
        let body = B64.encode(serde_json::to_vec(&json!({"t": "pake", "v": VERSION, "m": B64.encode(message)}))?);
        let p = Pending {
            role: if role == Role::A { "a" } else { "b" }.into(),
            nameplate, secret, token, expires_at,
            pake_seed: hex(&seed), after: 0, stage: "posting".into(), key: None, context, trace,
            message: Some(body),
        };
        self.save(&p)?;
        Ok(p)
    }

    /// Post the saved PAKE message of a "posting" pairing and move it on to
    /// wait_pake. The hub takes a repeat of a side's last message without
    /// appending it.
    async fn post_pake(&self, p: &mut Pending) -> Result<()> {
        let body = p.message.clone().unwrap_or_default();
        let posted = self.hub.request("POST", &format!("/api/pair/{}/messages", p.nameplate),
                                      Some(json!({"body": body})), Some(&p.token)).await;
        let step = match &posted {
            Ok(a) if a["repeat"] == true => "post (repeat)",
            _ => "post",
        };
        self.trace(&mut p.trace, step, posted.as_ref().err().map(|e| format!("{e:#}")), None);
        if posted.is_ok() {
            (p.stage, p.message) = ("wait_pake".into(), None);
        }
        self.save(p)?;
        posted.map(|_| ())
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
            if p.stage == "posting" {
                if let Err(e) = self.post_pake(&mut p).await {
                    match e.downcast_ref::<HubError>() {
                        Some(h) if h.status == 403 || h.status == 404 => {
                            return self.fail(&p, "pairing invite expired or was closed").await;
                        }
                        Some(h) if h.status < 500 => {
                            let message = h.message.clone();
                            return self.fail(&p, &message).await;
                        }
                        None if !e.is::<reqwest::Error>() => return Err(e),
                        _ => {}
                    }
                    // Unreachable: post it again while there's time, else next poll.
                    if deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(2) {
                        return Ok(Outcome::Waiting);
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            }
            let wait_secs = deadline.saturating_duration_since(Instant::now()).as_secs().min(25);
            let endpoint = format!("/api/pair/{}/messages?after={}&wait={wait_secs}", p.nameplate, p.after);
            let got = match self.hub.request("GET", &endpoint, None, Some(&p.token)).await {
                Ok(g) => g,
                Err(e) => match e.downcast_ref::<HubError>() {
                    Some(h) if h.status == 403 || h.status == 404 => {
                        return self.fail(&p, "pairing invite expired or was closed").await;
                    }
                    None if e.is::<reqwest::Error>() => {
                        // Network trouble (proxies drop long polls): retry while
                        // there's time, else leave it pending for the next poll.
                        self.trace(&mut p.trace, "wait", Some(format!("{e:#}")), None);
                        if deadline.saturating_duration_since(Instant::now()) <= Duration::from_secs(2) {
                            self.save(&p)?;
                            return Ok(Outcome::Waiting);
                        }
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
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
            let mine = json!({"v": VERSION, "pubkey": self.keys.public_key().to_hex(), "alias": self.alias,
                              "sig": sign_proof(self.keys, &key, &p.role)});
            self.post(&p.nameplate, &p.token, &seal(&key, &p.role, &p.nameplate, &mine, None)).await?;
            Ok(None)
        } else if p.stage == "wait_payload" && msg["t"] == "payload" {
            let key = B64.decode(p.key.as_deref().unwrap_or_default())?;
            let payload = unseal(&key, peer_role, &p.nameplate, &msg)?;
            let pubkey = payload["pubkey"].as_str().unwrap_or_default();
            if pubkey == self.keys.public_key().to_hex() {
                return fail("that's this agent's own invite");
            }
            let proven = check_proof(&key, peer_role, &payload)?;
            let alias = contacts::clean_name(payload["alias"].as_str().unwrap_or_default());
            let verify = verify_code(&key);
            let _lock = self.st.lock("state", true)?;
            let known = contacts::load(&*self.st)?.remove(pubkey);
            if let Some(c) = known.as_ref().filter(|c| c.status != contacts::APPROVED) {
                return fail(&known_refused(c, proven));
            }
            let contact = contacts::add_with_context(&*self.st, pubkey, &alias, &p.context)?;
            // They learned our alias from the payload; a card follows if we have one.
            contacts::peer_knows(&*self.st, pubkey, &self.alias, "")?;
            self.st.delete(&format!("pending/{}", p.nameplate))?;
            let mut text = format!("paired with {} (verification code {verify})", contact.alias);
            let given = p.context.relationship.is_some() || p.context.sharing.is_some();
            if known.is_some() {
                text += &format!(". {} was already your contact: nothing changed", contact.alias);
                if given {
                    text += &format!(" (the relationship and sharing given with this invite were not applied; \
                        to change them: myous context {:?} --relationship ... --sharing \"...\")", contact.alias);
                }
            }
            if contact.relationship.is_none() && !(known.is_some() && given) {
                text += &format!(". Ask your owner how they know this contact and what you may share with it, \
                    then record it: myous context {:?} --relationship ... --sharing \"...\"", contact.alias);
            }
            inbox::record(&*self.st, json!({
                "type": "paired", "peer": contact.npub, "alias": contact.alias, "text": text,
            }))?;
            // Don't close the mailbox: the peer may not have read our payload
            // yet. It only holds ciphertext and expires on its own.
            Ok(Some(Outcome::Done { contact, verify, known: known.is_some() }))
        } else {
            fail("unexpected message from the other side")
        }
    }

    async fn fail(&self, p: &Pending, error: &str) -> Result<Outcome> {
        let mut trace = p.trace.clone();
        self.trace(&mut trace, "failed", Some(error.into()), None);
        self.keep_trace(&p.nameplate, &trace);
        self.st.delete(&format!("pending/{}", p.nameplate))?;
        let _ = self.hub.request("DELETE", &format!("/api/pair/{}", p.nameplate), None, Some(&p.token)).await;
        let _lock = self.st.lock("state", true)?;
        inbox::record(&*self.st, json!({
            "type": "pairing_failed", "text": format!("pairing {} failed: {error}", p.nameplate),
        }))?;
        Ok(Outcome::Failed(error.into()))
    }

    /// Note a step of the handshake (see TraceStep).
    fn trace(&self, t: &mut Vec<TraceStep>, step: &str, error: Option<String>, claimed_at: Option<u64>) {
        let host = match step {
            "claim" | "post" | "post (repeat)" | "wait" => url::Url::parse(&self.hub.url).ok()
                .and_then(|u| u.host_str().map(|h| u.port().map_or(h.to_string(), |p| format!("{h}:{p}"))))
                .unwrap_or_default(),
            _ => String::new(),
        };
        t.push(TraceStep { at: now(), step: step.into(), host, error: error.unwrap_or_default(), claimed_at });
        let excess = t.len().saturating_sub(TRACE_LEN);
        t.drain(..excess);
    }

    /// Save a failed pairing's trace as "pairing-trace".
    fn keep_trace(&self, nameplate: &str, trace: &[TraceStep]) {
        let _ = self.st.put("pairing-trace", &json!({"nameplate": nameplate, "trace": trace}));
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

        let kp = &v["key_proof"];
        let k = unhex(kp["K_hex"].as_str().unwrap());
        let (a, b) = (kp["from_a_hex"].as_str().unwrap(), kp["from_b_hex"].as_str().unwrap());
        assert_eq!((hex(&key_proof(&k, "a")), hex(&key_proof(&k, "b"))), (a.to_string(), b.to_string()));
        let (pk, sig) = (kp["pubkey_hex"].as_str().unwrap(), kp["sig_hex"].as_str().unwrap());
        let keys = Keys::parse(kp["secret_key_hex"].as_str().unwrap()).unwrap();
        assert_eq!(keys.public_key().to_hex(), pk);
        assert!(valid_pubkey(pk));
        let msg = |h: &str| -> [u8; 32] { unhex(h).try_into().unwrap() };
        assert!(proof_holds(pk, msg(a), sig));
        assert!(!proof_holds(pk, msg(b), sig));
        let flipped = format!("{}{}", &sig[..127], if sig.ends_with('0') { "1" } else { "0" });
        assert!(!proof_holds(pk, msg(a), &flipped));
        assert!(proof_holds(pk, msg(a), &sign_proof(&keys, &k, "a")));
    }

    #[test]
    fn bad_codes() {
        let base = "https://myoushq.com/p/";
        assert!(parse_code("4821-K7F3Q", base).is_err());
        assert!(parse_code("K7F3QX", base).is_err());
        assert!(parse_code("https://evil.example/p/4821#K7F3QX", base).is_err());
        assert!(parse_code("https://myoushq.com/p/4821", base).is_err());
    }

    // --- claiming --------------------------------------------------------

    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::storage::FileStorage;

    const NAMEPLATE: &str = "4821";

    /// A fake hub with the claim rules (a repeat with the claimer's id gets
    /// the same token; anything else after the claim gets 409) and a mailbox
    /// that takes our messages and has none for us. It loses the first
    /// `lose` claim answers after acting on them (closes the connection),
    /// and answers the statuses in `fail` before acting.
    #[derive(Default)]
    struct ClaimHub {
        lose: usize,
        fail: Vec<u16>,
        claimed_by: Option<String>,
        claimed_at: u64,
        claims: Vec<String>,
        posts: usize,
    }

    /// A fake hub's answer to one request; None drops the connection.
    trait Fake: Send + 'static {
        fn answer(&mut self, method: &str, path: &str, body: &Value, auth: Option<&str>) -> Option<(u16, Value)>;
    }

    /// Serves `state` on a local port; returns the URL.
    async fn serve<T: Fake>(state: Arc<Mutex<T>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut conn, _) = listener.accept().await.unwrap();
                let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
                // Read the head, then the body by Content-Length.
                let head_end = loop {
                    let n = conn.read(&mut chunk).await.unwrap();
                    if n == 0 { break None; }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break Some(i + 4); }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let length: usize = head.lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                    .unwrap_or(0);
                while buf.len() < head_end + length {
                    let n = conn.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body: Value = serde_json::from_slice(&buf[head_end..head_end + length]).unwrap_or(Value::Null);
                let mut words = head.split_whitespace();
                let (method, path) = (words.next().unwrap().to_string(), words.next().unwrap().to_string());
                let auth = head.lines().find_map(|l| l.strip_prefix("authorization: Bearer ").map(String::from));
                let answer = {
                    let mut h = state.lock().unwrap();
                    h.answer(&method, &path, &body, auth.as_deref())
                };
                if let Some((status, json)) = answer {
                    let text = json.to_string();
                    let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
                    conn.write_all(resp.as_bytes()).await.unwrap();
                }
                // None: drop the connection without an answer.
            }
        });
        url
    }

    impl Fake for ClaimHub {
        fn answer(&mut self, method: &str, path: &str, body: &Value, auth: Option<&str>) -> Option<(u16, Value)> {
            let mailbox = format!("/api/pair/{NAMEPLATE}");
            let token = self.claimed_by.as_ref().map(|c| format!("tok-{c}"));
            if method == "POST" && path == format!("{mailbox}/claim") {
                let id = body["claim"].as_str().unwrap_or_default().to_string();
                self.claims.push(id.clone());
                if !self.fail.is_empty() {
                    return Some((self.fail.remove(0), json!({"error": "refused"})));
                }
                let mut answer = json!({"repeat": self.claimed_by.is_some()});
                match &self.claimed_by {
                    None => (self.claimed_by, self.claimed_at) = (Some(id), now() - 5),
                    Some(c) if *c != id => return Some((409, json!({"error": "invite already used, ask for a new one"}))),
                    Some(_) => answer["claimed_at"] = json!(self.claimed_at),
                }
                if self.lose > 0 {
                    self.lose -= 1;
                    return None;
                }
                answer["token"] = json!(format!("tok-{}", self.claimed_by.as_ref().unwrap()));
                answer["expires_at"] = json!(now() + 900);
                Some((200, answer))
            } else if method == "POST" && path == format!("{mailbox}/messages") {
                if auth.map(String::from) != token {
                    return Some((403, json!({"error": "bad token"})));
                }
                self.posts += 1;
                Some((200, json!({"index": self.posts - 1})))
            } else if method == "GET" && path.starts_with(&format!("{mailbox}/messages?")) {
                Some((200, json!({"messages": [], "next": 0})))
            } else {
                Some((404, json!({"error": format!("unexpected {method} {path}")})))
            }
        }
    }

    struct ClaimFixture {
        _dir: tempfile::TempDir,
        st: Arc<FileStorage>,
        state: Arc<Mutex<ClaimHub>>,
        hub: Hub,
        keys: Keys,
    }

    async fn claim_fixture(state: ClaimHub) -> ClaimFixture {
        let dir = tempfile::tempdir().unwrap();
        let st = Arc::new(FileStorage::new(Some(dir.path().to_path_buf())).unwrap());
        let state = Arc::new(Mutex::new(state));
        let url = serve(state.clone()).await;
        st.put("settings", &json!({"hub": url})).unwrap();
        st.put("hub", &json!({"url": url, "fetched_at": now(), "pair_link_base": "https://hub.test/p/"})).unwrap();
        let hub = Hub::new(st.clone()).unwrap();
        ClaimFixture { _dir: dir, st, state, hub, keys: Keys::generate() }
    }

    impl ClaimFixture {
        async fn accept(&self) -> Result<Outcome> {
            let pairing = Pairing::new(self.st.clone(), &self.hub, &self.keys, "me".into());
            pairing.accept(&format!("{NAMEPLATE}-K7F3QX"), Duration::ZERO, Default::default()).await
        }

        fn saved_claim(&self) -> Option<String> {
            self.st.get(&format!("claiming/{NAMEPLATE}")).unwrap().map(|v| v["claim"].as_str().unwrap().to_string())
        }

        fn claims(&self) -> Vec<String> {
            self.state.lock().unwrap().claims.clone()
        }

        fn pending_token(&self) -> String {
            self.st.get(&format!("pending/{NAMEPLATE}")).unwrap().unwrap()["token"].as_str().unwrap().to_string()
        }
    }

    #[tokio::test]
    async fn claim_lost_answer_then_retry() {
        let f = claim_fixture(ClaimHub { lose: 1, ..Default::default() }).await;
        assert_eq!(f.accept().await.unwrap(), Outcome::Waiting);
        let claims = f.claims();
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0], claims[1]);
        assert!(claims[0].len() == 32 && claims[0].bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(f.pending_token(), format!("tok-{}", claims[0]));
        assert_eq!(f.state.lock().unwrap().posts, 1);
        assert_eq!(f.saved_claim(), None, "the claiming record goes once the pairing is saved");
    }

    #[tokio::test]
    async fn claim_proxy_error_is_retried() {
        let f = claim_fixture(ClaimHub { fail: vec![502], ..Default::default() }).await;
        assert_eq!(f.accept().await.unwrap(), Outcome::Waiting);
        assert_eq!(f.claims().len(), 2);
    }

    #[tokio::test]
    async fn claim_rerun_after_crash_reuses_id() {
        let id = "ab".repeat(16);
        let f = claim_fixture(ClaimHub { claimed_by: Some(id.clone()), ..Default::default() }).await;
        // The first run saved its id and died before it saw the hub's answer.
        f.st.put(&format!("claiming/{NAMEPLATE}"), &json!({"nameplate": NAMEPLATE, "claim": id})).unwrap();
        assert_eq!(f.accept().await.unwrap(), Outcome::Waiting);
        assert_eq!(f.claims(), vec![id.clone()]);
        assert_eq!(f.pending_token(), format!("tok-{id}"));
        assert_eq!(f.saved_claim(), None);
    }

    #[tokio::test]
    async fn claim_unreachable_keeps_id_for_next_run() {
        let f = claim_fixture(ClaimHub { lose: CLAIM_ATTEMPTS as usize, ..Default::default() }).await;
        let err = f.accept().await.unwrap_err();
        let text = format!("{err:#}");
        assert!(text.starts_with("could not reach the hub"), "{text}");
        assert!(err.is::<reqwest::Error>() || err.downcast_ref::<reqwest::Error>().is_some(), "the drop stays the cause: {text}");
        assert!(err.downcast_ref::<PairingError>().is_none());
        let claims = f.claims();
        assert_eq!(claims.len(), CLAIM_ATTEMPTS as usize);
        assert_eq!(f.saved_claim().as_ref(), Some(&claims[0]));
        // The same accept again carries on with the claim the hub already gave.
        assert_eq!(f.accept().await.unwrap(), Outcome::Waiting);
        assert!(f.claims().iter().all(|c| *c == claims[0]));
        assert_eq!(f.pending_token(), format!("tok-{}", claims[0]));
        assert_eq!(f.saved_claim(), None);
    }

    #[tokio::test]
    async fn claim_taken_by_someone_else() {
        let f = claim_fixture(ClaimHub { claimed_by: Some("cd".repeat(16)), ..Default::default() }).await;
        let err = f.accept().await.unwrap_err();
        assert_eq!(err.downcast_ref::<PairingError>().map(|e| e.0.as_str()), Some("invite already used, ask for a new one"));
        assert_eq!(f.claims().len(), 1, "a refusal is not retried");
        assert_eq!(f.saved_claim(), None, "a refusal drops the claiming record");
    }

    #[tokio::test]
    async fn claim_rate_limit_keeps_id() {
        let f = claim_fixture(ClaimHub { fail: vec![429], ..Default::default() }).await;
        assert!(f.accept().await.is_err());
        let claims = f.claims();
        assert_eq!(claims.len(), 1);
        assert_eq!(f.saved_claim().as_ref(), Some(&claims[0]));
    }

    #[tokio::test]
    async fn claim_trace_names_the_drop_and_the_resume() {
        let f = claim_fixture(ClaimHub { lose: 1, ..Default::default() }).await;
        assert_eq!(f.accept().await.unwrap(), Outcome::Waiting);
        let p: Pending = serde_json::from_value(f.st.get(&format!("pending/{NAMEPLATE}")).unwrap().unwrap()).unwrap();
        let steps: Vec<&str> = p.trace.iter().map(|s| s.step.as_str()).collect();
        assert_eq!(steps, ["claim", "claim", "post"]);
        let host = f.hub.url.trim_start_matches("http://");
        assert!(!p.trace[0].error.is_empty() && p.trace[0].host == host, "the drop: {:?}", p.trace[0]);
        assert_eq!(p.trace[1].error, "");
        assert_eq!(p.trace[1].claimed_at, Some(f.state.lock().unwrap().claimed_at), "the resume");
    }

    #[tokio::test]
    async fn failed_accept_keeps_the_trace() {
        let f = claim_fixture(ClaimHub { claimed_by: Some("cd".repeat(16)), ..Default::default() }).await;
        let started = now();
        assert!(f.accept().await.is_err());
        let (nameplate, steps) = pairing_trace(&*f.st, started).expect("a trace");
        assert_eq!(nameplate, NAMEPLATE);
        assert_eq!(steps.len(), 1);
        assert!(steps[0].step == "claim" && steps[0].error.contains("invite already used"), "{steps:?}");
        assert!(pairing_trace(&*f.st, started + 3600).is_none(), "an older trace counted as this run's");
    }

    /// A fake hub with both sides of one mailbox, as the hub keeps it: a claim
    /// id that repeats, and a side's repeat of its last message answered
    /// without appending. The answers to the first `lose_posts` of side B's
    /// posts are lost after the message is stored.
    #[derive(Default)]
    struct MailboxHub {
        lose_posts: usize,
        claimed_by: Option<String>,
        a: Vec<String>,
        b: Vec<String>,
    }

    impl Fake for MailboxHub {
        fn answer(&mut self, method: &str, path: &str, body: &Value, auth: Option<&str>) -> Option<(u16, Value)> {
            let mailbox = format!("/api/pair/{NAMEPLATE}");
            let side_b = auth == Some("tok-b");
            let exp = now() + 900;
            if method == "POST" && path == "/api/pair" {
                Some((201, json!({"nameplate": NAMEPLATE, "token": "tok-a", "expires_at": exp})))
            } else if method == "POST" && path == format!("{mailbox}/claim") {
                let id = body["claim"].as_str().unwrap_or_default().to_string();
                if self.claimed_by.as_ref().is_some_and(|c| *c != id) {
                    return Some((409, json!({"error": "invite already used, ask for a new one"})));
                }
                let repeat = self.claimed_by.replace(id).is_some();
                Some((200, json!({"token": "tok-b", "expires_at": exp, "repeat": repeat})))
            } else if method == "POST" && path == format!("{mailbox}/messages") {
                let text = body["body"].as_str().unwrap_or_default().to_string();
                let mine = if side_b { &mut self.b } else { &mut self.a };
                if mine.last() == Some(&text) {
                    return Some((200, json!({"index": mine.len() - 1, "repeat": true})));
                }
                mine.push(text);
                let index = mine.len() - 1;
                if side_b && self.lose_posts > 0 {
                    self.lose_posts -= 1;
                    return None;
                }
                Some((200, json!({"index": index})))
            } else if method == "GET" && path.starts_with(&format!("{mailbox}/messages?")) {
                let after: usize = path.split("after=").nth(1).and_then(|r| r.split('&').next()).unwrap().parse().unwrap();
                let theirs = if side_b { &self.a } else { &self.b };
                let msgs: Vec<&String> = theirs.iter().skip(after).collect();
                Some((200, json!({"next": after + msgs.len(), "messages": msgs})))
            } else if method == "DELETE" {
                Some((204, json!({})))
            } else {
                Some((404, json!({"error": "unexpected"})))
            }
        }
    }

    #[tokio::test]
    async fn lost_post_answer_rerun_completes_without_second_pake() {
        let state = Arc::new(Mutex::new(MailboxHub { lose_posts: 1, ..Default::default() }));
        let url = serve(state.clone()).await;
        let side = || {
            let dir = tempfile::tempdir().unwrap();
            let st = Arc::new(FileStorage::new(Some(dir.path().to_path_buf())).unwrap());
            st.put("settings", &json!({"hub": url})).unwrap();
            st.put("hub", &json!({"url": url, "fetched_at": now(), "pair_link_base": "https://hub.test/p/"})).unwrap();
            let hub = Hub::new(st.clone()).unwrap();
            (dir, st, hub, Keys::generate())
        };
        let (_da, st_a, hub_a, keys_a) = side();
        let (_db, st_b, hub_b, keys_b) = side();
        let a = Pairing::new(st_a, &hub_a, &keys_a, "alice".into());
        let b = Pairing::new(st_b.clone(), &hub_b, &keys_b, "bob".into());
        let code = a.invite(Default::default()).await.unwrap().code;

        // The post reaches the mailbox; its answer doesn't.
        assert_eq!(b.accept(&code, Duration::ZERO, Default::default()).await.unwrap(), Outcome::Waiting);
        let saved: Pending = serde_json::from_value(st_b.get(&format!("pending/{NAMEPLATE}")).unwrap().unwrap()).unwrap();
        assert_eq!(saved.stage, "posting");
        assert_eq!(saved.message.as_ref(), state.lock().unwrap().b.first(), "the exact message is saved");

        // The same accept again.
        assert_eq!(b.accept(&code, Duration::ZERO, Default::default()).await.unwrap(), Outcome::Waiting);
        let p: Pending = serde_json::from_value(st_b.get(&format!("pending/{NAMEPLATE}")).unwrap().unwrap()).unwrap();
        assert_eq!(p.stage, "wait_payload");
        let steps: Vec<&str> = p.trace.iter().map(|s| s.step.as_str()).collect();
        assert_eq!(steps, ["claim", "post", "post (repeat)"]);
        assert!(!p.trace[1].error.is_empty());

        let done_a = a.advance_all().await.unwrap();
        let done_b = b.advance_all().await.unwrap();
        let verify = |o: &[Outcome]| match o {
            [Outcome::Done { verify, .. }] => verify.clone(),
            other => panic!("not done: {other:?}"),
        };
        assert_eq!(verify(&done_a), verify(&done_b), "side A completes, and B");
        let pakes = state.lock().unwrap().b.iter()
            .filter(|m| String::from_utf8(B64.decode(m).unwrap()).unwrap().contains("\"pake\""))
            .count();
        assert_eq!(pakes, 1, "one PAKE message from B, not two");
    }

    // --- the key a pairing pins ----------------------------------------------
    // The other side proves it holds the key it sends (a signature over a
    // value only the two sides know), and a pairing never changes a key that
    // is already a contact: a blocked contact stays blocked, an approved one
    // keeps its alias and context. Clients from before the proof send none
    // and still pair.

    use crate::contacts::ContactContext;
    use nostr_sdk::prelude::ToBech32;

    fn erin() -> ContactContext {
        ContactContext { relationship: Some("friend".into()), sharing: Some("my calendar and home address".into()), added_by: None }
    }

    /// Side A (a real pairing) and a mailbox for side B, which a test writes
    /// out by hand so it can send any payload.
    struct KeyFixture {
        _dir: tempfile::TempDir,
        st: Arc<FileStorage>,
        hub: Hub,
        keys: Keys,
    }

    async fn key_fixture() -> KeyFixture {
        let url = serve(Arc::new(Mutex::new(MailboxHub::default()))).await;
        let dir = tempfile::tempdir().unwrap();
        let st = Arc::new(FileStorage::new(Some(dir.path().to_path_buf())).unwrap());
        st.put("settings", &json!({"hub": url})).unwrap();
        st.put("hub", &json!({"url": url, "fetched_at": now(), "pair_link_base": "https://hub.test/p/"})).unwrap();
        let hub = Hub::new(st.clone()).unwrap();
        KeyFixture { _dir: dir, st, hub, keys: Keys::generate() }
    }

    /// A payload naming `k`, its proof signed by `signer` (None: no proof,
    /// as clients up to 0.7.5 send).
    fn payload_of(k: &Keys, alias: &str, signer: Option<&Keys>) -> impl Fn(&[u8]) -> Value {
        let (pubkey, alias, signer) = (k.public_key().to_hex(), alias.to_string(), signer.cloned());
        move |key| {
            let mut p = json!({"v": 1, "pubkey": pubkey, "alias": alias});
            if let Some(s) = &signer {
                p["sig"] = json!(sign_proof(s, key, "b"));
            }
            p
        }
    }

    impl KeyFixture {
        async fn post(&self, token: &str, msg: Value) {
            let body = B64.encode(serde_json::to_vec(&msg).unwrap());
            self.hub.request("POST", &format!("/api/pair/{NAMEPLATE}/messages"), Some(json!({"body": body})), Some(token)).await.unwrap();
        }

        /// Invite with `cc`; side B answers with make(K). Returns A's
        /// outcome, the payload A sent, and K.
        async fn pair(&self, cc: ContactContext, make: impl Fn(&[u8]) -> Value) -> (Outcome, Value, Vec<u8>) {
            let me = Pairing::new(self.st.clone(), &self.hub, &self.keys, "me".into());
            let code = me.invite(cc).await.unwrap().code;
            let claim = self.hub.request("POST", &format!("/api/pair/{NAMEPLATE}/claim"), Some(json!({"claim": "ab".repeat(16)})), None)
                .await.unwrap();
            let token = claim["token"].as_str().unwrap().to_string();
            let seed = [7u8; 32];
            self.post(&token, json!({"t": "pake", "v": 1, "m": B64.encode(myous_pake::start(Role::B, &code, seed))})).await;
            assert_eq!(me.advance_all().await.unwrap(), vec![], "A reads the PAKE and sends its payload");
            let got = self.hub.request("GET", &format!("/api/pair/{NAMEPLATE}/messages?after=0&wait=0"), None, Some(&token)).await.unwrap();
            let msgs: Vec<Value> = got["messages"].as_array().unwrap().iter()
                .map(|b| serde_json::from_slice(&B64.decode(b.as_str().unwrap()).unwrap()).unwrap())
                .collect();
            let key = myous_pake::finish(Role::B, &code, seed, &B64.decode(msgs[0]["m"].as_str().unwrap()).unwrap()).unwrap();
            self.post(&token, seal(&key, "b", NAMEPLATE, &make(&key), None)).await;
            let mine = unseal(&key, "a", NAMEPLATE, &msgs[1]).unwrap();
            let mut done = me.advance_all().await.unwrap();
            assert_eq!(done.len(), 1, "{done:?}");
            (done.remove(0), mine, key)
        }

        fn contacts(&self) -> contacts::Contacts {
            contacts::load(&*self.st).unwrap()
        }

        fn last(&self) -> Value {
            self.st.read_history().unwrap().pop().unwrap()
        }

        fn block(&self, k: &Keys, alias: &str) {
            contacts::add(&*self.st, &k.public_key().to_hex(), alias).unwrap();
            contacts::set_status(&*self.st, alias, contacts::BLOCKED).unwrap();
        }
    }

    #[tokio::test]
    async fn key_this_side_proves_its_key() {
        let f = key_fixture().await;
        let bob = Keys::generate();
        let (outcome, mine, key) = f.pair(erin(), payload_of(&bob, "bob", Some(&bob))).await;
        assert!(matches!(outcome, Outcome::Done { known: false, .. }), "{outcome:?}");
        let (pk, sig) = (mine["pubkey"].as_str().unwrap(), mine["sig"].as_str().unwrap());
        assert_eq!(pk, f.keys.public_key().to_hex());
        assert!(proof_holds(pk, key_proof(&key, "a"), sig));
        assert!(!proof_holds(pk, key_proof(&key, "b"), sig), "a proof is for one side");
        assert_eq!(f.contacts()[&bob.public_key().to_hex()].relationship.as_deref(), Some("friend"));
    }

    #[tokio::test]
    async fn key_a_client_without_the_proof_still_pairs() {
        let f = key_fixture().await;
        let bob = Keys::generate();
        let (outcome, _, _) = f.pair(erin(), payload_of(&bob, "bob", None)).await;
        assert!(matches!(outcome, Outcome::Done { known: false, .. }), "{outcome:?}");
        let c = &f.contacts()[&bob.public_key().to_hex()];
        assert_eq!((c.alias.as_str(), c.status.as_str(), c.sharing.as_deref()), ("bob", "approved", Some("my calendar and home address")));
        assert_eq!(f.last()["type"], "paired");
    }

    /// The audit's M2: whoever answers names a blocked contact's key.
    #[tokio::test]
    async fn key_a_blocked_contact_stays_blocked() {
        for proven in [false, true] {
            let f = key_fixture().await;
            let eve = Keys::generate();
            f.block(&eve, "eve");
            let (outcome, _, _) = f.pair(erin(), payload_of(&eve, "Erin", proven.then_some(&eve))).await;
            assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
            let all = f.contacts();
            let c = &all[&eve.public_key().to_hex()];
            assert_eq!((all.len(), c.alias.as_str(), c.status.as_str(), c.relationship.as_deref(), c.sharing.as_deref()),
                       (1, "eve", "blocked", None, None));
            let want = if proven {
                "answered by your contact \"eve\" (blocked); nothing changed"
            } else {
                "answered with the key of your contact \"eve\" (blocked), which the other side didn't prove it holds; \
                 nothing changed: if that is right, unblock it (myous unblock \"eve\")"
            };
            let last = f.last();
            assert_eq!(last["type"], "pairing_failed");
            assert!(last["text"].as_str().unwrap().contains(want), "{last}");
        }
    }

    #[tokio::test]
    async fn key_a_bad_proof_fails() {
        let (eve, mallory) = (Keys::generate(), Keys::generate());
        let elsewhere = sign_proof(&eve, &[9u8; 32], "b");
        let cases: Vec<(&str, Box<dyn Fn(&[u8]) -> Value>)> = vec![
            ("another key's", Box::new(|key| json!(sign_proof(&mallory, key, "b")))),
            ("another pairing's", Box::new(|_| json!(elsewhere))),
            ("uppercase", Box::new(|key| json!(sign_proof(&eve, key, "b").to_uppercase()))),
            ("empty", Box::new(|_| json!(""))),
            ("zeros", Box::new(|_| json!("00".repeat(64)))),
            ("short", Box::new(|_| json!("ab"))),
            ("a number", Box::new(|_| json!(3))),
        ];
        for (name, sig) in cases {
            let f = key_fixture().await;
            let eve_hex = eve.public_key().to_hex();
            let (outcome, _, _) = f.pair(ContactContext::default(), |key| json!({"v": 1, "pubkey": eve_hex, "alias": "Erin", "sig": sig(key)})).await;
            assert_eq!(outcome, Outcome::Failed("the other side didn't prove it holds the key it sent".into()), "{name}");
            assert!(f.contacts().is_empty(), "{name}");
        }
    }

    #[tokio::test]
    async fn key_an_approved_contact_keeps_its_context() {
        let f = key_fixture().await;
        let bob = Keys::generate();
        let bob_hex = bob.public_key().to_hex();
        let cc = ContactContext { relationship: Some("colleague".into()), sharing: Some("work only".into()), added_by: None };
        let before = contacts::add_with_context(&*f.st, &bob_hex, "bob", &cc).unwrap();
        let (outcome, _, _) = f.pair(erin(), payload_of(&bob, "Erin", None)).await;
        assert!(matches!(outcome, Outcome::Done { known: true, .. }), "{outcome:?}");
        let mut after = f.contacts().remove(&bob_hex).unwrap();
        after.peer_knows = None; // set again: the peer learned our alias
        assert_eq!(after, before, "alias, status and context as they were");
        assert_eq!(f.contacts().len(), 1);
        let last = f.last();
        assert_eq!(last["type"], "paired");
        assert!(last["text"].as_str().unwrap().contains("bob was already your contact: nothing changed (the relationship and sharing \
            given with this invite were not applied; to change them: myous context \"bob\""), "{last}");
    }

    #[tokio::test]
    async fn key_written_otherwise_is_refused() {
        let bob = Keys::generate();
        let f0 = key_fixture().await;
        let own_upper = f0.keys.public_key().to_hex().to_uppercase();
        for key in [bob.public_key().to_hex().to_uppercase(), bob.public_key().to_bech32().unwrap(), "ff".repeat(32)] {
            let f = key_fixture().await;
            let (outcome, _, _) = f.pair(ContactContext::default(), |_| json!({"v": 1, "pubkey": key, "alias": "x"})).await;
            assert_eq!(outcome, Outcome::Failed("the code didn't match (or the exchange was tampered with)".into()), "{key}");
            assert!(f.contacts().is_empty());
        }
        let (outcome, _, _) = f0.pair(ContactContext::default(), |_| json!({"v": 1, "pubkey": own_upper, "alias": "x"})).await;
        assert!(matches!(outcome, Outcome::Failed(_)), "own key in uppercase: {outcome:?}");
        assert!(f0.contacts().is_empty());
    }

    /// The context goes to the new contact by its key, never to a contact
    /// whose alias happens to be that key.
    #[tokio::test]
    async fn key_an_alias_that_names_a_key_gets_no_context() {
        for _ in 0..4 { // the contacts' order (by key) decided which one find() found
            let f = key_fixture().await;
            let (bob, mallory) = (Keys::generate(), Keys::generate());
            let bob_hex = bob.public_key().to_hex();
            contacts::add(&*f.st, &mallory.public_key().to_hex(), &bob_hex).unwrap();
            let (outcome, _, _) = f.pair(erin(), payload_of(&bob, "bob", Some(&bob))).await;
            assert!(matches!(outcome, Outcome::Done { .. }), "{outcome:?}");
            let all = f.contacts();
            assert_eq!(all[&bob_hex].relationship.as_deref(), Some("friend"));
            assert_eq!(all[&mallory.public_key().to_hex()].relationship, None);
        }
    }

    #[test]
    fn key_add_leaves_a_contact_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let st = FileStorage::new(Some(dir.path().to_path_buf())).unwrap();
        let eve = Keys::generate().public_key().to_hex();
        contacts::add_with_context(&st, &eve, "eve", &ContactContext { relationship: Some("other".into()), ..Default::default() }).unwrap();
        contacts::set_status(&st, "eve", contacts::BLOCKED).unwrap();
        let c = contacts::add_with_context(&st, &eve, "eve again", &erin()).unwrap();
        assert_eq!((c.alias.as_str(), c.status.as_str(), c.relationship.as_deref()), ("eve", "blocked", Some("other")));
    }
}

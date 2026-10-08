//! Files (protocol.md section 6): a file travels as an AES-256-GCM
//! encrypted blob on the hub plus a kind-15 message carrying the key. The
//! hub stores bytes it can't read and learns neither recipient nor name.

use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use nostr_sdk::prelude::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::hub::HubError;

/// Blossom (BUD-11) authorization event kind.
pub const KIND_BLOB_AUTH: u16 = 24242;
/// NIP-17 file message kind.
pub const KIND_FILE: u16 = 15;
/// How long an authorization stays valid; the hub accepts up to 10 minutes.
const AUTH_TTL: u64 = 5 * 60;

pub struct Encrypted {
    pub key: [u8; 32],
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
    /// SHA-256 of the ciphertext (the blob's name on the hub), hex.
    pub x: String,
    /// SHA-256 of the plaintext, hex.
    pub ox: String,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 {
        bail!("odd-length hex");
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!("bad hex: {e}"))).collect()
}

/// Encrypt a file with a fresh key and nonce.
pub fn encrypt(plaintext: &[u8]) -> Result<Encrypted> {
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 12];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);
    let ciphertext = Aes256Gcm::new_from_slice(&key)?
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| anyhow!("encryption failed"))?;
    Ok(Encrypted { key, nonce, x: sha256_hex(&ciphertext), ox: sha256_hex(plaintext), ciphertext })
}

/// Decrypt a blob, checking the ciphertext hash first and the plaintext
/// hash afterwards, so a tampered or swapped blob is refused.
pub fn decrypt(key_hex: &str, nonce_hex: &str, ciphertext: &[u8], x: &str, ox: &str) -> Result<Vec<u8>> {
    if sha256_hex(ciphertext) != x {
        bail!("the blob's hash doesn't match the message (x)");
    }
    let key = unhex(key_hex)?;
    let nonce = unhex(nonce_hex)?;
    if key.len() != 32 || nonce.len() != 12 {
        bail!("bad key or nonce length");
    }
    let plaintext = Aes256Gcm::new_from_slice(&key)?
        .decrypt(Nonce::from_slice(&nonce), ciphertext)
        .map_err(|_| anyhow!("decryption failed: wrong key or corrupted blob"))?;
    if sha256_hex(&plaintext) != ox {
        bail!("the decrypted file's hash doesn't match the message (ox)");
    }
    Ok(plaintext)
}

/// A file name from a message, reduced to its last path component; None
/// if nothing usable is left (empty, "." or "..").
pub fn sanitize_name(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let clean: String = last.chars().filter(|c| !c.is_control()).collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        return None;
    }
    Some(clean)
}

/// `Authorization` header value for one blob call: a signed kind-24242
/// event naming the action and the blob, base64url without padding.
pub fn auth_header(keys: &Keys, action: &str, x: &str) -> Result<String> {
    let expires = Timestamp::from_secs(crate::now() + AUTH_TTL);
    let tags = vec![Tag::parse(["t", action])?, Tag::parse(["x", x])?, Tag::expiration(expires)];
    let event = EventBuilder::new(Kind::Custom(KIND_BLOB_AUTH), format!("myous {action}")).tags(tags).finalize(keys)?;
    Ok(format!("Nostr {}", URL_SAFE_NO_PAD.encode(serde_json::to_string(&event)?)))
}

/// The hub's blob store, for one agent.
pub struct BlobClient {
    pub blob_api: String,
    keys: Keys,
    http: reqwest::Client,
}

impl BlobClient {
    pub fn new(blob_api: &str, keys: &Keys) -> Result<Self> {
        // Blobs are up to 64 MB: allow a slow link.
        let http = reqwest::Client::builder().timeout(Duration::from_secs(600)).build()?;
        Ok(Self { blob_api: blob_api.trim_end_matches('/').to_string(), keys: keys.clone(), http })
    }

    pub fn url_of(&self, x: &str) -> String {
        format!("{}/{x}", self.blob_api)
    }

    /// Store a ciphertext; returns the hub's blob descriptor.
    pub async fn upload(&self, ciphertext: &[u8], x: &str) -> Result<Value> {
        let resp = self.http.put(format!("{}/upload", self.blob_api))
            .header("Authorization", auth_header(&self.keys, "upload", x)?)
            .header("Content-Type", "application/octet-stream")
            .body(ciphertext.to_vec())
            .send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(hub_error(status, &text).into());
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn get(&self, x: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(self.url_of(x)).header("Authorization", auth_header(&self.keys, "get", x)?).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(hub_error(status, &resp.text().await.unwrap_or_default()).into());
        }
        Ok(resp.bytes().await?.to_vec())
    }

    pub async fn delete(&self, x: &str) -> Result<()> {
        let resp = self.http.delete(self.url_of(x)).header("Authorization", auth_header(&self.keys, "delete", x)?).send().await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(hub_error(status, &resp.text().await.unwrap_or_default()).into());
        }
        Ok(())
    }
}

fn hub_error(status: reqwest::StatusCode, text: &str) -> HubError {
    let message = serde_json::from_str::<Value>(text).ok()
        .and_then(|v| v["error"].as_str().map(String::from))
        .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string());
    HubError { status: status.as_u16(), message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Event;

    #[test]
    fn round_trip_and_tamper_detection() {
        let plain = b"the quick brown fox";
        let e = encrypt(plain).unwrap();
        assert_eq!(e.ciphertext.len(), plain.len() + 16);
        assert_eq!(e.ox, sha256_hex(plain));
        let got = decrypt(&hex(&e.key), &hex(&e.nonce), &e.ciphertext, &e.x, &e.ox).unwrap();
        assert_eq!(got, plain);

        let mut bad = e.ciphertext.clone();
        bad[0] ^= 1;
        assert!(decrypt(&hex(&e.key), &hex(&e.nonce), &bad, &e.x, &e.ox).unwrap_err().to_string().contains("(x)"));
        assert!(decrypt(&hex(&e.key), &hex(&e.nonce), &bad, &sha256_hex(&bad), &e.ox).unwrap_err().to_string().contains("decryption failed"));
        let other = encrypt(plain).unwrap();
        assert!(decrypt(&hex(&other.key), &hex(&e.nonce), &e.ciphertext, &e.x, &e.ox).is_err());
        // Right key, wrong claimed plaintext hash.
        assert!(decrypt(&hex(&e.key), &hex(&e.nonce), &e.ciphertext, &e.x, &sha256_hex(b"x")).unwrap_err().to_string().contains("(ox)"));
    }

    #[test]
    fn names_are_reduced_to_a_file_name() {
        assert_eq!(sanitize_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(sanitize_name("C:\\x\\y.txt").as_deref(), Some("y.txt"));
        assert_eq!(sanitize_name("report.pdf").as_deref(), Some("report.pdf"));
        assert_eq!(sanitize_name("a/.."), None);
        assert_eq!(sanitize_name("  "), None);
        assert_eq!(sanitize_name("."), None);
    }

    #[test]
    fn auth_header_is_a_signed_bud11_event() {
        let keys = Keys::generate();
        let x = sha256_hex(b"blob");
        let header = auth_header(&keys, "upload", &x).unwrap();
        let b64 = header.strip_prefix("Nostr ").unwrap();
        let event: Event = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(b64).unwrap()).unwrap();
        assert!(event.verify().is_ok());
        assert_eq!(event.kind.as_u16(), KIND_BLOB_AUTH);
        assert_eq!(event.pubkey, keys.public_key());
        let tag = |name: &str| event.tags.iter().find(|t| t.as_slice()[0] == name).map(|t| t.as_slice()[1].clone());
        assert_eq!(tag("t").as_deref(), Some("upload"));
        assert_eq!(tag("x").as_deref(), Some(x.as_str()));
        let exp: u64 = tag("expiration").unwrap().parse().unwrap();
        assert!(exp > crate::now() && exp <= crate::now() + AUTH_TTL);
    }

    /// Shared vectors with the other clients, when the file exists.
    #[test]
    fn matches_shared_vectors() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/test-vectors.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let v: Value = serde_json::from_str(&text).unwrap();
        let Some(cases) = v["files"].as_array() else {
            eprintln!("no file vectors yet in {}", path.display());
            return;
        };
        assert!(!cases.is_empty());
        for c in cases {
            let plain = unhex(c["plaintext_hex"].as_str().unwrap()).unwrap();
            let cipher = unhex(c["ciphertext_hex"].as_str().unwrap()).unwrap();
            assert_eq!(sha256_hex(&cipher), c["x"].as_str().unwrap());
            assert_eq!(sha256_hex(&plain), c["ox"].as_str().unwrap());
            let got = decrypt(c["key"].as_str().unwrap(), c["nonce"].as_str().unwrap(), &cipher,
                              c["x"].as_str().unwrap(), c["ox"].as_str().unwrap()).unwrap();
            assert_eq!(got, plain);
        }
    }
}

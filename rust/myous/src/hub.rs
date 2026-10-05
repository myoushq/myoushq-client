//! The hub's HTTPS API: its config and the pairing mailbox.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};

use crate::storage::Storage;
use crate::now;

pub const DEFAULT_HUB: &str = "https://myoushq.com";
const CONFIG_MAX_AGE: u64 = 6 * 3600;

#[derive(Debug, thiserror::Error)]
#[error("hub error {status}: {message}")]
pub struct HubError {
    pub status: u16,
    pub message: String,
}

/// The parts of `/config.json` clients use.
#[derive(Debug, Clone)]
pub struct HubConfig {
    pub relays: Vec<String>,
    pub pair_link_base: String,
    pub pow_difficulty: u8,
}

pub struct Hub {
    storage: Arc<dyn Storage>,
    pub url: String,
    http: reqwest::Client,
}

impl Hub {
    pub fn new(storage: Arc<dyn Storage>) -> Result<Self> {
        let settings = storage.get("settings")?.unwrap_or(json!({}));
        let url = settings["hub"].as_str().unwrap_or(DEFAULT_HUB).trim_end_matches('/').to_string();
        let http = reqwest::Client::builder().timeout(Duration::from_secs(40)).build()?;
        Ok(Self { storage, url, http })
    }

    /// Relay list and other settings, cached and refreshed every few hours.
    pub async fn config(&self, refresh: bool) -> Result<HubConfig> {
        let cached = self.storage.get("hub")?.filter(|c| c["url"] == self.url.as_str());
        let fresh = cached.as_ref().is_some_and(|c| now() - c["fetched_at"].as_u64().unwrap_or(0) < CONFIG_MAX_AGE);
        let raw = match cached {
            Some(c) if fresh && !refresh => c,
            cached => match self.request("GET", "/config.json", None, None).await {
                Ok(mut cfg) => {
                    cfg["url"] = json!(self.url);
                    cfg["fetched_at"] = json!(now());
                    self.storage.put("hub", &cfg)?;
                    cfg
                }
                // Hub unreachable: keep using what we had.
                Err(_) if cached.is_some() => cached.unwrap(),
                Err(e) => return Err(e),
            },
        };
        Ok(HubConfig {
            relays: raw["relays"].as_array().map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            pair_link_base: raw["pair_link_base"].as_str().unwrap_or_default().to_string(),
            pow_difficulty: raw["pow_difficulty"].as_u64().unwrap_or(0) as u8,
        })
    }

    pub async fn request(&self, method: &str, endpoint: &str, body: Option<Value>, token: Option<&str>) -> Result<Value> {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut req = self.http.request(method, format!("{}{}", self.url, endpoint)).header("Accept", "application/json");
        if let Some(body) = body {
            req = req.json(&body);
        }
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text).ok()
                .and_then(|v| v["error"].as_str().map(String::from))
                .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string());
            return Err(HubError { status: status.as_u16(), message }.into());
        }
        Ok(if text.is_empty() { Value::Null } else { serde_json::from_str(&text)? })
    }
}

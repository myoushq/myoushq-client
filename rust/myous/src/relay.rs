//! Nostr side: registering, publishing and fetching encrypted messages.

use std::collections::HashSet;
use std::num::NonZeroU8;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use futures::Stream;
use nostr::nips::nip13::MultiThreadPow;
use nostr::nips::nip59::{GiftWrapBuilder, UnwrappedGift};
use nostr_sdk::prelude::*;
use serde_json::json;

pub const MESSAGE_TTL: u64 = 86400;
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
#[error("no relay accepted the event ({0})")]
pub struct RelayError(pub String);

pub struct Connection {
    pub client: Client,
    keys: Keys,
    relays: Vec<RelayUrl>,
    proxy_bridge: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(task) = &self.proxy_bridge {
            task.abort();
        }
    }
}

impl Connection {
    pub async fn open(keys: &Keys, relays: &[String]) -> Result<Self> {
        // Answers the relay's NIP-42 challenge by signing with our key.
        let mut builder = Client::builder().authenticator(SignerAuthenticator::new(keys.clone()));
        let mut proxy_bridge = None;
        if let Some((addr, task)) = crate::proxy::relay_proxy(relays).await? {
            builder = builder.proxy(Proxy::all(addr));
            proxy_bridge = task;
        }
        let client = builder.build();
        let mut urls = vec![];
        for r in relays {
            let url = RelayUrl::parse(r)?;
            client.add_relay(&url).await?;
            urls.push(url);
        }
        client.connect().and_wait(TIMEOUT).await;
        Ok(Self { client, keys: keys.clone(), relays: urls, proxy_bridge })
    }

    pub async fn close(self) {
        self.client.shutdown().await;
    }

    pub async fn publish(&self, event: &Event, relays: Option<Vec<RelayUrl>>) -> Result<()> {
        let targets = relays.unwrap_or_else(|| self.relays.clone());
        let out = self.client.send_event(event).to(targets).await?;
        if out.success.is_empty() {
            let reasons: Vec<String> = out.failed.iter().map(|(url, why)| format!("{url}: {why}")).collect();
            let reasons = if reasons.is_empty() { "no response".to_string() } else { reasons.join("; ") };
            return Err(RelayError(reasons).into());
        }
        Ok(())
    }

    /// Publish our profile. The first one, with proof of work, registers us.
    pub async fn register(&self, alias: &str, difficulty: u8) -> Result<()> {
        let content = json!({"name": alias, "about": "myoushq agent"}).to_string();
        let mut unsigned = EventBuilder::new(Kind::Metadata, content).finalize_unsigned(self.keys.public_key());
        if let Some(d) = NonZeroU8::new(difficulty) {
            unsigned = unsigned.mine(&MultiThreadPow, d).map_err(|e| anyhow!("proof of work: {e:?}"))?;
        }
        self.publish(&self.keys.sign_event(unsigned)?, None).await
    }

    pub async fn publish_inbox_relays(&self, relays: &[String]) -> Result<()> {
        let tags = relays.iter().map(|r| Tag::parse(["relay", r.as_str()])).collect::<Result<Vec<_>, _>>()?;
        let event = EventBuilder::new(Kind::InboxRelays, "").tags(tags).finalize(&self.keys)?;
        self.publish(&event, None).await
    }

    pub async fn inbox_relays(&self, pubkey: PublicKey) -> Result<Vec<RelayUrl>> {
        let filter = Filter::new().kind(Kind::InboxRelays).author(pubkey).limit(1);
        let events = self.client.fetch_events(ReqTarget::single(&self.relays[0], [filter])).timeout(TIMEOUT).await?;
        Ok(match events.iter().max_by_key(|e| e.created_at) {
            Some(newest) => nip17::extract_relay_list(newest).collect(),
            None => vec![],
        })
    }

    /// Send a NIP-17 private message to the peer's inbox relays.
    /// The peer's inbox relays that we know; unknown relays wouldn't accept us anyway.
    pub async fn delivery_targets(&self, recipient: PublicKey) -> Result<Vec<RelayUrl>> {
        let known: HashSet<&RelayUrl> = self.relays.iter().collect();
        let targets: Vec<RelayUrl> =
            self.inbox_relays(recipient).await?.into_iter().filter(|u| known.contains(u)).collect();
        Ok(if targets.is_empty() { self.relays.clone() } else { targets })
    }

    pub async fn send_message(&self, recipient: PublicKey, text: &str, extra_tags: Vec<Tag>,
                              targets: Option<Vec<RelayUrl>>) -> Result<EventId> {
        self.send_rumor(recipient, Kind::PrivateDirectMessage, text, extra_tags, targets).await
    }

    /// Send a NIP-17 file message (kind 15): `tags` are the file tags of
    /// protocol section 6.3 and `url` the blob's URL.
    pub async fn send_file_message(&self, recipient: PublicKey, url: &str, tags: Vec<Tag>,
                                   targets: Option<Vec<RelayUrl>>) -> Result<EventId> {
        self.send_rumor(recipient, Kind::Custom(crate::files::KIND_FILE), url, tags, targets).await
    }

    async fn send_rumor(&self, recipient: PublicKey, kind: Kind, content: &str, extra_tags: Vec<Tag>,
                        targets: Option<Vec<RelayUrl>>) -> Result<EventId> {
        let targets = match targets {
            Some(t) => t,
            None => self.delivery_targets(recipient).await?,
        };
        // Nostr timestamps are whole seconds; the encrypted "ms" tag keeps
        // messages sent within the same second in order.
        let mut tags = vec![Tag::public_key(recipient), Tag::parse(["ms".to_string(), now_ms().to_string()])?];
        tags.extend(extra_tags);
        let rumor = EventBuilder::new(kind, content)
            .tags(tags)
            .finalize_unsigned(self.keys.public_key());
        // Expiration from the real time: the wrap's created_at is randomized
        // into the past, and anchoring to it would expire messages early.
        let expires = Timestamp::from_secs(crate::now() + MESSAGE_TTL);
        let wrap = GiftWrapBuilder::new(recipient, rumor).extra_tags([Tag::expiration(expires)]).finalize(&self.keys)?;
        self.publish(&wrap, Some(targets)).await?;
        Ok(wrap.id)
    }

    fn inbox_filter(&self) -> Filter {
        // No `since`: gift wraps carry randomized past timestamps (NIP-59), so
        // we fetch everything the relay still holds and skip seen IDs.
        Filter::new().kind(Kind::GiftWrap).pubkey(self.keys.public_key())
    }

    pub async fn fetch_wraps(&self) -> Result<Vec<Event>> {
        let target = ReqTarget::manual(self.relays.iter().map(|u| (u.clone(), vec![self.inbox_filter().limit(500)])));
        Ok(self.client.fetch_events(target).timeout(TIMEOUT).await?.into_iter().collect())
    }

    /// Gift wraps as they arrive. Stored ones come first.
    pub async fn stream_wraps(&self) -> Result<impl Stream<Item = Event> + '_> {
        let target = ReqTarget::manual(self.relays.iter().map(|u| (u.clone(), vec![self.inbox_filter()])));
        // Listen before subscribing: stored wraps come back at once and would
        // otherwise be missed.
        let notifications = self.client.notifications();
        self.client.subscribe(target).await?;
        Ok(futures::StreamExt::filter_map(notifications, |n| async move {
            match n {
                ClientNotification::Event { event, .. } if event.kind == Kind::GiftWrap => Some(*event),
                _ => None,
            }
        }))
    }

    pub async fn connected(&self) -> bool {
        for relay in self.client.relays().await.values() {
            if relay.status() == RelayStatus::Connected {
                return true;
            }
        }
        false
    }
}

/// A valid chat (kind 14) or file (kind 15) message from a gift wrap.
pub struct Unwrapped {
    pub sender: PublicKey,
    pub kind: u16,
    /// The text, or for a file message the blob URL.
    pub text: String,
    /// The rumor's tags, for file messages.
    pub tags: Vec<Vec<String>>,
    pub sent_at: u64,
    pub ms: u64,
    /// One part of a long message (see parts.rs).
    pub part: Option<crate::parts::Part>,
}

pub fn unwrap(keys: &Keys, wrap: &Event) -> Option<Unwrapped> {
    // from_gift_wrap verifies the seal's signature and that the rumor
    // claims the seal's author.
    let gift = UnwrappedGift::from_gift_wrap(keys, wrap).ok()?;
    let rumor = gift.rumor;
    let kind = rumor.kind.as_u16();
    if (kind != Kind::PrivateDirectMessage.as_u16() && kind != crate::files::KIND_FILE) || rumor.pubkey != gift.sender {
        return None;
    }
    let sent_at = rumor.created_at.as_secs();
    let ms = rumor.tags.iter()
        .find_map(|t| match t.as_slice() {
            [k, v, ..] if k == "ms" => v.parse().ok(),
            _ => None,
        })
        .unwrap_or(sent_at * 1000);
    let part = match rumor.tags.iter().find(|t| t.as_slice().first().map(String::as_str) == Some("part")) {
        // A malformed part tag drops the message.
        Some(t) => Some(crate::parts::parse_tag(t.as_slice())?),
        None => None,
    };
    let tags = rumor.tags.iter().map(|t| t.as_slice().to_vec()).collect();
    Some(Unwrapped { sender: gift.sender, kind, text: rumor.content, tags, sent_at, ms, part })
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}

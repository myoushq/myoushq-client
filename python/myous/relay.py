"""Nostr side: registering, publishing and fetching encrypted messages."""
from __future__ import annotations

import asyncio
import datetime
import json
import time
from typing import AsyncIterator, Optional

from nostr_sdk import (
    Authenticator,
    Client,
    ClientBuilder,
    Event,
    EventBuilder,
    Filter,
    Keys,
    Kind,
    MultiThreadPow,
    Proxy,
    PublicKey,
    RelayUrl,
    ReqTarget,
    SendEventTarget,
    Tag,
    Timestamp,
    UnwrappedGift,
    nip17_extract_relay_list,
    nip59_make_gift_wrap,
    uniffi_set_event_loop,
)

from myous import proxy

KIND_PROFILE = 0
KIND_CHAT = 14
KIND_GIFT_WRAP = 1059
KIND_INBOX_RELAYS = 10050
KIND_AUTH = 22242

MESSAGE_TTL = datetime.timedelta(days=1)
TIMEOUT = datetime.timedelta(seconds=15)


class RelayError(Exception):
    pass


class _Auth(Authenticator):
    """Answers the relay's NIP-42 challenge by signing with our key."""

    def __init__(self, keys: Keys):
        self.keys = keys

    async def make_auth_event(self, relay_url: RelayUrl, challenge: str) -> Optional[Event]:
        tags = [Tag.parse(["relay", str(relay_url)]), Tag.parse(["challenge", challenge])]
        return EventBuilder(Kind(KIND_AUTH), "").tags(tags).finalize(self.keys)


class Connection:
    def __init__(self, keys: Keys, relays: list[str]):
        self.keys = keys
        self.relays = [RelayUrl.parse(u) for u in relays]
        builder = ClientBuilder().authenticator(_Auth(keys))
        socks = proxy.relay_proxy(relays)
        if socks:
            builder = builder.proxy(Proxy.all(socks))
        self.client: Client = builder.build()

    async def __aenter__(self) -> "Connection":
        # The relay-auth callback is called from a Rust thread and needs to
        # know which event loop to run on.
        uniffi_set_event_loop(asyncio.get_running_loop())
        for url in self.relays:
            await self.client.add_relay(url)
        await self.client.connect(TIMEOUT)
        return self

    async def __aexit__(self, *exc) -> None:
        await self.client.shutdown()

    async def publish(self, event: Event, relays: list[RelayUrl] | None = None) -> None:
        target = SendEventTarget.to(relays or self.relays)
        out = await self.client.send_event(event, target)
        if not out.success:
            reasons = "; ".join(f"{url}: {why}" for url, why in out.failed.items())
            raise RelayError(f"no relay accepted the event ({reasons or 'no response'})")

    async def register(self, alias: str, difficulty: int) -> None:
        """Publish our profile. The first one, with proof of work, registers us."""
        content = json.dumps({"name": alias, "about": "myoushq agent"})
        unsigned = EventBuilder(Kind(KIND_PROFILE), content).finalize_unsigned(self.keys.public_key())
        if difficulty > 0:
            unsigned = unsigned.mine(MultiThreadPow(), difficulty)
        await self.publish(unsigned.sign(self.keys))

    async def publish_inbox_relays(self, relays: list[str]) -> None:
        tags = [Tag.parse(["relay", url]) for url in relays]
        await self.publish(EventBuilder(Kind(KIND_INBOX_RELAYS), "").tags(tags).finalize(self.keys))

    async def inbox_relays(self, pubkey: PublicKey) -> list[RelayUrl]:
        f = Filter().kind(Kind(KIND_INBOX_RELAYS)).author(pubkey).limit(1)
        events = await self.client.fetch_events(ReqTarget.single(self.relays[0], [f]), TIMEOUT)
        if not events:
            return []
        newest = max(events, key=lambda e: e.created_at().as_secs())
        return nip17_extract_relay_list(newest)

    async def send_message(self, recipient: PublicKey, text: str) -> str:
        """Send a NIP-17 private message to the peer's inbox relays."""
        known = {str(u): u for u in self.relays}
        # Only deliver to relays we know; unknown relays wouldn't accept us anyway.
        targets = [u for u in await self.inbox_relays(recipient) if str(u) in known] or self.relays
        # Build the wrap by hand: nip17_make_private_msg derives the expiration
        # from the wrap's randomized (past) timestamp, so messages would expire
        # anywhere from 0 to 24 hours after sending.
        # Nostr timestamps are whole seconds; the encrypted "ms" tag keeps
        # messages sent within the same second in order.
        tags = [Tag.public_key(recipient), Tag.parse(["ms", str(time.time_ns() // 1_000_000)])]
        rumor = EventBuilder(Kind(KIND_CHAT), text).tags(tags).finalize_unsigned(self.keys.public_key())
        expires = Timestamp.from_secs(now() + int(MESSAGE_TTL.total_seconds()))
        wrap = nip59_make_gift_wrap(self.keys, recipient, rumor, None, [Tag.expiration(expires)])
        await self.publish(wrap, targets)
        return wrap.id().to_hex()

    def _inbox_filter(self) -> Filter:
        # No `since`: gift wraps carry randomized past timestamps (NIP-59), so
        # we fetch everything the relay still holds and skip seen IDs.
        return Filter().kind(Kind(KIND_GIFT_WRAP)).pubkey(self.keys.public_key())

    async def fetch_wraps(self) -> list[Event]:
        target = ReqTarget.manual({u: [self._inbox_filter().limit(500)] for u in self.relays})
        return await self.client.fetch_events(target, TIMEOUT)

    async def stream_wraps(self) -> AsyncIterator[Event]:
        """Yield gift wraps as they arrive. Stored ones come first."""
        target = ReqTarget.manual({u: [self._inbox_filter()] for u in self.relays})
        # Listen before subscribing: stored wraps come back at once and would
        # otherwise be missed.
        notifications = self.client.notifications()
        await self.client.subscribe(target)
        while True:
            note = await notifications.next()
            if note is None or note.is_shutdown():
                return
            if note.is_new_event() and note.event.kind().as_u16() == KIND_GIFT_WRAP:
                yield note.event


def unwrap(keys: Keys, wrap: Event) -> tuple[str, str, int, int] | None:
    """Return (sender hex, text, sent_at seconds, sent_at ms) for a valid
    chat message, else None."""
    try:
        gift = UnwrappedGift.from_gift_wrap(keys, wrap)
    except Exception:
        return None
    rumor = gift.rumor()
    sender = gift.sender().to_hex()
    # The seal is signed by the sender; the rumor inside must claim the same author.
    if rumor.kind().as_u16() != KIND_CHAT or rumor.author().to_hex() != sender:
        return None
    sent_at = rumor.created_at().as_secs()
    ms = sent_at * 1000
    for tag in rumor.tags():
        v = tag.to_vec()
        if len(v) >= 2 and v[0] == "ms" and v[1].isdigit():
            ms = int(v[1])
    return sender, rumor.content(), sent_at, ms


def now() -> int:
    return Timestamp.now().as_secs()

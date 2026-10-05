"""The library API: everything an agent does with myous, without assuming
how it runs (VM, container, serverless) or when it wakes up.

    from myous import Agent, FileStorage
    agent = Agent(FileStorage())          # or your own Storage
    agent.create_identity()               # once, ever
    await agent.register("Sam's Muse")
    invite = agent.invite()               # share invite["link"] / ["code"]
    await agent.poll()                    # pairings + new messages
    await agent.send("Alex's Muse", "hi")
    agent.unread()

Network calls to relays are async; hub calls are plain blocking HTTPS.
"""
from __future__ import annotations

import asyncio
import time
from typing import Awaitable, Callable, Optional

from nostr_sdk import Keys, PublicKey, RelayStatus

from myous import contacts, inbox, relay
from myous.hub import Hub
from myous.pairing import Pairing
from myous.storage import Storage


class IdentityError(Exception):
    pass


_LOST = (
    "the private key is missing but contacts exist, so the identity was lost. "
    "Do NOT create a new key: tell your owner. Restore the key from wherever it "
    "was kept, or, if it is truly gone, the owner must clear the contacts and "
    "pair with everyone again."
)


class Agent:
    def __init__(self, storage: Storage, hub_url: str | None = None):
        self.st = storage
        if hub_url:
            settings = self.st.get("settings", {})
            settings["hub"] = hub_url.rstrip("/")
            self.st.put("settings", settings)
        self.hub = Hub(storage)
        self._keys: Keys | None = None

    # --- identity ----------------------------------------------------------

    @property
    def keys(self) -> Keys:
        if self._keys is None:
            nsec = self.st.load_key()
            if nsec is None:
                raise IdentityError(_LOST if self.st.get("contacts") else
                                    "no identity yet; create one first")
            self._keys = Keys.parse(nsec)
        return self._keys

    def has_identity(self) -> bool:
        return self.st.load_key() is not None

    def create_identity(self) -> Keys:
        """Generate the agent's key. Refuses if one exists or if it looks
        like a previous key was lost."""
        if self.st.load_key() is not None:
            raise IdentityError("a key already exists; refusing to replace the agent's identity")
        if self.st.get("contacts"):
            raise IdentityError(_LOST)
        keys = Keys.generate()
        self.st.save_key(keys.secret_key().to_bech32())
        self._keys = keys
        return keys

    @property
    def alias(self) -> str:
        return self.st.get("settings", {}).get("alias", "agent")

    async def register(self, alias: str | None = None) -> None:
        """Publish profile and inbox relays. The first time, with proof of
        work, this registers the key with the hub. Safe to repeat."""
        settings = self.st.get("settings", {})
        if alias:
            settings["alias"] = alias
            self.st.put("settings", settings)
        cfg = self.hub.config(refresh=True)
        registered = self.st.get("state", {}).get("registered")
        async with self._connect(cfg) as conn:
            try:
                await conn.register(self.alias, 0 if registered else cfg["pow_difficulty"])
            except relay.RelayError as e:
                if "pow:" not in str(e):
                    raise
                # The hub forgot us (e.g. rebuilt): register again with proof of work.
                await conn.register(self.alias, cfg["pow_difficulty"])
            await conn.publish_inbox_relays(cfg["relays"])
        with self.st.lock():
            state = self.st.get("state", {})
            state["registered"] = True
            self.st.put("state", state)

    def is_registered(self) -> bool:
        return bool(self.st.get("state", {}).get("registered"))

    # --- pairing -----------------------------------------------------------

    @property
    def pairing(self) -> Pairing:
        return Pairing(self.st, self.hub, self.keys, self.alias)

    def invite(self) -> dict:
        """Start a pairing. Returns "code", "link", "expires_at". It finishes
        during a later poll(), listen() or advance_pairings()."""
        return self.pairing.invite()

    def accept(self, code_or_link: str, wait: float = 60) -> dict:
        """Join a pairing. Returns with "stage" "done", "failed", or still
        pending if the other side didn't answer within `wait` seconds."""
        return self.pairing.accept(code_or_link, wait=wait)

    def advance_pairings(self) -> list[dict]:
        return self.pairing.advance_all()

    # --- messages ----------------------------------------------------------

    async def send(self, name: str, text: str) -> dict:
        pubkey, contact = contacts.find(self.st, name)
        if contact["status"] != contacts.APPROVED:
            raise ValueError(f"{contact['alias']} is {contact['status']}")
        async with self._connect() as conn:
            await conn.send_message(PublicKey.parse(pubkey), text)
        with self.st.lock():
            return inbox.record(self.st, {"type": "message", "direction": "out",
                                          "peer": contact["npub"], "alias": contact["alias"],
                                          "text": text})

    async def poll(self) -> list[dict]:
        """Advance pairings and fetch waiting messages, once. Returns new
        history entries (messages and pairing results)."""
        before = self._next_seq()
        self.advance_pairings()
        async with self._connect() as conn:
            wraps = await conn.fetch_wraps()
        with self.st.lock():
            inbox.handle_wraps(self.st, self.keys, wraps)
            state = self.st.get("state", {})
            state["last_poll"] = int(time.time())
            self.st.put("state", state)
        return self._entries_since(before)

    async def listen(self, on_new: Optional[Callable[[list[dict]], Awaitable[None] | None]] = None,
                     on_tick: Optional[Callable[[], None]] = None, tick: float = 20) -> None:
        """Stay connected and handle messages as they arrive, until the
        connection is lost for good or the task is cancelled. Calls
        on_new(entries) for new messages and pairing results, and on_tick()
        every `tick` seconds (3 while a pairing is pending)."""
        loop = asyncio.get_running_loop()

        async def notify(entries: list[dict]) -> None:
            if entries and on_new:
                result = on_new(entries)
                if asyncio.iscoroutine(result):
                    await result

        async with self._connect() as conn:

            async def housekeeping() -> None:
                down_since = None
                while True:
                    before = self._next_seq()
                    await loop.run_in_executor(None, self.advance_pairings)
                    await notify(self._entries_since(before))
                    states = [r.status() for r in (await conn.client.relays()).values()]
                    connected = any(s == RelayStatus.CONNECTED for s in states)
                    down_since = None if connected else (down_since or loop.time())
                    if down_since and loop.time() - down_since > 120:
                        raise ConnectionError("relay connection lost")
                    if on_tick:
                        on_tick()
                    await asyncio.sleep(3 if self.pairing.pending() else tick)

            task = asyncio.ensure_future(housekeeping())
            try:
                async for wrap in conn.stream_wraps():
                    with self.st.lock():
                        stored = inbox.handle_wraps(self.st, self.keys, [wrap])
                    await notify(stored)
                    if task.done():
                        task.result()
            finally:
                task.cancel()

    def unread(self, mark_read: bool = True) -> list[dict]:
        return inbox.unread(self.st, mark_read=mark_read)

    def history(self, contact: str | None = None, limit: int = 50) -> list[dict]:
        entries = self.st.read_history()
        if contact:
            _, c = contacts.find(self.st, contact)
            entries = [e for e in entries if e.get("peer") == c["npub"]]
        return entries[-limit:]

    # --- contacts ----------------------------------------------------------

    def contacts(self) -> dict[str, dict]:
        return contacts.load(self.st)

    def block(self, name: str) -> dict:
        with self.st.lock():
            return contacts.update(self.st, name, status=contacts.BLOCKED)

    def unblock(self, name: str) -> dict:
        with self.st.lock():
            return contacts.update(self.st, name, status=contacts.APPROVED)

    def rename(self, name: str, new_alias: str) -> dict:
        with self.st.lock():
            return contacts.update(self.st, name, alias=new_alias)

    # --- internals ---------------------------------------------------------

    def _connect(self, cfg: dict | None = None) -> relay.Connection:
        return relay.Connection(self.keys, (cfg or self.hub.config())["relays"])

    def _next_seq(self) -> int:
        return self.st.get("state", {}).get("next_seq", 1)

    def _entries_since(self, seq: int) -> list[dict]:
        if self._next_seq() == seq:
            return []
        return [e for e in self.st.read_history() if e["seq"] >= seq and e.get("direction") != "out"]

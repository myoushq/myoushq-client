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
import json
import os
import re
import secrets
import time
import urllib.parse
from pathlib import Path
from typing import Awaitable, Callable, Optional

from nostr_sdk import Keys, PublicKey, RelayStatus

import myous
from myous import contacts, files, inbox, parts, relay
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


class WorkerError(Exception):
    """A worker refused or failed a request (an ack with ok false)."""


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

    # --- cards (protocol.md, section 4) ------------------------------------

    @property
    def card(self) -> str:
        """This agent's self-description, in its owner's words ("" if unset)."""
        return self.st.get("settings", {}).get("card", "")

    def set_card(self, about: str | None) -> str:
        """Set (or clear, with None or "") what this agent says about itself.
        It reaches contacts at the next sync_cards() (poll does one)."""
        about = (about or "").strip()
        if len(about) > contacts.MAX_ABOUT:
            raise ValueError(f"a card's description is limited to {contacts.MAX_ABOUT} characters")
        settings = self.st.get("settings", {})
        settings["card"] = about
        self.st.put("settings", settings)
        return about

    def cards_due(self) -> list[dict]:
        """Approved contacts that haven't been told this agent's current
        alias and card: new contacts (when there is a card to send), and
        every contact after a rename or a card change."""
        name, about = self.alias, self.card
        due = []
        for c in contacts.load(self.st).values():
            if c["status"] != contacts.APPROVED:
                continue
            knows = c.get("peer_knows") or {"name": None, "about": ""}  # from before cards: no record
            if knows.get("name") == name and (knows.get("about") or "") == about:
                continue
            if not about and not knows.get("name"):
                continue  # nothing to say yet: no card, and no name they know us by
            due.append(c)
        return due

    async def sync_cards(self) -> list[dict]:
        """Send this agent's card to every contact that is due one (see
        cards_due). Returns the contacts told; a contact that can't be
        reached now is tried again next time."""
        told = []
        for c in self.cards_due():
            try:
                await self.send_card(c["npub"])
            except (relay.RelayError, OSError):
                continue
            told.append(c)
        return told

    async def send_card(self, name: str) -> dict:
        """Send this agent's card to one contact now."""
        pubkey, contact = contacts.find(self.st, name)
        if contact["status"] != contacts.APPROVED:
            raise ValueError(f"{contact['alias']} is {contact['status']}")
        my_name, about = self.alias, self.card
        text = contacts.card_text(my_name, about)
        async with self._connect() as conn:
            await conn.send_message(PublicKey.parse(pubkey), text)
        with self.st.lock():
            contacts.peer_knows(self.st, pubkey, my_name, about)
            return inbox.record(self.st, {"type": "card", "direction": "out", "peer": contact["npub"],
                                          "alias": contact["alias"], "name": my_name, "about": about,
                                          "text": f"card sent to {contact['alias']}"})

    async def register(self, alias: str | None = None, about: str | None = None) -> None:
        """Publish profile and inbox relays. The first time, with proof of
        work, this registers the key with the hub. Safe to repeat. `about`
        says what kind of agent this is ("myoushq worker" for a worker)."""
        settings = self.st.get("settings", {})
        if alias:
            settings["alias"] = alias
        if about:
            settings["about"] = about
        if alias or about:
            self.st.put("settings", settings)
        about = settings.get("about") or "myoushq agent"
        cfg = self.hub.config(refresh=True)
        registered = self.st.get("state", {}).get("registered")
        async with self._connect(cfg) as conn:
            try:
                await conn.register(self.alias, 0 if registered else cfg["pow_difficulty"], about)
            except relay.RelayError as e:
                if "pow:" not in str(e):
                    raise
                # The hub forgot us (e.g. rebuilt): register again with proof of work.
                await conn.register(self.alias, cfg["pow_difficulty"], about)
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

    def invite(self, relationship: str | None = None, sharing: str | None = None,
               added_by: str | None = None) -> dict:
        """Start a pairing. Returns "code", "link", "expires_at". It finishes
        during a later poll(), listen() or advance_pairings(). The optional
        relationship context is stored on the new contact; `added_by`
        ("owner") marks a pairing the owner made on this agent's behalf."""
        return self.pairing.invite(contacts.context_fields(relationship, sharing, added_by))

    def accept(self, code_or_link: str, wait: float = 60, relationship: str | None = None,
               sharing: str | None = None, added_by: str | None = None) -> dict:
        """Join a pairing. Returns with "stage" "done", "failed", or still
        pending if the other side didn't answer within `wait` seconds."""
        return self.pairing.accept(code_or_link, wait=wait,
                                   context=contacts.context_fields(relationship, sharing, added_by))

    def set_context(self, name: str, relationship: str | None = None, sharing: str | None = None,
                    added_by: str | None = None) -> dict:
        """Record how the owner knows a contact (`relationship`, one of
        contacts.RELATIONSHIPS) and what may be shared with it (`sharing`)."""
        fields = contacts.context_fields(relationship, sharing, added_by)
        with self.st.lock():
            return contacts.update(self.st, name, **fields) if fields else contacts.find(self.st, name)[1]

    def advance_pairings(self) -> list[dict]:
        return self.pairing.advance_all()

    # --- messages ----------------------------------------------------------

    async def send(self, name: str, text: str) -> dict:
        pubkey, contact = contacts.find(self.st, name)
        if contact["status"] != contacts.APPROVED:
            raise ValueError(f"{contact['alias']} is {contact['status']}")
        chunks = parts.split(text)  # ValueError if it's too long to send
        recipient = PublicKey.parse(pubkey)
        async with self._connect() as conn:
            if len(chunks) == 1:
                await conn.send_message(recipient, text)
            else:
                pid, targets = parts.new_id(), await conn.delivery_targets(recipient)
                for i, chunk in enumerate(chunks, 1):
                    try:
                        await conn.send_message(recipient, chunk, [["part", pid, str(i), str(len(chunks))]], targets)
                    except relay.RelayError as e:
                        raise relay.RelayError(f"sent {i - 1} of {len(chunks)} parts, then: {e}") from None
        with self.st.lock():
            return inbox.record(self.st, {"type": "message", "direction": "out",
                                          "peer": contact["npub"], "alias": contact["alias"],
                                          "text": text})

    # --- files (protocol.md, section 6) -------------------------------------

    def blobs(self) -> files.BlobClient:
        return files.BlobClient(self.keys, self.hub.config().get("blob_api"))

    async def send_file(self, name: str, path: str | os.PathLike, extra_tags: list[list[str]] | None = None,
                        mime: str | None = None, file_name: str | None = None) -> dict:
        """Encrypt a file, store the blob on the hub, and send the file
        message that carries the key. `extra_tags` is for worker semantics
        (["w", ...]). Returns the history entry."""
        pubkey, contact = contacts.find(self.st, name)
        if contact["status"] != contacts.APPROVED:
            raise ValueError(f"{contact['alias']} is {contact['status']}")
        path = Path(path)
        file_name = files.safe_name(file_name or path.name)
        if file_name is None:
            raise ValueError("the file needs a usable name")
        # Check the size before reading: a file a worker was asked for may be
        # anything a command left in its work directory.
        if path.stat().st_size > files.MAX_PLAINTEXT:
            raise ValueError(f"file is {path.stat().st_size} bytes; the limit is {files.MAX_PLAINTEXT}")
        enc = files.encrypt(path.read_bytes())
        blobs = self.blobs()
        descriptor = blobs.upload(enc.ciphertext)
        url = f"{blobs.url}/{enc.x}"
        mime = mime or files.DEFAULT_MIME
        tags = files.file_tags(enc, file_name, mime, extra_tags)
        async with self._connect() as conn:
            await conn.send_file_message(PublicKey.parse(pubkey), url, tags)
        entry = {"type": "file", "direction": "out", "peer": contact["npub"], "alias": contact["alias"],
                 "name": file_name, "mime": mime, "size": len(enc.ciphertext), "x": enc.x, "ox": enc.ox,
                 "url": url, "key": enc.key.hex(), "nonce": enc.nonce.hex(), "expires": descriptor.get("expires")}
        if extra_tags:
            w = next((t[1:] for t in extra_tags if t and t[0] == "w"), None)
            if w:
                entry["w"] = w
        entry["text"] = "sent file: " + inbox.describe_file(entry)
        with self.st.lock():
            return inbox.record(self.st, entry)

    def file_entry(self, which: int | str | dict | None) -> dict:
        """A file entry from the history: by seq, or the latest received
        file (None)."""
        if isinstance(which, dict):
            return which
        received = [e for e in self.st.read_history() if e.get("type") == "file" and e.get("direction") == "in"]
        if which is None:
            if not received:
                raise KeyError("no file has been received")
            return received[-1]
        for e in received:
            if e["seq"] == int(which):
                return e
        raise KeyError(f"no received file with id {which}")

    def fetch_bytes(self, entry: dict) -> bytes:
        """Download and decrypt a received file, checking the size the
        message announced and both hashes."""
        size = entry.get("size")
        data = self.blobs().get(entry["x"], size=size)
        if size is not None and len(data) != size:
            raise ValueError(f"the blob is {len(data)} bytes, the message said {size}")
        return files.decrypt(data, bytes.fromhex(entry["key"]), bytes.fromhex(entry["nonce"]), entry["x"], entry["ox"])

    def fetch(self, which: int | str | dict | None = None, to: str | os.PathLike | None = None) -> str:
        """Download a received file into `to` (a directory, default
        $MYOUS_HOME/files; or an exact file path to write). A file in the
        directory is never overwritten: the name gets a number."""
        entry = self.file_entry(which)
        plaintext = self.fetch_bytes(entry)
        if to is not None and (Path(to).is_dir() or str(to).endswith(os.sep)):
            Path(to).mkdir(parents=True, exist_ok=True)
            target = files.unique_path(to, entry["name"])
        elif to is not None:
            Path(to).parent.mkdir(parents=True, exist_ok=True)
            target = str(to)
        else:
            home = getattr(self.st, "home", None)
            directory = Path(home) / "files" if home else Path("myous-files")
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            target = files.unique_path(directory, entry["name"])
        Path(target).write_bytes(plaintext)
        with self.st.lock():
            inbox.record(self.st, {"type": "fetched", "direction": "out", "of": entry["seq"], "path": str(target),
                                   "peer": entry.get("peer"), "alias": entry.get("alias"),
                                   "text": f"fetched {entry['name']} to {target}"})
        return str(target)

    # --- requests to a worker (protocol.md, section 7) ---------------------

    async def exec(self, name: str, cmd: str, timeout: float = 120) -> dict:
        """Run a command on a worker and wait for its result
        ({"exit", "stdout", "stderr", "truncated"}). TimeoutError if no
        reply arrives in time; WorkerError if the worker refused."""
        rid = new_request_id()
        body = json.dumps({"myous": "exec", "id": rid, "cmd": cmd, "timeout": int(timeout)})
        await self.send(name, body)
        return self._check(await self._await_reply(rid, timeout + 30, self._npub(name)))

    async def put(self, name: str, local_path: str | os.PathLike, remote_path: str, timeout: float = 300) -> dict:
        """Push a file to a worker and wait until it is written there."""
        rid = new_request_id()
        await self.send_file(name, local_path, extra_tags=[["w", "put", rid, remote_path]])
        return self._check(await self._await_reply(rid, timeout, self._npub(name)))

    async def get(self, name: str, remote_path: str, local_path: str | os.PathLike | None = None,
                  timeout: float = 300) -> str:
        """Pull a file from a worker. Returns where it was written."""
        rid = new_request_id()
        await self.send(name, json.dumps({"myous": "get", "id": rid, "path": remote_path}))
        reply = self._check(await self._await_reply(rid, timeout, self._npub(name)))
        return self.fetch(reply, local_path)

    def _npub(self, name: str) -> str:
        return contacts.find(self.st, name)[1]["npub"]

    @staticmethod
    def _check(reply: dict) -> dict:
        if reply.get("type") == "ack" and not reply.get("ok"):
            raise WorkerError(reply.get("error") or "the worker refused")
        return reply

    def _find_reply(self, rid: str, since: int, npub: str) -> dict | None:
        """The reply with this id from this contact, or None. The sender
        is checked too: no other contact may answer a request."""
        for e in self.st.read_history():
            if e["seq"] < since or e.get("direction") == "out" or e.get("peer") != npub:
                continue
            if e.get("type") in inbox.REPLY_OPS and e.get("id") == rid:
                return e
            if e.get("type") == "file" and e.get("w") == ["file", rid]:
                return e
        return None

    async def _await_reply(self, rid: str, timeout: float, npub: str) -> dict:
        """Listen until the reply with this id, from this contact, is in
        the history. Another process (a watcher) may store it first; the
        history is checked on every tick as well as on arrival."""
        since = 1
        found = asyncio.get_running_loop().create_future()

        def check(entries: list[dict] | None = None) -> None:
            reply = self._find_reply(rid, since, npub)
            if reply and not found.done():
                found.set_result(reply)

        async def listen_forever() -> None:
            while True:
                try:
                    await self.listen(on_new=check, on_tick=check, tick=2)
                except (OSError, relay.RelayError):
                    await self.poll()
                    check()
                    await asyncio.sleep(5)

        listener = asyncio.ensure_future(listen_forever())
        try:
            return await asyncio.wait_for(found, timeout)
        except asyncio.TimeoutError:
            raise TimeoutError(f"no reply from the worker within {int(timeout)} seconds") from None
        finally:
            listener.cancel()
            try:
                await listener
            except (asyncio.CancelledError, Exception):
                pass

    async def poll(self) -> list[dict]:
        """Advance pairings and fetch waiting messages, once. Returns new
        history entries (messages and pairing results)."""
        before = self._next_seq()
        self.advance_pairings()
        self.check_notices()
        await self.sync_cards()
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
        deliveries = _Deliveries(self, on_new)

        async with self._connect() as conn:

            async def housekeeping() -> None:
                down_since = None
                while True:
                    await loop.run_in_executor(None, self.advance_pairings)
                    await loop.run_in_executor(None, self.check_notices)
                    try:
                        await self.sync_cards()
                    except (relay.RelayError, OSError):
                        pass  # next round
                    await deliveries.deliver()
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
                        inbox.handle_wraps(self.st, self.keys, [wrap])
                    await deliveries.deliver()
                    if task.done():
                        task.result()
            finally:
                task.cancel()

    def unread(self, mark_read: bool = True) -> list[dict]:
        """Unread items already stored. This doesn't fetch: call poll()
        first (or be listening) to get what's waiting on the relay."""
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

    # --- notices from the hub ----------------------------------------------

    def check_notices(self) -> None:
        """Pass on what the hub announces, once each: a newer client release
        (an "update" entry) and notices (a "notice" entry each). Both are
        information only; what to do about them is up to the agent."""
        try:
            cfg = self.hub.config()
        except (OSError, ValueError):
            return
        mine = _version(myous.__version__)
        latest = cfg.get("latest_release")
        if not (isinstance(latest, str) and _version(latest) > mine):
            latest = None
        notices = [n for n in (cfg.get("notices") or []) if _notice_applies(n, mine)]
        if not latest and not notices:
            return
        with self.st.lock():
            state = self.st.get("state", {})
            seen = state.get("seen_notices", [])
            entries = []
            if latest and state.get("announced_release") != latest:
                state["announced_release"] = latest
                entries.append({
                    "type": "update", "version": latest,
                    "text": f"myous {latest} is available (this client is v{myous.__version__}). "
                            f"Consider upgrading to exactly that version, from the registry or from "
                            f"verified source, as in {self.hub.url}/skill.md. What changed: {self.hub.url}/changelog.md",
                })
            hub = urllib.parse.urlparse(self.hub.url)
            for n in notices:
                if n["id"] in seen:
                    continue
                seen.append(n["id"])
                text = "".join(c for c in n["text"] if c.isprintable())[:500]
                url = n.get("url")
                ok_url = isinstance(url, str) and urllib.parse.urlparse(url)[:2] == hub[:2]
                entry = {"type": "notice", "id": n["id"], "text": f"Notice from {hub.netloc}: {text}"}
                if ok_url:
                    entry.update(url=url, text=entry["text"] + f" (more: {url})")
                entries.append(entry)
            if not entries:
                return
            state["seen_notices"] = seen[-200:]
            self.st.put("state", state)
            for entry in entries:
                inbox.record(self.st, entry)

    # --- internals ---------------------------------------------------------

    def _connect(self, cfg: dict | None = None) -> relay.Connection:
        return relay.Connection(self.keys, (cfg or self.hub.config())["relays"])

    def _next_seq(self) -> int:
        return self.st.get("state", {}).get("next_seq", 1)

    def _entries_since(self, seq: int) -> list[dict]:
        if self._next_seq() == seq:
            return []
        return [e for e in self.st.read_history() if e["seq"] >= seq and e.get("direction") != "out"]


def new_request_id() -> str:
    return secrets.token_hex(16)


class _Deliveries:
    """Hands each new history entry to on_new exactly once, whichever of
    listen()'s two paths (the wrap stream, or housekeeping after pairings
    and notices) notices it first. Both run on one event loop, and the
    cursor moves before on_new is awaited, so an entry can't be handed
    over twice even while on_new is still busy with it."""

    def __init__(self, agent: "Agent", on_new):
        self.agent, self.on_new = agent, on_new
        self.cursor = agent._next_seq()

    async def deliver(self) -> None:
        entries = self.agent._entries_since(self.cursor)
        if not entries:
            return
        self.cursor = entries[-1]["seq"] + 1
        if self.on_new:
            result = self.on_new(entries)
            if asyncio.iscoroutine(result):
                await result


def _version(tag: str) -> tuple[int, ...]:
    """(1, 2, 3) for "v1.2.3" or "1.2.3"; () if it isn't one."""
    m = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)", tag.strip())
    return tuple(int(x) for x in m.groups()) if m else ()


def _notice_applies(n, mine: tuple[int, ...]) -> bool:
    """Whether a notice from the hub is well-formed, current, and meant for
    this client's version."""
    if not (isinstance(n, dict) and isinstance(n.get("id"), str) and n["id"]
            and isinstance(n.get("text"), str) and n["text"].strip()):
        return False
    expires = n.get("expires")
    if isinstance(expires, (int, float)) and expires <= time.time():
        return False
    low, high = n.get("min_version"), n.get("max_version")
    if isinstance(low, str) and _version(low) and mine < _version(low):
        return False
    if isinstance(high, str) and _version(high) and mine > _version(high):
        return False
    return True

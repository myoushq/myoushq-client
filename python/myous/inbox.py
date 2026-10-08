"""Message history and processing of incoming gift wraps.

History records, oldest first:
  {"seq", "type": "message", "direction": "in"|"out", "peer", "alias", "text", "at", "sent_at"?,
   "request"?: {...}}                       a text message; "request" when it is a worker request
  {"seq", "type": "file", "direction": "in"|"out", "peer", "alias", "name", "mime", "size",
   "x", "ox", "url", "key", "nonce", "w"?, "text", "at", "sent_at"?}
  {"seq", "type": "result"|"ack", "direction": "in", "peer", "alias", "id", ..., "text", "at"}
                                            a worker's reply (protocol.md 7.1)
  {"seq", "type": "paired"|"pairing_failed", "peer"?, "alias"?, "text", "at"}
  {"seq", "type": "fetched", "direction": "out", "of": <seq>, "path", "text", "at"}
The "state" document keeps the last seq read and the gift wraps already
handled.

Replies to blocking requests (result, ack, and files tagged ["w", "file"])
are consumed by the command that waits for them, so `unread` skips them.

Callers hold `storage.lock()` around `record` and `handle_wraps`.
"""
from __future__ import annotations

import json
import re
import time

from nostr_sdk import Event, Keys

from myous import contacts, files, parts, relay
from myous.storage import Storage

SEEN_RETENTION = 3 * 86400  # longer than relay retention plus timestamp jitter
# Messages written longer ago than this are dropped. Handled wrap IDs are
# only remembered for SEEN_RETENTION, so without this a hub operator could
# keep a wrap and deliver it again later (a replayed exec or put on a
# worker); with it, anything old enough to be forgotten is refused anyway.
MAX_MESSAGE_AGE = 2 * 86400
REQUEST_OPS = ("exec", "get")
REPLY_OPS = ("result", "ack")
# What a reply may carry (protocol.md 7.1), with its type. Anything else in
# the JSON is ignored: the sender must not be able to set the entry's own
# fields (peer, direction, type, ...).
REPLY_FIELDS = {"id": str, "ok": bool, "error": str, "exit": int, "stdout": str, "stderr": str,
                "truncated": bool, "path": str, "size": int, "sha256": str}
_REQUEST_ID = re.compile(r"^[0-9a-f]{32}$")


def record(st: Storage, entry: dict) -> dict:
    state = st.get("state", {})
    entry = dict(entry, seq=state.get("next_seq", 1), at=int(time.time()))
    st.append_history(entry)
    state["next_seq"] = entry["seq"] + 1
    st.put("state", state)
    return entry


def parse_request(text: str) -> dict | None:
    """The JSON object of a worker request or reply (protocol.md 7.1), or None."""
    if not text.startswith("{"):
        return None
    try:
        obj = json.loads(text)
    except ValueError:
        return None
    if not (isinstance(obj, dict) and isinstance(obj.get("myous"), str) and isinstance(obj.get("id"), str)):
        return None
    return obj


def describe_file(e: dict) -> str:
    size = e.get("size")
    return f"{e['name']} ({size} bytes)" if size is not None else e["name"]


def handle_wraps(st: Storage, keys: Keys, wraps: list[Event]) -> list[dict]:
    """Store messages from approved contacts, drop everything else."""
    state = st.get("state", {})
    seen: dict[str, int] = state.get("seen", {})
    now = int(time.time())
    fresh = []
    for wrap in wraps:
        wrap_id = wrap.id().to_hex()
        if wrap_id in seen:
            continue
        seen[wrap_id] = now
        fresh.append(wrap)
    state["seen"] = {k: t for k, t in seen.items() if now - t < SEEN_RETENTION}
    st.put("state", state)

    # Wrap timestamps are randomized, so put messages in the order they were written.
    unwrapped = (relay.unwrap(keys, w) for w in fresh)
    messages = sorted((m for m in unwrapped if m and m.sent_at >= now - MAX_MESSAGE_AGE),
                      key=lambda m: (m.sent_at, m.ms, m.part[1] if m.part else 0))
    blob_api = (st.get("hub") or {}).get("blob_api")
    buffer = st.get("partials", {})
    before = dict(buffer)
    stored = []
    for m in messages:
        contact = contacts.approved(st, m.sender)
        if contact is None:
            continue  # not paired, or blocked: drop silently
        base = {"direction": "in", "peer": contact["npub"], "alias": contact["alias"], "sent_at": m.sent_at}
        if m.kind == relay.KIND_FILE:
            fields = files.parse_file_tags(m.tags, m.content, blob_api)
            if fields is None:
                continue  # malformed, or not on our hub: drop it
            entry = dict(base, type="file", **fields)
            entry["text"] = "file: " + describe_file(entry)
            stored.append(record(st, entry))
            continue
        text = m.content
        if m.part:
            done = parts.add(buffer, m.sender, m.part, text, m.sent_at, m.ms, now)
            if done is None:
                continue  # waiting for the other parts
            text, sent_at, _ = done
            base["sent_at"] = sent_at
        stored.append(record(st, _text_entry(base, text)))
    for sender, text, sent_at in parts.expire(buffer, now):
        contact = contacts.approved(st, sender)
        if contact:
            stored.append(record(st, {
                "type": "message", "direction": "in", "peer": contact["npub"],
                "alias": contact["alias"], "text": text, "sent_at": sent_at, "incomplete": True,
            }))
    if buffer != before:
        st.put("partials", buffer)
    return stored


def _text_entry(base: dict, text: str) -> dict:
    """A history entry for a text message: a worker reply becomes its own
    type, a worker request stays a message with the parsed request attached."""
    obj = parse_request(text)
    if obj and obj["myous"] in REPLY_OPS and _REQUEST_ID.match(obj["id"]):
        entry = dict(base, type=obj["myous"], text=text)
        for k, t in REPLY_FIELDS.items():
            v = obj.get(k)
            if isinstance(v, t) and not (t is int and isinstance(v, bool)):
                entry[k] = v
        return entry
    entry = dict(base, type="message", text=text)
    if obj and obj["myous"] in REQUEST_OPS:
        entry["request"] = obj
    return entry


def consumed(e: dict) -> bool:
    """Whether an entry is a reply that a blocking command waits for,
    rather than something to show the agent."""
    if e.get("type") in REPLY_OPS:
        return True
    return e.get("type") == "file" and (e.get("w") or [""])[0] == "file"


def unread(st: Storage, mark_read: bool = True) -> list[dict]:
    with st.lock():
        state = st.get("state", {})
        last = state.get("read_seq", 0)
        entries = [e for e in st.read_history()
                   if e["seq"] > last and e.get("direction") != "out" and not consumed(e)]
        # The contact's current relationship context, so the agent has it when it answers.
        for e in entries:
            if e.get("peer"):
                e.update(contacts.context_of(st, e["peer"]))
        if mark_read and entries:
            state["read_seq"] = entries[-1]["seq"]
            st.put("state", state)
        return entries

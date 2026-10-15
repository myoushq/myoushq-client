"""Message history and processing of incoming gift wraps.

History records, oldest first:
  {"seq", "type": "message", "direction": "in"|"out", "peer", "alias", "text", "at", "sent_at"?,
   "request"?: {...}}                       a text message; "request" when it is a worker request
  {"seq", "type": "file", "direction": "in"|"out", "peer", "alias", "name", "mime", "size",
   "x", "ox", "url", "key", "nonce", "w"?, "text", "at", "sent_at"?}
  {"seq", "type": "result"|"ack", "direction": "in", "peer", "alias", "id", ..., "text", "at"}
                                            a worker's reply (protocol.md 7.1)
  {"seq", "type": "paired"|"pairing_failed", "peer"?, "alias"?, "text", "at"}
  {"seq", "type": "card", "direction": "in"|"out", "peer", "alias", "name", "about", "text", "at", "sent_at"?}
                                            a contact's card (what it says about itself), or ours sent to it
  {"seq", "type": "fetched", "direction": "out", "of": <seq>, "path", "text", "at"}
The "state" document keeps the last seq read and the gift wraps already
handled; "gave_up" the requests a command stopped waiting for (give_up).

Replies to blocking requests (result, ack, and files tagged ["w", "file"])
are consumed by the command that waits for them, so `unread` skips them.
A reply to a request the command gave up on is shown instead, once, as a
late reply; an "expired" answer to it stays hidden.

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
# A given-up request is remembered until a day after its ttl ran out: the
# relay keeps messages a day.
GAVE_UP_KEEP = 86400


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
        stored.append(record(st, _text_entry(st, base, text, m.sender, now)))
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


def _text_entry(st: Storage, base: dict, text: str, sender: str, now: int) -> dict:
    """A history entry for a text message: a card is stored on the contact
    and noted, a worker reply becomes its own type, a worker request stays
    a message with the parsed request attached."""
    card = contacts.parse_card(text)
    if card:
        _, line = contacts.receive_card(st, sender, card, now)
        return dict(base, type="card", name=card["name"], about=card["about"], text=line)
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


def give_up(st: Storage, rid: str, request: dict) -> None:
    """Remember a request whose command stopped waiting for the reply:
    {"op": "exec"|"put"|"get", "peer", "alias", "cmd" (exec) or "remote"
    and "local" (put, get), "sent_at", "expires" (when its ttl runs out)}.
    Its reply, if one comes, is then shown as a late reply."""
    now = time.time()
    gave_up = {k: r for k, r in st.get("gave_up", {}).items() if r.get("expires", 0) > now - GAVE_UP_KEEP}
    gave_up[rid] = request
    st.put("gave_up", gave_up)


def _answers(e: dict) -> str | None:
    """The id of the request a worker's reply answers ("" if it has
    none); None for an entry that is not a reply."""
    if e.get("type") in REPLY_OPS:
        return e.get("id") or ""
    w = e.get("w") or [""]
    if e.get("type") == "file" and w[0] == "file":
        return w[1] if len(w) > 1 else ""
    return None


def late_request(e: dict, gave_up: dict) -> dict | None:
    """The given-up request (give_up) that this reply answers, or None.
    The sender is checked too: no other contact may answer a request."""
    rid = _answers(e)
    r = gave_up.get(rid) if rid else None
    return r if r and r.get("peer") == e.get("peer") else None


def consumed(e: dict, gave_up: dict | None = None) -> bool:
    """Whether an entry is a reply that a blocking command waits for,
    rather than something to show the agent. A reply to a request the
    command gave up on (`gave_up`, the document give_up writes) is shown,
    unless it says the request expired: the agent knows it gave up."""
    if _answers(e) is None:
        return False
    if late_request(e, gave_up or {}) is None:
        return True
    return e.get("type") == "ack" and not e.get("ok") and (e.get("error") or "").startswith("expired")


def late_what(late: dict) -> str:
    """The request a late reply answers: "exec `make test` (sent 13:58)"."""
    if late.get("op") == "exec":
        what = f"exec `{late.get('cmd')}`"
    elif late.get("op") == "put":
        what = f"cp of {late.get('local')} to {late.get('remote')}"
    else:
        what = f"cp of {late.get('remote')}"
    return what + f" (sent {time.strftime('%H:%M', time.localtime(late.get('sent_at', 0)))})"


def late_text(e: dict) -> str:
    """What a late reply says, for the agent to read instead of its JSON."""
    return _reply_text(e, "late ", late_what(e["late"]))


def reply_to(e: dict) -> str | None:
    """The request a worker's reply answers, as history names it (by the
    start of its id, as request_text shows the request): "request
    1a2b3c4d"; None for an entry that is not a reply."""
    rid = _answers(e)
    return None if rid is None else _request_ref(rid)


def reply_text(e: dict) -> str:
    """What a reply that isn't late says, for history: as late_text, with
    the request named by reply_to."""
    return _reply_text(e, "", reply_to(e))


def request_text(e: dict) -> str | None:
    """What a request this agent sent to a worker asks, for history:
    "exec `make test` (request 1a2b3c4d)"; None for any other entry."""
    if e.get("direction") != "out":
        return None
    w = e.get("w") or [""]
    if e.get("type") == "file" and w[0] == "put" and len(w) > 2:
        return f"cp of {e.get('name')} to {w[2]} ({e.get('size')} bytes, {_request_ref(w[1])})"
    obj = parse_request(e.get("text") or "") if e.get("type") == "message" else None
    if obj and obj["myous"] == "exec":
        return f"exec `{obj.get('cmd')}` ({_request_ref(obj['id'])})"
    if obj and obj["myous"] == "get":
        return f"cp of {obj.get('path')} ({_request_ref(obj['id'])})"
    return None


def _request_ref(rid: str) -> str:
    return f"request {rid[:8]}" if rid else "a request"


def _reply_text(e: dict, late: str, what: str) -> str:
    if e["type"] == "file":
        return f"{late}reply to {what}: {describe_file(e)}"
    if e["type"] == "result":
        lines = [f"{late}result of {what}: exit {e.get('exit')}"]
        if e.get("stdout"):
            lines.append(e["stdout"].rstrip("\n"))
        if e.get("stderr"):
            lines += ["stderr:", e["stderr"].rstrip("\n")]
        if e.get("truncated"):
            lines.append("(the output was cut to fit the message)")
        return "\n".join(lines)
    if not e.get("ok"):
        answer = "refused: " + (e.get("error") or "no reason given")
    elif "path" in e:
        answer = f"written: {e['path']} ({e.get('size')} bytes)"
    else:
        answer = "done"
    return f"{late}answer to {what}: {answer}"


def shown(e: dict, gave_up: dict) -> dict:
    """An entry as unread shows it: a late reply gets "late" (the request
    it answers) and readable text."""
    r = late_request(e, gave_up)
    if r is None:
        return e
    e = dict(e, late={k: r[k] for k in ("op", "cmd", "local", "remote", "sent_at") if r.get(k) is not None})
    e["text"] = late_text(e)
    return e


def unread(st: Storage, mark_read: bool = True) -> list[dict]:
    with st.lock():
        state = st.get("state", {})
        last = state.get("read_seq", 0)
        gave_up = st.get("gave_up", {})
        entries = [shown(e, gave_up) for e in st.read_history()
                   if e["seq"] > last and e.get("direction") != "out" and not consumed(e, gave_up)]
        # The contact's current relationship context, so the agent has it when it answers.
        for e in entries:
            if e.get("peer"):
                e.update(contacts.context_of(st, e["peer"]))
        if mark_read and entries:
            state["read_seq"] = entries[-1]["seq"]
            st.put("state", state)
        return entries

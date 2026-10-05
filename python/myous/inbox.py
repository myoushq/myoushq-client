"""Message history and processing of incoming gift wraps.

History records, oldest first:
  {"seq", "type": "message", "direction": "in"|"out", "peer", "alias", "text", "at", "sent_at"?}
  {"seq", "type": "paired"|"pairing_failed", "peer"?, "alias"?, "text", "at"}
The "state" document keeps the last seq read and the gift wraps already
handled.

Callers hold `storage.lock()` around `record` and `handle_wraps`.
"""
from __future__ import annotations

import time

from nostr_sdk import Event, Keys

from myous import contacts, relay
from myous.storage import Storage

SEEN_RETENTION = 3 * 86400  # longer than relay retention plus timestamp jitter


def record(st: Storage, entry: dict) -> dict:
    state = st.get("state", {})
    entry = dict(entry, seq=state.get("next_seq", 1), at=int(time.time()))
    st.append_history(entry)
    state["next_seq"] = entry["seq"] + 1
    st.put("state", state)
    return entry


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
    messages = sorted((m for m in unwrapped if m), key=lambda m: (m[2], m[3]))
    stored = []
    for sender, text, sent_at, _ in messages:
        contact = contacts.approved(st, sender)
        if contact is None:
            continue  # not paired, or blocked: drop silently
        stored.append(record(st, {
            "type": "message", "direction": "in", "peer": contact["npub"],
            "alias": contact["alias"], "text": text, "sent_at": sent_at,
        }))
    return stored


def unread(st: Storage, mark_read: bool = True) -> list[dict]:
    with st.lock():
        state = st.get("state", {})
        last = state.get("read_seq", 0)
        entries = [e for e in st.read_history() if e["seq"] > last and e.get("direction") != "out"]
        if mark_read and entries:
            state["read_seq"] = entries[-1]["seq"]
            st.put("state", state)
        return entries

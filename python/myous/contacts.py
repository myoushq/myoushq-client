"""Paired peers. Only `approved` contacts can reach this agent.

Statuses: approved, blocked. `pending` is reserved for future contact
requests that the owner approves.

Relationship context (optional, kept only here): `relationship`, one of
RELATIONSHIPS, and `sharing`, the owner's guidance on what may be shared
with this contact. Incoming messages carry both when read, so the agent has
them when it answers.

Callers hold `storage.lock()` around changes.
"""
from __future__ import annotations

import time

from nostr_sdk import PublicKey

from myous.storage import Storage

APPROVED = "approved"
BLOCKED = "blocked"
RELATIONSHIPS = ("family", "friend", "colleague", "business", "service", "other")
MAX_SHARING = 500


def context_fields(relationship: str | None = None, sharing: str | None = None) -> dict:
    """Validated relationship context to store on a contact (only what's given)."""
    fields = {}
    if relationship is not None:
        if relationship not in RELATIONSHIPS:
            raise ValueError(f"relationship must be one of: {', '.join(RELATIONSHIPS)}")
        fields["relationship"] = relationship
    if sharing is not None:
        if len(sharing) > MAX_SHARING:
            raise ValueError(f"sharing guidance is limited to {MAX_SHARING} characters")
        fields["sharing"] = sharing.strip()
    return fields


def context_of(st: Storage, npub: str) -> dict:
    """{"relationship", "sharing"} for the contact with this npub (None if unset)."""
    for c in load(st).values():
        if c["npub"] == npub:
            return {"relationship": c.get("relationship"), "sharing": c.get("sharing")}
    return {"relationship": None, "sharing": None}


def load(st: Storage) -> dict[str, dict]:
    return st.get("contacts", {})


def add(st: Storage, pubkey_hex: str, alias: str) -> dict:
    """Pin a peer as approved. Re-pairing with a known key keeps its alias."""
    contacts = load(st)
    if pubkey_hex in contacts:
        contacts[pubkey_hex]["status"] = APPROVED
    else:
        contacts[pubkey_hex] = {
            "alias": _unique_alias(contacts, alias or "peer"),
            "npub": PublicKey.parse(pubkey_hex).to_bech32(),
            "status": APPROVED,
            "paired_at": int(time.time()),
        }
    st.put("contacts", contacts)
    return contacts[pubkey_hex]


def find(st: Storage, name: str) -> tuple[str, dict]:
    """Look up a contact by alias, npub or hex key."""
    contacts = load(st)
    for pubkey, c in contacts.items():
        if name in (c["alias"], c["npub"], pubkey):
            return pubkey, c
    for pubkey, c in contacts.items():
        if c["alias"].lower() == name.lower():
            return pubkey, c
    raise KeyError(f"no contact named {name!r}")


def update(st: Storage, name: str, **fields) -> dict:
    pubkey, _ = find(st, name)
    contacts = load(st)
    new_alias = fields.get("alias")
    if new_alias and any(c["alias"] == new_alias for k, c in contacts.items() if k != pubkey):
        raise ValueError(f"alias {new_alias!r} is already used")
    contacts[pubkey].update(fields)
    st.put("contacts", contacts)
    return contacts[pubkey]


def approved(st: Storage, pubkey_hex: str) -> dict | None:
    c = load(st).get(pubkey_hex)
    return c if c and c["status"] == APPROVED else None


def _unique_alias(contacts: dict[str, dict], alias: str) -> str:
    taken = {c["alias"] for c in contacts.values()}
    candidate, n = alias, 2
    while candidate in taken:
        candidate, n = f"{alias}-{n}", n + 1
    return candidate

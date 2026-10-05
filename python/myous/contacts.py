"""Paired peers. Only `approved` contacts can reach this agent.

Statuses: approved, blocked. `pending` is reserved for future contact
requests that the owner approves.

Callers hold `storage.lock()` around changes.
"""
from __future__ import annotations

import time

from nostr_sdk import PublicKey

from myous.storage import Storage

APPROVED = "approved"
BLOCKED = "blocked"


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

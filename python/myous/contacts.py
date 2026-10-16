"""Paired peers. Only `approved` contacts can reach this agent.

Statuses: approved, blocked. `pending` is reserved for future contact
requests that the owner approves.

Relationship context (optional, kept only here): `relationship`, one of
RELATIONSHIPS, and `sharing`, the owner's guidance on what may be shared
with this contact. Incoming messages carry both when read, so the agent has
them when it answers.

Cards (protocol.md, section 4): `card` is the latest {"name", "about",
"at"} the contact sent about itself; `peer_knows` is {"name", "about"} as
this agent last told the contact (the alias at pairing, then each card
sent), so a change is sent once. The contact's `alias` is this agent's own
label for it and never follows a card.

Callers hold `storage.lock()` around changes.
"""
from __future__ import annotations

import json
import time
import unicodedata

from nostr_sdk import PublicKey

from myous.storage import Storage

APPROVED = "approved"
BLOCKED = "blocked"
RELATIONSHIPS = ("family", "friend", "colleague", "business", "service", "other")
MAX_SHARING = 500
MAX_NAME = 64     # an alias, as in the pairing payload
MAX_ABOUT = 500   # a card's self-description


# What a name a peer chose may not contain: characters that act inside the
# double quotes a name is written in on a command line (the client's own
# suggestions, `myous send "NAME" ...`), and the invisible ones that
# reorder or break what is shown (bidi controls, line separators).
_SHELL_ACTIVE = frozenset('$`"\\')
_INVISIBLE = frozenset(chr(c) for c in (0x200E, 0x200F, 0x2028, 0x2029, *range(0x202A, 0x202F), *range(0x2066, 0x206A)))


def clean_text(text: str) -> str:
    """Text from a peer to show on one line: whitespace (newlines too) as
    single spaces, control and invisible characters (terminal escape
    sequences, bidi overrides) removed."""
    kept = (" " if c.isspace() else c for c in text
            if c.isspace() or (unicodedata.category(c) != "Cc" and c not in _INVISIBLE))
    return " ".join("".join(kept).split())


def visible(line: str) -> str:
    """One line of text from a peer without control characters (terminal
    escape sequences) and invisible ones; tabs and spaces kept."""
    return "".join(c for c in line if c == "\t" or (unicodedata.category(c) != "Cc" and c not in _INVISIBLE))


def clean_name(name: str) -> str:
    """A name a peer chose (its alias at pairing, the name in its card),
    safe to show and to put between double quotes in a shell command:
    clean_text, without $ ` " and backslash, at most MAX_NAME characters.
    "" if nothing is left."""
    return clean_text("".join(c for c in name if c not in _SHELL_ACTIVE))[:MAX_NAME].strip()


def context_fields(relationship: str | None = None, sharing: str | None = None,
                   added_by: str | None = None) -> dict:
    """Validated relationship context to store on a contact (only what's given).
    `added_by` records who made the pairing when it wasn't the agent itself
    ("owner": the owner, through the myous desktop app)."""
    fields = {}
    if added_by:
        fields["added_by"] = added_by.strip()
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
    """{"relationship", "sharing", "about"} for the contact with this npub
    (None if unset): the owner's context, and what the contact says about
    itself (its card)."""
    for c in load(st).values():
        if c["npub"] == npub:
            return {"relationship": c.get("relationship"), "sharing": c.get("sharing"),
                    "about": (c.get("card") or {}).get("about") or None}
    return {"relationship": None, "sharing": None, "about": None}


def parse_card(text: str) -> dict | None:
    """The {"name", "about"} of a card message (protocol.md, section 4),
    or None when the text is not a valid card."""
    if not text.startswith("{"):
        return None
    try:
        obj = json.loads(text)
    except ValueError:
        return None
    if not isinstance(obj, dict) or obj.get("myous") != "card":
        return None
    name, about = obj.get("name"), obj.get("about", "")
    if not isinstance(name, str) or not isinstance(about, str):
        return None
    if not 1 <= len(name.strip()) <= MAX_NAME or len(about.strip()) > MAX_ABOUT:
        return None
    # A peer's words: kept to one line, and the name safe on a command line.
    name, about = clean_name(name), clean_text(about)
    if not name:
        return None
    return {"name": name, "about": about}


def card_text(name: str, about: str) -> str:
    """The JSON of this agent's card."""
    return json.dumps({"myous": "card", "name": name, "about": about})


def receive_card(st: Storage, pubkey_hex: str, card: dict, at: int) -> tuple[dict, str]:
    """Store a contact's card; returns the contact and a line for the
    history saying what changed: a new or changed description, a new
    name (announced, never applied: the alias is ours), or both."""
    contacts = load(st)
    c = contacts[pubkey_hex]
    old = c.get("card") or {}
    known_name = old.get("name") or c["alias"]
    bits = []
    if card["name"] != known_name:
        bits.append(f"now calls itself \"{card['name']}\"; you call it \"{c['alias']}\" "
                    f"(keep that, or follow it: myous rename \"{c['alias']}\" \"{card['name']}\")")
    if card["about"] != (old.get("about") or ""):
        bits.append(f"describes itself: {card['about']}" if card["about"] else "cleared its description")
    if not bits:
        bits.append("sent its card again, unchanged")
    c["card"] = {"name": card["name"], "about": card["about"], "at": at}
    st.put("contacts", contacts)
    return c, f"{c['alias']} " + "; ".join(bits)


def peer_knows(st: Storage, pubkey_hex: str, name: str, about: str) -> None:
    """Record what this agent has told a contact about itself."""
    contacts = load(st)
    if pubkey_hex in contacts:
        contacts[pubkey_hex]["peer_knows"] = {"name": name, "about": about}
        st.put("contacts", contacts)


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
    known = ", ".join(sorted(c["alias"] for c in contacts.values()))
    raise KeyError(f'no contact named "{name}" ' +
                   (f"(your contacts: {known}; see `myous contacts`)" if known else "(no contacts yet)"))


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

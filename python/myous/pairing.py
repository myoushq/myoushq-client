"""Pairing two agents through the hub's mailbox. See PROTOCOL.md, "Pairing".

A pairing code looks like 4821-K7F3QX. The nameplate (4821) names a mailbox
on the hub; the secret (K7F3QX) never leaves the two agents. Both sides run
SPAKE2 with the full code as the password, relayed through the mailbox, and
get a shared key the hub can't learn. Each side then sends its identity
(public key and alias) encrypted with that key. A wrong code, or a hub that
tampers, makes decryption fail and the pairing aborts.

The PAKE lives in _pake_start/_pake_finish only, so it can be swapped out.

Mailbox messages (base64 of JSON):
  {"t": "pake", "v": 1, "m": <spake2 message>}
  {"t": "payload", "v": 1, "n": <nonce>, "c": <ciphertext>}
"""
from __future__ import annotations

import base64
import json
import os
import re
import secrets
import time
import urllib.parse

from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.ciphers.aead import ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from nostr_sdk import Keys, PublicKey
from spake2 import SPAKE2_A, SPAKE2_B

from myous import contacts, inbox
from myous.hub import Hub, HubError
from myous.storage import Storage

VERSION = 1
SECRET_LEN = 6
# Crockford base32: no I, L, O, U, so codes survive being read aloud.
ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
ID_A, ID_B = b"myous-pair-a", b"myous-pair-b"


class PairingError(Exception):
    pass


# --- codes ---------------------------------------------------------------

def make_secret() -> str:
    return "".join(secrets.choice(ALPHABET) for _ in range(SECRET_LEN))


def format_code(nameplate: str, secret: str) -> str:
    return f"{nameplate}-{secret}"


def make_link(link_base: str, nameplate: str, secret: str) -> str:
    return f"{link_base}{nameplate}#{secret}"


def parse_code(text: str, link_base: str) -> tuple[str, str]:
    """Accept a pairing link, or a code like '4821-K7F3QX' or '4821 k7f 3qx'."""
    text = text.strip()
    if "://" in text:
        url = urllib.parse.urlparse(text)
        expected = urllib.parse.urlparse(link_base).netloc
        if url.netloc != expected:
            raise PairingError(f"this link is for {url.netloc}, but this agent uses {expected}")
        match = re.fullmatch(r"/p/(\d+)", url.path)
        if not match or not url.fragment:
            raise PairingError("that doesn't look like a complete pairing link")
        return match.group(1), normalize_secret(url.fragment)
    match = re.fullmatch(r"(\d+)[\s-]+([0-9A-Za-z\s-]+)", text)
    if not match:
        raise PairingError("pairing codes look like 4821-K7F3QX")
    return match.group(1), normalize_secret(match.group(2))


def normalize_secret(raw: str) -> str:
    s = re.sub(r"[\s-]", "", raw).upper().translate(str.maketrans("ILO", "110"))
    if len(s) != SECRET_LEN or any(c not in ALPHABET for c in s):
        raise PairingError("the secret part of the code is not valid")
    return s


# --- crypto --------------------------------------------------------------

def _pake_start(role: str, code: str) -> tuple[bytes, bytes]:
    """Returns (message to send, state to keep)."""
    cls = SPAKE2_A if role == "a" else SPAKE2_B
    spake = cls(code.encode(), idA=ID_A, idB=ID_B)
    return spake.start(), spake.serialize()


def _pake_finish(role: str, state: bytes, peer_message: bytes) -> bytes:
    """Returns the shared key."""
    cls = SPAKE2_A if role == "a" else SPAKE2_B
    return cls.from_serialized(state).finish(peer_message)


def derive(key: bytes, info: str, length: int = 32) -> bytes:
    return HKDF(algorithm=hashes.SHA256(), length=length, salt=None,
                info=f"myous pairing v{VERSION} {info}".encode()).derive(key)


def verify_code(key: bytes) -> str:
    """Six digits both owners can compare by eye, if they want to."""
    return f"{int.from_bytes(derive(key, 'verify', 8), 'big') % 1_000_000:06d}"


def seal(key: bytes, role: str, nameplate: str, payload: dict, nonce: bytes | None = None) -> dict:
    nonce = nonce or os.urandom(12)
    sealed = ChaCha20Poly1305(derive(key, f"from {role}")).encrypt(
        nonce, json.dumps(payload, sort_keys=True).encode(), nameplate.encode())
    return {"t": "payload", "v": VERSION, "n": _b64(nonce), "c": _b64(sealed)}


def unseal(key: bytes, peer_role: str, nameplate: str, msg: dict) -> dict:
    try:
        plain = ChaCha20Poly1305(derive(key, f"from {peer_role}")).decrypt(
            _unb64(msg["n"]), _unb64(msg["c"]), nameplate.encode())
        payload = json.loads(plain)
        PublicKey.parse(payload["pubkey"])
    except Exception:
        raise PairingError("the code didn't match (or the exchange was tampered with)")
    return payload


def _b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def _unb64(s: str) -> bytes:
    return base64.b64decode(s)


# --- the exchange --------------------------------------------------------
# A pairing spans a few round trips, so its state is saved as a
# "pending/<nameplate>" document and finished by whichever call gets there
# first: accept (which waits a while), or any later advance().

class Pairing:
    def __init__(self, storage: Storage, hub: Hub, keys: Keys, alias: str):
        self.st, self.hub, self.keys, self.alias = storage, hub, keys, alias

    def pending(self) -> list[dict]:
        found = (self.st.get(name) for name in self.st.names("pending/"))
        return [p for p in found if p]

    def invite(self, context: dict | None = None) -> dict:
        """Open a mailbox and post our half of the PAKE. Returns immediately
        with the code and link to share. `context` (relationship, sharing)
        is stored on the contact when the pairing finishes."""
        box = self.hub.request("POST", "/api/pair", {})
        nameplate, secret = box["nameplate"], make_secret()
        message, state = _pake_start("a", format_code(nameplate, secret))
        self._post(nameplate, box["token"], {"t": "pake", "v": VERSION, "m": _b64(message)})
        p = {
            "role": "a", "nameplate": nameplate, "secret": secret, "token": box["token"],
            "expires_at": box["expires_at"], "pake": _b64(state), "after": 0, "stage": "wait_pake",
        }
        if context:
            p["context"] = context
        self._save(p)
        link_base = self.hub.config()["pair_link_base"]
        return dict(p, code=format_code(nameplate, secret), link=make_link(link_base, nameplate, secret))

    def accept(self, code: str, wait: float = 60, context: dict | None = None) -> dict:
        """Join someone else's invite. Finishes now if the other side answers
        within `wait` seconds; otherwise a later advance() finishes it."""
        nameplate, secret = parse_code(code, self.hub.config()["pair_link_base"])
        mine = self.st.get(f"pending/{nameplate}")
        if mine and mine["role"] == "b" and mine["secret"] == secret:
            # Accepted before (e.g. the connection dropped); carry on with it.
            return self.advance(mine, wait=wait)
        try:
            claim = self.hub.request("POST", f"/api/pair/{nameplate}/claim", {})
        except HubError as e:
            raise PairingError(e.message) from None
        message, state = _pake_start("b", format_code(nameplate, secret))
        self._post(nameplate, claim["token"], {"t": "pake", "v": VERSION, "m": _b64(message)})
        p = {
            "role": "b", "nameplate": nameplate, "secret": secret, "token": claim["token"],
            "expires_at": claim["expires_at"], "pake": _b64(state), "after": 0, "stage": "wait_pake",
        }
        if context:
            p["context"] = context
        self._save(p)
        return self.advance(p, wait=wait)

    def advance_all(self) -> list[dict]:
        """Move every pending pairing forward without waiting. Returns the
        ones that finished (stage "done" or "failed")."""
        finished = []
        for p in self.pending():
            try:
                r = self.advance(p, wait=0, block=False)
            except (HubError, OSError):
                continue  # hub unreachable: try again next time
            if r["stage"] in ("done", "failed"):
                finished.append(r)
        return finished

    def advance(self, p: dict, wait: float = 0, block: bool = True) -> dict:
        """Move one pairing forward with whatever the peer has posted.

        Returns the record with "stage" set to "done" (with "contact" and
        "verify"), "failed" (with "error"), "elsewhere" (another run finished
        it), or still waiting."""
        with self.st.lock(f"pending/{p['nameplate']}", wait=block) as got:
            if not got:
                return p
            current = self.st.get(f"pending/{p['nameplate']}")
            if current is None:
                return dict(p, stage="elsewhere")
            return self._advance_locked(current, wait)

    def _advance_locked(self, p: dict, wait: float) -> dict:
        deadline = time.time() + wait
        while True:
            if time.time() > p["expires_at"]:
                return self._fail(p, "pairing invite expired")
            try:
                got = self.hub.request(
                    "GET", f"/api/pair/{p['nameplate']}/messages?after={p['after']}"
                           f"&wait={int(max(0, min(25, deadline - time.time())))}",
                    token=p["token"], timeout=40)
            except HubError as e:
                if e.status in (403, 404):
                    return self._fail(p, "pairing invite expired or was closed")
                raise
            except OSError:
                # Network trouble (proxies drop long polls): retry while there's
                # time, else leave it pending for the next poll.
                if time.time() + 2 >= deadline:
                    return p
                time.sleep(2)
                continue
            for body in got["messages"]:
                p["after"] += 1
                try:
                    self._step(p, json.loads(_unb64(body)))
                except PairingError as e:
                    return self._fail(p, str(e))
                if p["stage"] == "done":
                    return p
            self._save(p)
            if time.time() >= deadline:
                return p

    def _step(self, p: dict, msg: dict) -> None:
        peer_role = "b" if p["role"] == "a" else "a"
        if p["stage"] == "wait_pake" and msg.get("t") == "pake":
            try:
                key = _pake_finish(p["role"], _unb64(p.pop("pake")), _unb64(msg["m"]))
            except Exception:
                raise PairingError("bad message from the other side")
            p.update(key=_b64(key), stage="wait_payload")
            self._save(p)
            mine = {"v": VERSION, "pubkey": self.keys.public_key().to_hex(), "alias": self.alias}
            self._post(p["nameplate"], p["token"], seal(key, p["role"], p["nameplate"], mine))
        elif p["stage"] == "wait_payload" and msg.get("t") == "payload":
            key = _unb64(p["key"])
            payload = unseal(key, peer_role, p["nameplate"], msg)
            if payload["pubkey"] == self.keys.public_key().to_hex():
                raise PairingError("that's this agent's own invite")
            with self.st.lock():
                contact = contacts.add(self.st, payload["pubkey"], str(payload.get("alias", ""))[:64])
                if p.get("context"):
                    contact = contacts.update(self.st, payload["pubkey"], **p["context"])
                p.update(stage="done", contact=contact, verify=verify_code(key))
                self.st.delete(f"pending/{p['nameplate']}")
                text = f"paired with {contact['alias']} (verification code {p['verify']})"
                if not contact.get("relationship"):
                    text += (". Ask your owner how they know this contact and what you may share with it, "
                             f"then record it: myous context \"{contact['alias']}\" --relationship ... --sharing \"...\"")
                inbox.record(self.st, {"type": "paired", "peer": contact["npub"], "alias": contact["alias"],
                                       "verify": p["verify"], "text": text})
            # Don't close the mailbox: the peer may not have read our payload
            # yet. It only holds ciphertext and expires on its own.
        else:
            raise PairingError("unexpected message from the other side")

    def _fail(self, p: dict, error: str) -> dict:
        self.st.delete(f"pending/{p['nameplate']}")
        try:
            self.hub.request("DELETE", f"/api/pair/{p['nameplate']}", token=p["token"])
        except (HubError, OSError):
            pass
        with self.st.lock():
            inbox.record(self.st, {"type": "pairing_failed",
                                   "text": f"pairing {p['nameplate']} failed: {error}"})
        p.update(stage="failed", error=error)
        return p

    def _save(self, p: dict) -> None:
        self.st.put(f"pending/{p['nameplate']}", p)

    def _post(self, nameplate: str, token: str, msg: dict) -> None:
        body = _b64(json.dumps(msg).encode())
        self.hub.request("POST", f"/api/pair/{nameplate}/messages", {"body": body}, token=token)

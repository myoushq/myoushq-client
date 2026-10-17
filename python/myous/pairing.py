"""Pairing two agents through the hub's mailbox. See PROTOCOL.md, "Pairing".

A pairing code looks like 4821-K7F3QX. The nameplate (4821) names a mailbox
on the hub; the secret (K7F3QX) never leaves the two agents. Both sides run
SPAKE2 with the full code as the password, relayed through the mailbox, and
get a shared key the hub can't learn. Each side then sends its identity
(public key and alias) encrypted with that key, with a signature by that
key over a value derived from the shared key, which proves the sender
holds it. A wrong code, or a hub that tampers, makes decryption fail and
the pairing aborts. A pairing never changes an existing contact: a key
that is already one stays as it is (blocked stays blocked).

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

from nostr_sdk import Event, Keys, PublicKey
from spake2 import SPAKE2_A, SPAKE2_B

from myous import _aead, contacts, inbox
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
    """HKDF-SHA256, no salt."""
    return _aead.hkdf_sha256(key, length, f"myous pairing v{VERSION} {info}".encode())


def verify_code(key: bytes) -> str:
    """Six digits both owners can compare by eye, if they want to."""
    return f"{int.from_bytes(derive(key, 'verify', 8), 'big') % 1_000_000:06d}"


NONCE_LEN, TAG_LEN = 12, 16  # ChaCha20-Poly1305 as in RFC 8439


def _check_nonce(nonce: bytes) -> None:
    """Only the RFC 8439 nonce: pycryptodome would take 8 or 24 bytes as
    other ChaCha20 variants."""
    if len(nonce) != NONCE_LEN:
        raise ValueError(f"nonce must be {NONCE_LEN} bytes")


def seal(key: bytes, role: str, nameplate: str, payload: dict, nonce: bytes | None = None) -> dict:
    """ChaCha20-Poly1305 with the nameplate as associated data."""
    nonce = nonce or os.urandom(NONCE_LEN)
    _check_nonce(nonce)
    sealed = _aead.chacha20_poly1305_seal(derive(key, f"from {role}"), nonce, nameplate.encode(),
                                          json.dumps(payload, sort_keys=True).encode())
    return {"t": "payload", "v": VERSION, "n": _b64(nonce), "c": _b64(sealed)}


def unseal(key: bytes, peer_role: str, nameplate: str, msg: dict) -> dict:
    try:
        nonce, data = _unb64(msg["n"]), _unb64(msg["c"])
        _check_nonce(nonce)
        if len(data) < TAG_LEN:
            raise ValueError("too short")
        plain = _aead.chacha20_poly1305_open(derive(key, f"from {peer_role}"), nonce, nameplate.encode(), data)
        payload = json.loads(plain)
        if not valid_pubkey(payload["pubkey"]):
            raise ValueError("not a key")
    except Exception:
        raise PairingError("the code didn't match (or the exchange was tampered with)")
    return payload


_P = 2**256 - 2**32 - 977  # secp256k1's field prime


def valid_pubkey(text) -> bool:
    """A key as protocol.md writes it: 64 lowercase hex characters, the x
    of a point on secp256k1 (BIP-340's lift_x). PublicKey.parse alone also
    takes npubs, uppercase and x values off the curve."""
    if not (isinstance(text, str) and re.fullmatch(r"[0-9a-f]{64}", text)):
        return False
    x = int(text, 16)
    return x < _P and pow((pow(x, 3, _P) + 7) % _P, (_P - 1) // 2, _P) == 1


def key_proof(key: bytes, role: str) -> bytes:
    """The 32 bytes a side signs with its identity key, so the other side
    knows it holds the key in its payload (protocol.md, section 5.4). They
    come from the shared key, so a signature made for one pairing proves
    nothing in another."""
    return derive(key, f"key proof from {role}")


def sign_proof(keys: Keys, key: bytes, role: str) -> str:
    return keys.sign_schnorr(key_proof(key, role))


def proof_holds(pubkey_hex: str, message: bytes, sig) -> bool:
    """Whether `sig` is a BIP-340 signature of `message` by `pubkey_hex`.
    nostr_sdk has no bare Schnorr check, but Event.verify_signature checks
    the signature over the event id as given (it doesn't recompute the id),
    so an event whose id is `message` checks exactly that."""
    if not (isinstance(sig, str) and re.fullmatch(r"[0-9a-f]{128}", sig)):
        return False
    event = {"id": message.hex(), "pubkey": pubkey_hex, "created_at": 0, "kind": 0,
             "tags": [], "content": "", "sig": sig}
    try:
        return Event.from_json(json.dumps(event)).verify_signature()
    except Exception:
        return False


def check_proof(key: bytes, peer_role: str, payload: dict) -> bool:
    """True if the payload proves its key; False if it has no proof (clients
    up to 0.7.5 send none; a later version will require it). A proof that
    doesn't hold fails the pairing."""
    if payload.get("sig") is None:
        return False
    if not proof_holds(payload["pubkey"], key_proof(key, peer_role), payload["sig"]):
        raise PairingError("the other side didn't prove it holds the key it sent")
    return True


def _known_refused(c: dict, proven: bool) -> str:
    """Why a pairing that named a contact which isn't approved changed nothing."""
    who = (f'answered by your contact "{c["alias"]}" ({c["status"]})' if proven else
           f'answered with the key of your contact "{c["alias"]}" ({c["status"]}), '
           "which the other side didn't prove it holds")
    return f'{who}; nothing changed: if that is right, unblock it (myous unblock "{c["alias"]}")'


def _refused(e: HubError) -> bool:
    """The hub turned the request down for good (gone, taken, bad), as
    opposed to "not now" (429) or a proxy that couldn't reach it (5xx)."""
    return 400 <= e.status < 500 and e.status != 429


def _body(msg: dict) -> str:
    """A mailbox message as posted: base64 of its JSON."""
    return _b64(json.dumps(msg).encode())


def _b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def _unb64(s: str) -> bytes:
    return base64.b64decode(s)


# --- the exchange --------------------------------------------------------
# A pairing spans a few round trips, so its state is saved as a
# "pending/<nameplate>" document and finished by whichever call gets there
# first: accept (which waits a while), or any later advance().

CLAIM_ATTEMPTS = 3
CLAIM_RETRY_DELAY = 1.0  # seconds, growing per attempt
TRACE_LEN = 20  # steps kept per pairing


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
            self.st.delete(f"claiming/{nameplate}")
            return self.advance(mine, wait=wait)
        claiming = self.st.get(f"claiming/{nameplate}") or {}
        trace = claiming.get("trace") or []
        try:
            claim = self._claim(nameplate, claiming, trace)
        except Exception:
            self.st.put("pairing-trace", {"nameplate": nameplate, "trace": trace})
            raise
        message, state = _pake_start("b", format_code(nameplate, secret))
        # Saved before it is posted ("posting"): if the answer to the post
        # is lost, advance() posts exactly this message again, never a new one.
        p = {
            "role": "b", "nameplate": nameplate, "secret": secret, "token": claim["token"],
            "expires_at": claim["expires_at"], "pake": _b64(state), "after": 0, "stage": "posting",
            "message": _body({"t": "pake", "v": VERSION, "m": _b64(message)}), "trace": trace,
        }
        if context:
            p["context"] = context
        self._save(p)
        self.st.delete(f"claiming/{nameplate}")
        return self.advance(p, wait=wait)

    def _claim(self, nameplate: str, saved: dict, trace: list) -> dict:
        """Claim the invite's second side, retrying when the answer is lost
        (the connection drops, or a proxy answers 5xx).

        The claim carries a random id, saved in "claiming/<nameplate>"
        before the first attempt: the hub answers a repeat of the same id
        with the same token, so neither a retry here nor running the same
        accept again (after a crash, or when every attempt failed) burns
        the code. The record goes once the pairing is saved or the hub
        refuses the code; it stays while the hub can't be reached. It also
        holds the trace of the attempts."""
        name = f"claiming/{nameplate}"
        claim_id = saved.get("claim")
        if not (isinstance(claim_id, str) and re.fullmatch(r"[0-9a-f]{32}", claim_id)):
            claim_id = secrets.token_hex(16)
        dropped = None
        for attempt in range(CLAIM_ATTEMPTS):
            self.st.put(name, {"nameplate": nameplate, "claim": claim_id, "trace": trace})
            try:
                answer = self.hub.request("POST", f"/api/pair/{nameplate}/claim", {"claim": claim_id})
                self._trace(trace, "claim", claimed_at=answer.get("claimed_at") if answer.get("repeat") else None)
                return answer
            except HubError as e:
                self._trace(trace, "claim", error=e)
                if e.status < 500:  # turned down; a 429 keeps the record for later
                    if _refused(e):
                        self.st.delete(name)
                    raise PairingError(e.message) from None
                dropped = e  # a proxy that couldn't get the hub's answer
            except OSError as e:
                self._trace(trace, "claim", error=e)
                dropped = e
            if attempt + 1 < CLAIM_ATTEMPTS:
                time.sleep(CLAIM_RETRY_DELAY * (attempt + 1))
        self.st.put(name, {"nameplate": nameplate, "claim": claim_id, "trace": trace})
        raise PairingError(f"could not reach the hub to accept the code ({dropped!r}); accepting the same "
                           "code again carries on where this stopped") from None

    def _trace(self, trace: list, step: str, error: Exception | str | None = None,
               claimed_at: int | None = None) -> None:
        """Note a step of the handshake (claim, post, post (repeat), wait,
        done, failed) in the pairing's own record, for the owner to read
        when it fails. It stays on this machine."""
        entry = {"at": int(time.time()), "step": step}
        if step in ("claim", "post", "post (repeat)", "wait"):
            entry["host"] = urllib.parse.urlparse(self.hub.url).netloc
        if error is not None:
            entry["error"] = error if isinstance(error, str) else str(error) if isinstance(error, HubError) else repr(error)
        if claimed_at is not None:
            entry["claimed_at"] = claimed_at
        trace.append(entry)
        del trace[:-TRACE_LEN]

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
            if p["stage"] == "posting":
                # Our PAKE message, saved before the first try; the hub takes
                # a repeat of a side's last message without appending it.
                try:
                    answer = self.hub.request("POST", f"/api/pair/{p['nameplate']}/messages",
                                              {"body": p["message"]}, token=p["token"]) or {}
                except (HubError, OSError) as e:
                    self._trace(p.setdefault("trace", []), "post", error=e)
                    if isinstance(e, HubError) and e.status in (403, 404):
                        return self._fail(p, "pairing invite expired or was closed")
                    if isinstance(e, HubError) and e.status < 500:
                        return self._fail(p, e.message)
                    self._save(p)  # unreachable: post it again next time
                    if time.time() + 2 >= deadline:
                        return p
                    time.sleep(2)
                    continue
                self._trace(p.setdefault("trace", []), "post (repeat)" if answer.get("repeat") else "post")
                del p["message"]
                p["stage"] = "wait_pake"
                self._save(p)
            try:
                got = self.hub.request(
                    "GET", f"/api/pair/{p['nameplate']}/messages?after={p['after']}"
                           f"&wait={int(max(0, min(25, deadline - time.time())))}",
                    token=p["token"], timeout=40)
            except HubError as e:
                if e.status in (403, 404):
                    return self._fail(p, "pairing invite expired or was closed")
                raise
            except OSError as e:
                # Network trouble (proxies drop long polls): retry while there's
                # time, else leave it pending for the next poll.
                self._trace(p.setdefault("trace", []), "wait", error=e)
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
            self._post(p["nameplate"], p["token"], seal(key, p["role"], p["nameplate"], self._payload(key, p["role"])))
        elif p["stage"] == "wait_payload" and msg.get("t") == "payload":
            key = _unb64(p["key"])
            payload = unseal(key, peer_role, p["nameplate"], msg)
            if payload["pubkey"] == self.keys.public_key().to_hex():
                raise PairingError("that's this agent's own invite")
            proven = check_proof(key, peer_role, payload)
            with self.st.lock():
                known = contacts.load(self.st).get(payload["pubkey"])
                if known and known["status"] != contacts.APPROVED:
                    # Raised inside the lock's block, which releases it before _fail.
                    raise PairingError(_known_refused(known, proven))
                contact = known or contacts.add(self.st, payload["pubkey"], contacts.clean_name(str(payload.get("alias", ""))),
                                                **(p.get("context") or {}))
                # They learned our alias from the payload; a card follows if we have one.
                contacts.peer_knows(self.st, payload["pubkey"], self.alias, "")
                p.update(stage="done", contact=contact, verify=verify_code(key), known=bool(known))
                self._trace(p.setdefault("trace", []), "done")
                self.st.delete(f"pending/{p['nameplate']}")
                text = f"paired with {contact['alias']} (verification code {p['verify']})"
                given = bool(p.get("context", {}).keys() & {"relationship", "sharing"})
                if known:
                    text += f". {contact['alias']} was already your contact: nothing changed"
                    if given:
                        text += (" (the relationship and sharing given with this invite were not applied; "
                                 f"to change them: myous context \"{contact['alias']}\" --relationship ... --sharing \"...\")")
                if not contact.get("relationship") and not (known and given):
                    text += (". Ask your owner how they know this contact and what you may share with it, "
                             f"then record it: myous context \"{contact['alias']}\" --relationship ... --sharing \"...\"")
                inbox.record(self.st, {"type": "paired", "peer": contact["npub"], "alias": contact["alias"],
                                       "verify": p["verify"], "text": text})
            # Don't close the mailbox: the peer may not have read our payload
            # yet. It only holds ciphertext and expires on its own.
        else:
            raise PairingError("unexpected message from the other side")

    def _payload(self, key: bytes, role: str) -> dict:
        """What this side tells the other: its key, with the proof that it
        holds it, and its alias."""
        return {"v": VERSION, "pubkey": self.keys.public_key().to_hex(), "alias": self.alias,
                "sig": sign_proof(self.keys, key, role)}

    def _fail(self, p: dict, error: str) -> dict:
        self._trace(p.setdefault("trace", []), "failed", error=error)
        self.st.put("pairing-trace", {"nameplate": p["nameplate"], "trace": p["trace"]})
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
        self.hub.request("POST", f"/api/pair/{nameplate}/messages", {"body": _body(msg)}, token=token)

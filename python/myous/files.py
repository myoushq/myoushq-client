"""Files: encryption and the hub's blob store (protocol.md, section 6).

A file goes out as an encrypted blob on the hub plus a file message that
carries the key. The hub never sees the plaintext, the name, or who the
recipient is. Every blob call is authorized with a signed Nostr event
(Blossom-style, kind 24242), so only registered agents use the store.
"""
from __future__ import annotations

import base64
import hashlib
import http.client
import json
import os
import re
import secrets
import time
import urllib.error
import urllib.request
from typing import NamedTuple

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from nostr_sdk import EventBuilder, Keys, Kind, Tag, Timestamp

KIND_BLOB_AUTH = 24242
KEY_BYTES = 32
NONCE_BYTES = 12
MAX_BLOB = 64 << 20  # the hub's cap per blob
TAG_BYTES = 16  # AES-GCM appends this much, so a plaintext must be smaller
MAX_PLAINTEXT = MAX_BLOB - TAG_BYTES
AUTH_TTL = 5 * 60  # how long an authorization event stays valid
ALGORITHM = "aes-gcm"
DEFAULT_MIME = "application/octet-stream"


class Encrypted(NamedTuple):
    key: bytes
    nonce: bytes
    ciphertext: bytes
    x: str  # sha256 of the ciphertext, hex
    ox: str  # sha256 of the plaintext, hex


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def encrypt(plaintext: bytes, key: bytes | None = None, nonce: bytes | None = None) -> Encrypted:
    """AES-256-GCM with a fresh random key and nonce per file (fixed ones
    only for test vectors). The 16-byte tag is appended, as the library does."""
    if len(plaintext) > MAX_PLAINTEXT:
        raise ValueError(f"file is {len(plaintext)} bytes; the limit is {MAX_PLAINTEXT} ({MAX_BLOB >> 20} MB less the tag)")
    key = key or secrets.token_bytes(KEY_BYTES)
    nonce = nonce or secrets.token_bytes(NONCE_BYTES)
    if len(key) != KEY_BYTES or len(nonce) != NONCE_BYTES:
        raise ValueError("key must be 32 bytes and nonce 12 bytes")
    ciphertext = AESGCM(key).encrypt(nonce, plaintext, None)
    return Encrypted(key, nonce, ciphertext, sha256(ciphertext), sha256(plaintext))


def decrypt(ciphertext: bytes, key: bytes, nonce: bytes, x: str | None = None, ox: str | None = None) -> bytes:
    """The plaintext, after checking the ciphertext hash (if given), the
    authentication tag, and the plaintext hash (if given)."""
    if x is not None and sha256(ciphertext) != x:
        raise ValueError("the downloaded blob doesn't match the hash in the message")
    try:
        plaintext = AESGCM(key).decrypt(nonce, ciphertext, None)
    except Exception:
        raise ValueError("the file could not be decrypted (wrong key, or the blob was altered)") from None
    if ox is not None and sha256(plaintext) != ox:
        raise ValueError("the decrypted file doesn't match the hash in the message")
    return plaintext


def safe_name(name: str) -> str | None:
    """The last path component of a sender-supplied file name, or None if
    there's nothing usable: a name must never point outside the directory
    the receiver writes to."""
    if not isinstance(name, str):
        return None
    name = name.replace("\\", "/").rsplit("/", 1)[-1].strip()
    name = "".join(c for c in name if c.isprintable() and c not in "\0")
    if name in ("", ".", "..") or len(name) > 255:
        return None
    return name


def file_tags(enc: Encrypted, name: str, mime: str, extra: list[list[str]] | None = None) -> list[list[str]]:
    """The tags of a kind-15 file message (section 6.3), without p and ms."""
    tags = [
        ["file-type", mime or DEFAULT_MIME],
        ["encryption-algorithm", ALGORITHM],
        ["decryption-key", enc.key.hex()],
        ["decryption-nonce", enc.nonce.hex()],
        ["x", enc.x],
        ["ox", enc.ox],
        ["size", str(len(enc.ciphertext))],
        ["name", name],
    ]
    return tags + [list(t) for t in (extra or [])]


_HEX = re.compile(r"^[0-9a-f]+$")


def parse_file_tags(tags: list[list[str]], content: str, blob_api: str | None) -> dict | None:
    """The fields of a received file message, validated, or None if it is
    malformed or points anywhere but the hub's own blob store."""
    fields: dict = {}
    for t in tags:
        if len(t) >= 2 and t[0] in ("file-type", "encryption-algorithm", "decryption-key",
                                    "decryption-nonce", "x", "ox", "size", "name"):
            fields.setdefault(t[0], t[1])
        elif t and t[0] == "w":
            fields["w"] = list(t[1:])
    key, nonce, x, ox = (fields.get(k, "") for k in ("decryption-key", "decryption-nonce", "x", "ox"))
    if fields.get("encryption-algorithm") != ALGORITHM:
        return None
    if not (len(key) == 2 * KEY_BYTES and len(nonce) == 2 * NONCE_BYTES and len(x) == 64 and len(ox) == 64):
        return None
    if not all(_HEX.match(v) for v in (key, nonce, x, ox)):
        return None
    name = safe_name(fields.get("name", ""))
    if name is None:
        return None
    if not blob_api or content != f"{blob_api.rstrip('/')}/{x}":
        return None
    size = fields.get("size", "")
    return {
        "name": name, "mime": fields.get("file-type") or DEFAULT_MIME,
        "size": int(size) if size.isdigit() else None,
        "x": x, "ox": ox, "url": content, "key": key, "nonce": nonce,
        **({"w": fields["w"]} if "w" in fields else {}),
    }


class BlobError(Exception):
    def __init__(self, status: int, message: str):
        super().__init__(f"blob store error {status}: {message}")
        self.status = status
        self.message = message


class BlobClient:
    """Upload, download and delete blobs, as this agent."""

    def __init__(self, keys: Keys, blob_api: str):
        if not blob_api:
            raise ValueError("the hub doesn't offer a blob store (no blob_api in its config)")
        self.keys = keys
        self.url = blob_api.rstrip("/")
        self.max_blob = MAX_BLOB  # never read more than this from the hub (tests lower it)

    def authorization(self, action: str, x: str, now: int | None = None) -> str:
        """The Authorization header value: a signed kind-24242 event."""
        now = now or int(time.time())
        tags = [Tag.parse(["t", action]), Tag.parse(["x", x]),
                Tag.expiration(Timestamp.from_secs(now + AUTH_TTL))]
        event = EventBuilder(Kind(KIND_BLOB_AUTH), f"myous {action}").tags(tags).finalize(self.keys)
        token = base64.urlsafe_b64encode(event.as_json().encode()).decode().rstrip("=")
        return "Nostr " + token

    def upload(self, data: bytes, mime: str = DEFAULT_MIME) -> dict:
        """Store a blob. Returns the hub's descriptor (url, sha256, size, ...)."""
        if len(data) > MAX_BLOB:
            raise ValueError(f"blob is {len(data)} bytes; the limit is {MAX_BLOB}")
        x = sha256(data)
        raw = self._call("PUT", "/upload", "upload", x, data, {"Content-Type": mime, "X-SHA-256": x})
        return json.loads(raw)

    def get(self, x: str, size: int | None = None) -> bytes:
        """The stored bytes. `size`, when known from the file message, must
        match what the hub says it is sending."""
        return self._call("GET", "/" + x, "get", x, expect_size=size)

    def head(self, x: str) -> int:
        """The stored size, or a BlobError (404 once it has expired)."""
        return int(self._call("HEAD", "/" + x, "get", x, want_length=True))

    def delete(self, x: str) -> None:
        self._call("DELETE", "/" + x, "delete", x)

    def _call(self, method: str, endpoint: str, action: str, x: str, body: bytes | None = None,
              headers: dict | None = None, want_length: bool = False, expect_size: int | None = None,
              timeout: float = 120):
        req = urllib.request.Request(self.url + endpoint, data=body, method=method)
        req.add_header("Authorization", self.authorization(action, x))
        for k, v in (headers or {}).items():
            req.add_header(k, v)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                if want_length:
                    return resp.headers.get("Content-Length", "0")
                # The hub (or whatever answers in its place) decides what it
                # sends; the blob cap bounds what we hold in memory.
                length = resp.headers.get("Content-Length")
                if expect_size is not None and length is not None and length.isdigit() and int(length) != expect_size:
                    raise ValueError(f"the hub is sending {length} bytes, the message said {expect_size}")
                data = resp.read(self.max_blob + 1)
                if len(data) > self.max_blob:
                    raise ValueError(f"the hub sent more than {self.max_blob} bytes; refusing to read on")
                return data
        except urllib.error.HTTPError as e:
            try:
                message = json.loads(e.read()).get("error", e.reason)
            except ValueError:
                message = e.reason
            raise BlobError(e.code, message) from None
        except http.client.HTTPException as e:
            raise ConnectionError(f"lost the connection to {self.url}: {e!r}") from None


def unique_path(directory: str | os.PathLike, name: str) -> str:
    """A path under `directory` for `name` that doesn't exist yet: `name`,
    else `name (2)`, `name (3)`, ... before the extension."""
    base, ext = os.path.splitext(name)
    candidate, n = os.path.join(directory, name), 2
    while os.path.exists(candidate):
        candidate, n = os.path.join(directory, f"{base} ({n}){ext}"), n + 1
    return candidate

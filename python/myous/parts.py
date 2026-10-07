"""Long messages: splitting text into parts and putting it back together.
See PROTOCOL.md, "Long messages".

One message holds about 28 KB of text. Longer text goes out as up to 16
parts, each a normal message tagged ["part", id, index, total]; the
receiver buffers them and delivers one message when all have arrived.
"""
from __future__ import annotations

import re
import secrets

PART_BYTES = 24_000  # JSON-escaped size of one part's text
MAX_PARTS = 16
MAX_BYTES = 262_144  # UTF-8 size of a whole message
MAX_UNFINISHED = 4  # per sender
UNFINISHED_TTL = 3600  # seconds after the first part arrived

_SHORT_ESCAPES = set('"\\\b\f\n\r\t')
_LONG_ESCAPES = set("<>&  ")


def _escaped_size(c: str) -> int:
    if c in _SHORT_ESCAPES:
        return 2
    if c in _LONG_ESCAPES or ord(c) < 0x20:
        return 6
    return len(c.encode())


def split(text: str) -> list[str]:
    """The parts to send `text` in (one, if it fits). Raises ValueError if
    it's too long to send at all."""
    size = len(text.encode())
    if size > MAX_BYTES:
        raise ValueError(f"message is {size} bytes; the limit is {MAX_BYTES}. Shorten it or send it in several messages")
    parts, current, current_size = [], [], 0
    for c in text:
        n = _escaped_size(c)
        if current and current_size + n > PART_BYTES:
            parts.append("".join(current))
            current, current_size = [], 0
        current.append(c)
        current_size += n
    parts.append("".join(current))
    if len(parts) > MAX_PARTS:
        raise ValueError(f"message needs {len(parts)} parts; the limit is {MAX_PARTS}. Shorten it")
    return parts


def new_id() -> str:
    return secrets.token_hex(16)


def parse_tag(values: list[str]) -> tuple[str, int, int] | None:
    """(id, index, total) from a ["part", ...] tag, or None if malformed."""
    if len(values) < 4 or not re.fullmatch(r"[0-9a-f]{1,64}", values[1]):
        return None
    if not (values[2].isdigit() and values[3].isdigit()):
        return None
    index, total = int(values[2]), int(values[3])
    if not (2 <= total <= MAX_PARTS and 1 <= index <= total):
        return None
    return values[1], index, total


def add(buffer: dict, sender: str, part: tuple[str, int, int], text: str,
        sent_at: int, ms: int, now: int) -> tuple[str, int, int] | None:
    """Buffer one part. Returns (text, sent_at, ms) when the message is
    complete, else None. `buffer` is the stored "partials" document."""
    pid, index, total = part
    key = f"{sender}:{pid}"
    entry = buffer.get(key)
    if entry is None:
        mine = sorted((k for k in buffer if k.startswith(sender + ":")), key=lambda k: buffer[k]["first"])
        for old in mine[: max(0, len(mine) - MAX_UNFINISHED + 1)]:
            del buffer[old]
        entry = buffer[key] = {"sender": sender, "total": total, "parts": {}, "first": now}
    if entry["total"] != total or str(index) in entry["parts"]:
        return None
    if sum(len(t.encode()) for t in entry["parts"].values()) + len(text.encode()) > MAX_BYTES:
        del buffer[key]
        return None
    entry["parts"][str(index)] = text
    if index == 1:
        entry["sent_at"], entry["ms"] = sent_at, ms
    if len(entry["parts"]) < total:
        return None
    del buffer[key]
    joined = "".join(entry["parts"][str(i)] for i in range(1, total + 1))
    return joined, entry.get("sent_at", sent_at), entry.get("ms", ms)


def expire(buffer: dict, now: int) -> list[tuple[str, str, int]]:
    """Remove messages unfinished for too long. Returns (sender, text,
    sent_at) for each, with markers where parts are missing."""
    out = []
    for key in [k for k, e in buffer.items() if now - e["first"] > UNFINISHED_TTL]:
        entry = buffer.pop(key)
        total = entry["total"]
        text = "".join(entry["parts"].get(str(i), f"[part {i} of {total} missing]") for i in range(1, total + 1))
        out.append((entry["sender"], text, entry.get("sent_at", entry["first"])))
    return out

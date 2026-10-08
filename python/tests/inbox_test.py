"""Incoming gift wraps: text and file messages, worker replies, and what
`unread` shows. No network: wraps are built directly."""
import json
import unittest

from nostr_sdk import EventBuilder, Keys, Kind, Tag, nip59_make_gift_wrap

from myous import contacts, files, inbox, relay
from myous.storage import Storage

BLOB_API = "https://hub.test/blob"


class MemoryStorage(Storage):
    def __init__(self):
        self.key, self.docs, self.log = None, {}, []

    def load_key(self):
        return self.key

    def save_key(self, nsec):
        self.key = nsec

    def get(self, name, default=None):
        return json.loads(json.dumps(self.docs.get(name, default)))

    def put(self, name, value):
        self.docs[name] = json.loads(json.dumps(value))

    def delete(self, name):
        self.docs.pop(name, None)

    def names(self, prefix):
        return sorted(n for n in self.docs if n.startswith(prefix))

    def append_history(self, entry):
        self.log.append(entry)

    def read_history(self):
        return list(self.log)


def wrap(sender: Keys, recipient: Keys, kind: int, content: str, tags: list[list[str]]):
    all_tags = [Tag.public_key(recipient.public_key()), Tag.parse(["ms", "1700000000123"])]
    all_tags += [Tag.parse(t) for t in tags]
    rumor = EventBuilder(Kind(kind), content).tags(all_tags).finalize_unsigned(sender.public_key())
    return nip59_make_gift_wrap(sender, recipient.public_key(), rumor, None, [])


class Inbox(unittest.TestCase):
    def setUp(self):
        self.me, self.peer, self.stranger = Keys.generate(), Keys.generate(), Keys.generate()
        self.st = MemoryStorage()
        self.st.put("hub", {"blob_api": BLOB_API})
        contacts.add(self.st, self.peer.public_key().to_hex(), "peer")

    def handle(self, *wraps):
        return inbox.handle_wraps(self.st, self.me, list(wraps))

    def test_text_file_and_replies(self):
        enc = files.encrypt(b"payload")
        tags = files.file_tags(enc, "doc.txt", "text/plain")
        stored = self.handle(
            wrap(self.peer, self.me, relay.KIND_CHAT, "plain text", []),
            wrap(self.peer, self.me, relay.KIND_FILE, f"{BLOB_API}/{enc.x}", tags),
            wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "result", "id": "a" * 32, "exit": 0,
                                                                   "stdout": "hi\n", "stderr": "", "truncated": False}), []),
            wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "ack", "id": "b" * 32, "ok": False,
                                                                   "error": "nope"}), []),
            wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "exec", "id": "c" * 32, "cmd": "ls"}), []),
            wrap(self.peer, self.me, relay.KIND_FILE, f"{BLOB_API}/{enc.x}", tags + [["w", "file", "d" * 32]]),
        )
        types = [e["type"] for e in stored]
        self.assertEqual(types, ["message", "file", "result", "ack", "message", "file"])
        f = stored[1]
        self.assertEqual((f["name"], f["mime"], f["size"], f["x"], f["ox"]), ("doc.txt", "text/plain", len(enc.ciphertext), enc.x, enc.ox))
        self.assertEqual((f["key"], f["nonce"]), (enc.key.hex(), enc.nonce.hex()))
        self.assertEqual(f["text"], f"file: doc.txt ({len(enc.ciphertext)} bytes)")
        self.assertEqual((stored[2]["id"], stored[2]["exit"], stored[2]["stdout"]), ("a" * 32, 0, "hi\n"))
        self.assertEqual(stored[3]["error"], "nope")
        self.assertEqual(stored[4]["request"], {"myous": "exec", "id": "c" * 32, "cmd": "ls"})
        self.assertEqual(stored[5]["w"], ["file", "d" * 32])

        # The agent sees the text, the plain file and the request; replies are for the waiting command.
        shown = inbox.unread(self.st)
        self.assertEqual([e["type"] for e in shown], ["message", "file", "message"])
        self.assertEqual(inbox.unread(self.st), [])

    def test_strangers_bad_files_and_foreign_stores_are_dropped(self):
        enc = files.encrypt(b"payload")
        tags = files.file_tags(enc, "doc.txt", "text/plain")
        stored = self.handle(
            wrap(self.stranger, self.me, relay.KIND_CHAT, "spam", []),
            wrap(self.peer, self.me, relay.KIND_FILE, f"https://elsewhere.test/{enc.x}", tags),
            wrap(self.peer, self.me, relay.KIND_FILE, f"{BLOB_API}/{enc.x}", [t for t in tags if t[0] != "x"]),
            wrap(self.peer, self.me, relay.KIND_FILE, f"{BLOB_API}/{enc.x}", tags + [["part", "ab", "1", "2"]]),
            wrap(self.peer, self.me, 1, "wrong kind", []),
        )
        self.assertEqual(stored, [])

    def test_reply_fields_cannot_override_the_entry(self):
        other = self.stranger.public_key().to_hex()
        stored = self.handle(
            wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "ack", "id": "a" * 32, "type": "paired",
                                                                   "peer": other, "direction": "out", "alias": "boss",
                                                                   "seq": 0, "at": 0, "sent_at": 0, "ok": True,
                                                                   "exit": True, "size": "big", "extra": 1}), []),
            wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "result", "id": "not hex", "exit": 0}), []),
        )
        self.assertEqual(len(stored), 2)
        e = stored[0]
        self.assertEqual((e["type"], e["peer"], e["direction"], e["alias"]), ("ack", self.peer.public_key().to_bech32(), "in", "peer"))
        self.assertEqual((e["id"], e["ok"]), ("a" * 32, True))
        for k in ("exit", "size", "extra"):
            self.assertNotIn(k, e)  # wrong types and unknown keys are dropped
        self.assertNotEqual(e["sent_at"], 0)
        # An id that isn't 32 hex isn't a reply at all: an ordinary message.
        self.assertEqual(stored[1]["type"], "message")

    def test_old_messages_are_dropped(self):
        from nostr_sdk import Timestamp
        old = Timestamp.from_secs(int(__import__("time").time()) - inbox.MAX_MESSAGE_AGE - 60)
        tags = [Tag.public_key(self.me.public_key())]
        rumor = EventBuilder(Kind(relay.KIND_CHAT), "from the past").tags(tags).custom_created_at(old) \
            .finalize_unsigned(self.peer.public_key())
        stale = nip59_make_gift_wrap(self.peer, self.me.public_key(), rumor, None, [])
        fresh = wrap(self.peer, self.me, relay.KIND_CHAT, "now", [])
        self.assertEqual([e["text"] for e in self.handle(stale, fresh)], ["now"])

    def test_handled_once(self):
        w = wrap(self.peer, self.me, relay.KIND_CHAT, "once", [])
        self.assertEqual(len(self.handle(w)), 1)
        self.assertEqual(self.handle(w), [])

    def test_consumed(self):
        self.assertTrue(inbox.consumed({"type": "result"}))
        self.assertTrue(inbox.consumed({"type": "ack"}))
        self.assertTrue(inbox.consumed({"type": "file", "w": ["file", "x"]}))
        self.assertFalse(inbox.consumed({"type": "file", "w": ["put", "x", "p"]}))
        self.assertFalse(inbox.consumed({"type": "file"}))
        self.assertFalse(inbox.consumed({"type": "message"}))


if __name__ == "__main__":
    unittest.main()

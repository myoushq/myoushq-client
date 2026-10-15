"""Incoming gift wraps: text and file messages, worker replies, and what
`unread` shows. No network: wraps are built directly."""
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest import mock

from nostr_sdk import EventBuilder, Keys, Kind, Tag, nip59_make_gift_wrap

from myous import cli, contacts, files, inbox, processes, relay, vm
from myous.agent import Agent
from myous.muse import watcher
from myous.storage import FileStorage, Storage

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

    def test_cards(self):
        peer_hex = self.peer.public_key().to_hex()
        card = json.dumps({"myous": "card", "name": "peer", "about": "Sam's own Mac; the 'myous browser' is its browser"})
        (e,) = self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, card, []))
        self.assertEqual(e["type"], "card")
        self.assertEqual(e["about"], "Sam's own Mac; the 'myous browser' is its browser")
        self.assertIn("peer describes itself: Sam's own Mac", e["text"])
        self.assertNotIn("calls itself", e["text"])
        self.assertEqual(contacts.load(self.st)[peer_hex]["card"]["about"], "Sam's own Mac; the 'myous browser' is its browser")
        # A rename is announced, never applied: the alias is ours.
        renamed = json.dumps({"myous": "card", "name": "Sam's Mac", "about": "Sam's own Mac; the 'myous browser' is its browser"})
        (e,) = self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, renamed, []))
        self.assertIn('peer now calls itself "Sam\'s Mac"; you call it "peer"', e["text"])
        self.assertIn('myous rename "peer" "Sam\'s Mac"', e["text"])
        self.assertEqual(contacts.load(self.st)[peer_hex]["alias"], "peer")
        self.assertEqual(contacts.load(self.st)[peer_hex]["card"]["name"], "Sam's Mac")
        # The same card again: noted as unchanged. A cleared description is noted too.
        (e,) = self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, renamed, []))
        self.assertIn("unchanged", e["text"])
        (e,) = self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "card", "name": "Sam's Mac", "about": ""}), []))
        self.assertIn("cleared its description", e["text"])
        # Incoming messages carry the contact's description, like the owner's context.
        self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps({"myous": "card", "name": "Sam's Mac", "about": "a Mac"}), []))
        self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, "hello", []))
        unread = inbox.unread(self.st)
        self.assertEqual(unread[-1]["text"], "hello")
        self.assertEqual(unread[-1]["about"], "a Mac")
        # Malformed cards are plain messages: no name, a long name, a long about, wrong types.
        for bad in ({"myous": "card", "about": "x"}, {"myous": "card", "name": "n" * 65, "about": ""},
                    {"myous": "card", "name": "n", "about": "a" * 501}, {"myous": "card", "name": 3, "about": ""}):
            (e,) = self.handle(wrap(self.peer, self.me, relay.KIND_CHAT, json.dumps(bad), []))
            self.assertEqual(e["type"], "message", bad)
        self.assertEqual(contacts.load(self.st)[peer_hex]["card"]["about"], "a Mac")

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

    def test_long_message_parts_in_separate_deliveries(self):
        # A listener hands each wrap over on its own: every part must be
        # saved as it comes, or the message is lost once its wraps are seen.
        texts = ["first ", "second ", "third"]
        wraps = [wrap(self.peer, self.me, relay.KIND_CHAT, t, [["part", "ab12", str(i), "3"]])
                 for i, t in enumerate(texts, 1)]
        for i, w in enumerate(wraps[:2], 1):
            self.assertEqual(self.handle(w), [])
            buffered = self.st.get("partials")[f"{self.peer.public_key().to_hex()}:ab12"]
            self.assertEqual(sorted(buffered["parts"]), [str(n) for n in range(1, i + 1)])
        stored = self.handle(wraps[2])
        self.assertEqual([(e["type"], e["text"]) for e in stored], [("message", "first second third")])
        self.assertNotIn("incomplete", stored[0])
        self.assertEqual(self.st.get("partials"), {})

    def test_long_message_parts_out_of_order_and_in_one_delivery(self):
        texts = ["a" * 10, "b" * 10, "c" * 10]
        wraps = [wrap(self.peer, self.me, relay.KIND_CHAT, t, [["part", "cd34", str(i), "3"]])
                 for i, t in enumerate(texts, 1)]
        self.assertEqual(self.handle(wraps[2]), [])
        self.assertEqual([e["text"] for e in self.handle(wraps[0], wraps[1])], ["".join(texts)])

    def test_consumed(self):
        self.assertTrue(inbox.consumed({"type": "result"}))
        self.assertTrue(inbox.consumed({"type": "ack"}))
        self.assertTrue(inbox.consumed({"type": "file", "w": ["file", "x"]}))
        self.assertFalse(inbox.consumed({"type": "file", "w": ["put", "x", "p"]}))
        self.assertFalse(inbox.consumed({"type": "file"}))
        self.assertFalse(inbox.consumed({"type": "message"}))


class LateReplies(unittest.TestCase):
    """A reply to a request whose command stopped waiting (inbox.give_up)
    is shown once, readably; every other reply stays the command's."""

    def setUp(self):
        self.st = MemoryStorage()
        self.worker = contacts.add(self.st, "a" * 64, "Sam's Mac")["npub"]
        self.other = contacts.add(self.st, "b" * 64, "other")["npub"]
        self.sent = int(time.time()) - 120
        self.clock = time.strftime("%H:%M", time.localtime(self.sent))

    def give_up(self, rid: str, **request) -> None:
        inbox.give_up(self.st, rid, dict(request, peer=self.worker, alias="Sam's Mac", sent_at=self.sent,
                                         expires=self.sent + 3600))

    def reply(self, kind: str, rid: str, peer: str | None = None, **fields) -> dict:
        """A worker's reply, stored as handle_wraps stores it."""
        entry = dict(fields, type=kind, direction="in", peer=peer or self.worker, alias="Sam's Mac")
        if kind == "file":
            entry.update(w=["file", rid], text="file: " + inbox.describe_file(entry))
        else:
            entry.update(id=rid, text=json.dumps(dict(fields, myous=kind, id=rid)))
        return inbox.record(self.st, entry)

    def incoming(self, **entry) -> dict:
        """Anything else the worker sent."""
        return inbox.record(self.st, dict(entry, direction="in", peer=self.worker, alias="Sam's Mac"))

    def printed(self, entries: list[dict]) -> str:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cli._print_entries(entries)
        return out.getvalue()

    def test_a_late_result_is_shown_once_and_readably(self):
        self.give_up("1" * 32, op="exec", cmd="make test")
        self.reply("result", "1" * 32, exit=2, stdout="built\n", stderr="1 failed\n", truncated=True)
        entries = inbox.unread(self.st)
        self.assertEqual(len(entries), 1)
        e = entries[0]
        self.assertEqual(e["late"], {"op": "exec", "cmd": "make test", "sent_at": self.sent})
        self.assertEqual(e["text"], f"late result of exec `make test` (sent {self.clock}): exit 2\nbuilt\n"
                                    "stderr:\n1 failed\n(the output was cut to fit the message)")
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(e["at"]))
        self.assertEqual(self.printed(entries), f"[{when}] Sam's Mac: {e['text']}\n")
        self.assertEqual(watcher.describe(e), f"[{when}] Sam's Mac: {e['text']}")
        self.assertEqual(inbox.unread(self.st), [])   # once
        self.assertEqual(self.st.read_history()[-1]["text"][0], "{")   # the history keeps the reply as it came

    def test_inbox_json_carries_late(self):
        self.give_up("1" * 32, op="exec", cmd="ls")
        self.reply("result", "1" * 32, exit=0, stdout="a\n", stderr="", truncated=False)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cli.cmd_inbox(Agent(self.st), self.st, SimpleNamespace(local=True, peek=False, json=True))
        [e] = json.loads(out.getvalue())
        self.assertEqual((e["type"], e["late"]["op"], e["late"]["cmd"], e["exit"]), ("result", "exec", "ls", 0))
        self.assertEqual(e["text"], f"late result of exec `ls` (sent {self.clock}): exit 0\na")

    def test_an_expired_answer_stays_hidden_a_refusal_shows(self):
        self.give_up("1" * 32, op="exec", cmd="make test", ttl=150, wait=150)
        self.give_up("2" * 32, op="exec", cmd="rm -rf build", ttl=150, wait=150)
        self.reply("ack", "1" * 32, ok=False, error="expired: sent 4000 seconds ago, its ttl is 150 seconds; not run")
        self.reply("ack", "2" * 32, ok=False, error="refused by the owner")
        self.assertEqual([e["text"] for e in inbox.unread(self.st)],
                         [f"late answer to exec `rm -rf build` (sent {self.clock}): refused: refused by the owner"])

    def test_an_expired_answer_shows_when_the_ttl_outlived_the_wait(self):
        # The timeout said the worker might still run it until its ttl ran out.
        self.give_up("1" * 32, op="exec", cmd="make test", ttl=3600, wait=150)
        self.give_up("2" * 32, op="exec", cmd="make lint")   # from before ttl and wait were kept: hidden
        why = "expired: sent 2 hours ago, its ttl is 3600 seconds; not run"
        self.reply("ack", "1" * 32, ok=False, error=why)
        self.reply("ack", "2" * 32, ok=False, error=why)
        self.assertEqual([e["text"] for e in inbox.unread(self.st)],
                         [f"late answer to exec `make test` (sent {self.clock}): {why}"])

    def test_a_reply_its_command_still_waits_for_stays_the_commands(self):
        inbox.waiting(self.st, "1" * 32, {"op": "exec", "cmd": "make test", "peer": self.worker, "alias": "Sam's Mac",
                                          "sent_at": self.sent, "expires": self.sent + 3600})
        self.assertEqual(self.st.get("gave_up")["1" * 32]["pid"], os.getpid())
        self.reply("result", "1" * 32, exit=0, stdout="", stderr="")
        self.assertEqual(inbox.unread(self.st), [])
        inbox.answered(self.st, "1" * 32)   # the command took it
        self.assertEqual(self.st.get("gave_up"), {})

    def test_a_reply_whose_command_was_killed_shows(self):
        gone = subprocess.Popen([sys.executable, "-c", "pass"])
        gone.wait()
        started = processes.created_at(os.getpid())
        self.give_up("1" * 32, op="exec", cmd="make test", pid=gone.pid)
        # This process's pid, but a process that started earlier: another one, gone, had it.
        self.give_up("2" * 32, op="exec", cmd="make lint", pid=os.getpid(), pid_started=started - 3600)
        self.reply("result", "1" * 32, exit=0, stdout="", stderr="")
        self.reply("result", "2" * 32, exit=1, stdout="", stderr="")
        self.assertEqual([e["text"] for e in inbox.unread(self.st)],
                         [f"late result of exec `make test` (sent {self.clock}): exit 0",
                          f"late result of exec `make lint` (sent {self.clock}): exit 1"])

    def test_only_the_worker_asked_may_answer(self):
        self.give_up("1" * 32, op="exec", cmd="make test")
        self.give_up("2" * 32, op="get", remote="/work/out.txt", local="out.txt")
        self.reply("result", "1" * 32, peer=self.other, exit=0, stdout="", stderr="")
        self.reply("file", "2" * 32, peer=self.other, name="out.txt", size=3)
        self.assertEqual(inbox.unread(self.st), [])

    def test_a_reply_nobody_gave_up_on_stays_consumed(self):
        self.give_up("1" * 32, op="exec", cmd="make test")
        self.reply("result", "3" * 32, exit=0, stdout="mine", stderr="")
        self.reply("ack", "3" * 32, ok=True, path="/work/x", size=1)
        self.reply("file", "3" * 32, name="x", size=1)
        self.assertEqual(inbox.unread(self.st), [])
        gave_up = self.st.get("gave_up")
        self.assertTrue(all(inbox.consumed(e, gave_up) for e in self.st.read_history()))

    def test_late_put_and_get(self):
        self.give_up("4" * 32, op="put", local="report.pdf", remote="/work/report.pdf")
        self.give_up("5" * 32, op="get", remote="/work/out.txt", local="out.txt")
        self.reply("ack", "4" * 32, ok=True, path="/work/report.pdf", size=5, sha256="00")
        got = self.reply("file", "5" * 32, name="out.txt", size=123)
        put, get = inbox.unread(self.st)
        self.assertEqual(put["late"], {"op": "put", "local": "report.pdf", "remote": "/work/report.pdf",
                                       "sent_at": self.sent})
        self.assertEqual(put["text"], f"late answer to cp of report.pdf to /work/report.pdf (sent {self.clock}): "
                                      "written: /work/report.pdf (5 bytes)")
        self.assertEqual(get["late"], {"op": "get", "remote": "/work/out.txt", "local": "out.txt", "sent_at": self.sent})
        self.assertEqual(get["text"], f"late reply to cp of /work/out.txt (sent {self.clock}): out.txt (123 bytes)")
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(got["at"]))
        n = got["seq"]
        self.assertEqual(self.printed([get]), f"[{when}] file from Sam's Mac: late reply to cp of /work/out.txt "
                                              f"(sent {self.clock}): out.txt (123 bytes, id {n}; `myous fetch {n}`)\n")
        self.assertIn(f"file from Sam's Mac: late reply to cp of /work/out.txt (sent {self.clock}): out.txt",
                      watcher.describe(get))

    def history(self, json_out: bool = False) -> str:
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            cli.cmd_history(Agent(self.st), self.st, SimpleNamespace(with_=None, limit=50, json=json_out))
        return out.getvalue()

    def out(self, **entry) -> dict:
        """A request (or message) this agent sent to the worker."""
        return inbox.record(self.st, dict(entry, direction="out", peer=self.worker, alias="Sam's Mac"))

    def test_history_shows_requests_and_replies_readably(self):
        def line(e: dict, text: str) -> str:
            return f"[{time.strftime('%Y-%m-%d %H:%M', time.localtime(e['at']))}] {text}"

        exec_ = json.dumps({"myous": "exec", "id": "1" * 32, "cmd": "make test", "timeout": 120, "ttl": 150})
        get = json.dumps({"myous": "get", "id": "2" * 32, "path": "/work/out.txt", "ttl": 300})
        self.give_up("5" * 32, op="exec", cmd="sleep 900")
        entries = [
            self.out(type="message", text=exec_),
            self.reply("result", "1" * 32, exit=0, stdout="ok\n", stderr="", truncated=False),
            self.out(type="message", text=get),
            self.reply("file", "2" * 32, name="out.txt", size=3),
            self.out(type="file", name="in.pdf", size=5, w=["put", "3" * 32, "/work/in.pdf", "300"],
                      text="sent file: in.pdf (5 bytes)"),
            self.reply("ack", "3" * 32, ok=True, path="/work/in.pdf", size=5, sha256="00"),
            self.reply("ack", "4" * 32, ok=False, error="refused by the owner"),
            self.reply("result", "5" * 32, exit=0, stdout="", stderr=""),
            self.out(type="message", text="hello"),
            self.out(type="file", name="notes.txt", size=9, text="sent file: notes.txt (9 bytes)"),
        ]
        n = entries[3]["seq"]
        self.assertEqual(self.history(), "\n".join([
            line(entries[0], "me -> Sam's Mac: exec `make test` (request 11111111)"),
            line(entries[1], "Sam's Mac: result of request 11111111: exit 0\nok"),
            line(entries[2], "me -> Sam's Mac: cp of /work/out.txt (request 22222222)"),
            line(entries[3], f"file from Sam's Mac: reply to request 22222222: out.txt (3 bytes, id {n}; `myous fetch {n}`)"),
            line(entries[4], "file me -> Sam's Mac: cp of in.pdf to /work/in.pdf (5 bytes, request 33333333)"),
            line(entries[5], "Sam's Mac: answer to request 33333333: written: /work/in.pdf (5 bytes)"),
            line(entries[6], "Sam's Mac: answer to request 44444444: refused: refused by the owner"),
            # A late reply exactly as the inbox shows it.
            line(entries[7], f"Sam's Mac: late result of exec `sleep 900` (sent {self.clock}): exit 0"),
            line(entries[8], "me -> Sam's Mac: hello"),
            line(entries[9], "file me -> Sam's Mac: notes.txt (9 bytes)"),
        ]) + "\n")
        [late] = inbox.unread(self.st)   # the inbox shows the late one the same way
        self.assertEqual(self.printed([late]), line(entries[7], f"Sam's Mac: {late['text']}") + "\n")
        # --json keeps the entries as stored.
        stored = json.loads(self.history(json_out=True))
        self.assertEqual([e["text"] for e in stored], [e["text"] for e in entries])
        self.assertNotIn("late", stored[7])

    def test_poll_counts_and_lists_what_the_inbox_shows(self):
        self.give_up("1" * 32, op="exec", cmd="make test")
        new = [self.reply("result", "3" * 32, exit=0, stdout="mine", stderr=""),   # a waiting command's
               self.reply("result", "1" * 32, exit=0, stdout="", stderr=""),       # late
               self.incoming(type="file", name="notes.txt", size=9, key="00", nonce="00", text="file: notes.txt (9 bytes)"),
               self.incoming(type="message", text="hi")]
        agent = Agent(self.st)

        async def poll():
            return new

        def run(**args) -> str:
            out = io.StringIO()
            with mock.patch.object(agent, "poll", poll), contextlib.redirect_stdout(out):
                cli.cmd_poll(agent, self.st, SimpleNamespace(**dict(dict(json=False, quiet=False), **args)))
            return out.getvalue()

        self.assertEqual(run(), "3 new item(s); read them with `myous inbox`\n")
        late, file, message = json.loads(run(json=True))
        self.assertEqual((late["late"]["cmd"], late["text"]), ("make test", f"late result of exec `make test` (sent {self.clock}): exit 0"))
        self.assertEqual((file["name"], message["text"]), ("notes.txt", "hi"))
        self.assertNotIn("key", file)   # as in inbox --json: the key stays in the history
        self.assertEqual([e["text"] for e in inbox.unread(self.st)], [late["text"], file["text"], "hi"])

    def test_give_up_keeps_requests_a_day_past_their_ttl(self):
        now = int(time.time())
        self.st.put("gave_up", {"old": {"expires": now - inbox.GAVE_UP_KEEP - 5}, "recent": {"expires": now - 3600}})
        inbox.give_up(self.st, "new", {"op": "exec", "expires": now + 60})
        self.assertEqual(sorted(self.st.get("gave_up")), ["new", "recent"])

    def test_the_wake_hook_runs_for_a_late_reply_not_for_a_consumed_one(self):
        with tempfile.TemporaryDirectory() as home:
            st = FileStorage(home)
            st.put("settings", {"on_message": "true"})
            st.put("gave_up", {"1" * 32: {"peer": self.worker}})
            consumed = {"type": "result", "id": "3" * 32, "peer": self.worker}
            late = {"type": "result", "id": "1" * 32, "peer": self.worker}
            with mock.patch.object(vm.processes, "spawn_background") as spawn:
                vm.run_hook(st, [consumed])
                spawn.assert_not_called()
                vm.run_hook(st, [consumed, late, {"type": "message"}])
                self.assertEqual(spawn.call_args.kwargs["env"]["MYOUS_NEW"], "2")
                spawn.call_args.kwargs["stdout"].close()   # the hook's log, which the child would keep


if __name__ == "__main__":
    unittest.main()

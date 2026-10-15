"""FileStorage's history: read from the end for the hot paths (unread, a
reply being waited for, the latest file, the last entries), with the same
answers as reading it all."""
import json
import random
import tempfile
import unittest
from pathlib import Path

try:
    from inbox_test import MemoryStorage  # unittest discover puts tests/ on the path
except ModuleNotFoundError:
    from tests.inbox_test import MemoryStorage
from myous import contacts, inbox
from myous.agent import Agent
from myous.storage import FileStorage

PEERS = ["npub1alice", "npub1bob", "npub1carol"]


class CopyingMemoryStorage(MemoryStorage):
    """As a store on disk: what is read is a copy (unread() adds the
    contact's context to what it returns)."""

    def read_history(self):
        return json.loads(json.dumps(self.log))


def entries(n, seed=1):
    rnd = random.Random(seed)
    out = []
    for seq in range(1, n + 1):
        kind = rnd.choice(["message", "message", "result", "file", "card"])
        e = {"seq": seq, "at": 1760000000 + seq, "type": kind, "direction": rnd.choice(["in", "in", "out"]),
             "peer": rnd.choice(PEERS), "alias": "x",
             # multi-byte text, so lines straddle block boundaries mid-character
             "text": "é✓" * rnd.randint(0, 40) + "\n" * rnd.randint(0, 1)}
        if kind == "result":
            e["id"] = rnd.choice(["a" * 32, "b" * 32, "c" * 32])
        out.append(e)
    return out


class HistoryFromTheEnd(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.st = FileStorage(Path(self.tmp.name))

    def tearDown(self):
        self.tmp.cleanup()

    def fill(self, es):
        # As append_history writes them (one sorted JSON object per line), without an fsync each.
        with open(self.st.path("messages.jsonl"), "a") as f:
            f.writelines(json.dumps(e, sort_keys=True) + "\n" for e in es)

    def test_reversed_and_since_match_reading_it_all(self):
        es = entries(300)
        self.fill(es)
        for block in (256, 4096, 64 * 1024):
            self.st.HISTORY_BLOCK = block
            self.assertEqual(list(self.st.history_reversed()), es[::-1], block)
            for since in (0, 1, 2, 150, 299, 300, 301, 1000):
                self.assertEqual(self.st.read_history_since(since), [e for e in es if e["seq"] >= since],
                                 (block, since))
        self.st.HISTORY_BLOCK = 1   # a byte at a time: every line straddles blocks
        newest = self.st.history_reversed()
        self.assertEqual([next(newest) for _ in range(3)], es[::-1][:3])

    def test_empty_and_missing(self):
        self.assertEqual(list(self.st.history_reversed()), [])
        self.assertEqual(self.st.read_history_since(1), [])
        (Path(self.tmp.name) / "messages.jsonl").write_text("\n\n")
        self.assertEqual(list(self.st.history_reversed()), [])

    def test_the_agent_answers_as_before(self):
        """unread, history, file_entry and _find_reply on FileStorage give
        what the same history gives through the read-it-all defaults."""
        mem = CopyingMemoryStorage()
        for st in (self.st, mem):
            for i, npub in enumerate(PEERS):
                contacts.add(st, f"{i:064x}", npub[5:])
                c = st.get("contacts")
                c[f"{i:064x}"]["npub"] = npub
                st.put("contacts", c)
        es = entries(400, seed=7)
        self.fill(es)
        mem.log = json.loads(json.dumps(es))
        for st in (self.st, mem):
            st.put("state", {"next_seq": 401, "read_seq": 350})
        self.st.HISTORY_BLOCK = 2048
        fast, slow = Agent(self.st), Agent(mem)
        self.assertEqual(fast.unread(mark_read=False), slow.unread(mark_read=False))
        for contact in (None, "bob"):
            for limit in (1, 50, 1000, 0):
                self.assertEqual(fast.history(contact, limit), slow.history(contact, limit), (contact, limit))
        self.assertEqual(fast.file_entry(None), slow.file_entry(None))
        files = [e for e in es if e["type"] == "file" and e["direction"] == "in"]
        for e in (files[0], files[len(files) // 2], files[-1]):
            self.assertEqual(fast.file_entry(e["seq"]), e)
            self.assertEqual(fast.file_entry(str(e["seq"])), e)
        not_a_file = next(e for e in es if e["type"] != "file")
        for which in (not_a_file["seq"], 10_000, "abc"):
            with self.assertRaises(KeyError):
                fast.file_entry(which)
        for rid in ("a" * 32, "b" * 32, "d" * 32):
            for since in (1, 200, 390):
                for npub in PEERS:
                    self.assertEqual(fast._find_reply(rid, since, npub), slow._find_reply(rid, since, npub))
        self.assertEqual(fast._entries_since(390), slow._entries_since(390))
        self.assertEqual(fast.unread(), slow.unread())   # and marks the same as read
        self.assertEqual(self.st.get("state")["read_seq"], mem.get("state")["read_seq"])

    def test_a_reply_is_looked_for_from_before_the_request_went_out(self):
        st = MemoryStorage()
        contacts.add(st, "d" * 64, "box")
        npub = contacts.find(st, "box")[1]["npub"]
        with st.lock():
            inbox.record(st, {"type": "message", "direction": "in", "peer": npub, "alias": "box", "text": "old"})
        agent = Agent(st)
        seen = {}

        class Conn:
            async def __aenter__(self):
                return self

            async def __aexit__(self, *exc):
                return None

        async def send(name, text, conn=None):
            with st.lock():   # the request goes in the history, as Agent.send records it
                inbox.record(st, {"type": "message", "direction": "out", "peer": npub, "alias": "box", "text": text})

        async def await_reply(rid, timeout, npub, conn=None, expires=None, since=1):
            seen["since"] = since
            return {"type": "result", "exit": 0}

        agent._connect = lambda cfg=None: Conn()
        agent.send = send
        agent._await_reply = await_reply
        import asyncio
        asyncio.run(agent.exec("box", "ls"))
        self.assertEqual(seen["since"], 2)   # the request itself is seq 2: anything from there on counts


class TornHistory(unittest.TestCase):
    """A write cut short (a crash, a full disk) leaves the history's last
    line torn: it is skipped, and the next append mends the file, instead
    of every read failing from then on."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.st = FileStorage(Path(self.tmp.name))
        self.file = Path(self.tmp.name) / "messages.jsonl"
        self.st.append_history({"seq": 1, "type": "message", "text": "whole"})

    def tearDown(self):
        self.tmp.cleanup()

    def seqs(self):
        return [e["seq"] for e in self.st.read_history()], [e["seq"] for e in self.st.history_reversed()]

    def test_a_torn_last_line_is_skipped_and_mended(self):
        with open(self.file, "a") as f:
            f.write('{"seq": 2, "type": "mess')
        self.assertEqual(self.seqs(), ([1], [1]))
        for block in (1, 5, 64 * 1024):
            self.st.HISTORY_BLOCK = block
            self.assertEqual([e["seq"] for e in self.st.history_reversed()], [1])
        self.st.append_history({"seq": 2, "type": "message", "text": "next"})
        self.assertEqual(self.seqs(), ([1, 2], [2, 1]))
        self.assertTrue(self.file.read_text().endswith('"text": "next", "type": "message"}\n'))

    def test_a_whole_entry_without_its_newline_is_kept(self):
        with open(self.file, "a") as f:
            f.write('{"seq": 2, "type": "message"}')
        self.assertEqual(self.seqs(), ([1, 2], [2, 1]))
        self.st.append_history({"seq": 3, "type": "message"})
        self.assertEqual(self.seqs(), ([1, 2, 3], [3, 2, 1]))

    def test_only_a_fragment(self):
        self.file.write_text('{"seq": 1, "ty')
        self.assertEqual(self.seqs(), ([], []))
        self.st.append_history({"seq": 1, "type": "message"})
        self.assertEqual(self.seqs(), ([1], [1]))

    def test_lines_end_with_newline_only(self):
        # Go and TypeScript leave U+0085 (and TypeScript U+2028) unescaped;
        # splitlines() would cut such a line in two.
        with open(self.file, "a", encoding="utf-8") as f:
            f.write(json.dumps({"seq": 2, "type": "message", "text": "a b\u0085c d"}, ensure_ascii=False) + "\n")
        self.assertEqual(self.seqs(), ([1, 2], [2, 1]))
        self.assertEqual(self.st.read_history()[-1]["text"], "a b\u0085c d")


if __name__ == "__main__":
    unittest.main()

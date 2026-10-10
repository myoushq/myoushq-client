"""The Agent's bookkeeping around listen() and worker replies, without a
relay: delivery happens once, and only the right contact can answer."""
import asyncio
import unittest

try:
    from inbox_test import MemoryStorage  # unittest discover puts tests/ on the path
except ModuleNotFoundError:
    from tests.inbox_test import MemoryStorage
from myous import contacts, inbox
from myous.agent import Agent, _Deliveries


class Cards(unittest.TestCase):
    """Which contacts are told this agent's card, and when."""

    def setUp(self):
        self.st = MemoryStorage()
        self.st.put("settings", {"alias": "Max's Muse"})
        self.agent = Agent(self.st)
        contacts.add(self.st, "a" * 64, "old friend")                       # from before cards: nothing recorded
        contacts.add(self.st, "b" * 64, "new friend")
        contacts.peer_knows(self.st, "b" * 64, "Max's Muse", "")           # paired now: knows the alias
        contacts.add(self.st, "c" * 64, "blocked one")
        contacts.update(self.st, "blocked one", status="blocked")

    def due(self):
        return sorted(c["alias"] for c in self.agent.cards_due())

    def test_no_card_nothing_to_say(self):
        self.assertEqual(self.due(), [])

    def test_a_card_goes_to_everyone_once(self):
        self.agent.set_card("Max's own assistant")
        self.assertEqual(self.due(), ["new friend", "old friend"])
        contacts.peer_knows(self.st, "b" * 64, "Max's Muse", "Max's own assistant")
        self.assertEqual(self.due(), ["old friend"])

    def test_a_rename_is_announced_to_those_who_knew_the_old_name(self):
        self.st.put("settings", {"alias": "Max's Assistant"})
        self.assertEqual(self.due(), ["new friend"])   # the old friend never recorded a name; nothing to correct

    def test_limits(self):
        with self.assertRaises(ValueError):
            self.agent.set_card("x" * 501)
        self.assertEqual(self.agent.set_card("  spaced  "), "spaced")
        self.assertEqual(self.agent.set_card(None), "")


class Deliveries(unittest.TestCase):
    def setUp(self):
        self.st = MemoryStorage()
        self.agent = Agent(self.st)

    def test_each_entry_is_delivered_once_even_when_both_paths_see_it(self):
        seen = []

        async def slow_on_new(entries):
            seen.append([e["seq"] for e in entries])
            await asyncio.sleep(0.05)  # still busy when the other path looks

        d = _Deliveries(self.agent, slow_on_new)
        with self.st.lock():
            inbox.record(self.st, {"type": "message", "direction": "in", "peer": "p", "alias": "a", "text": "one"})
            inbox.record(self.st, {"type": "message", "direction": "out", "peer": "p", "alias": "a", "text": "mine"})
        async def both():
            await asyncio.gather(d.deliver(), d.deliver())  # the stream and housekeeping, at once

        asyncio.run(both())
        self.assertEqual(seen, [[1]])
        with self.st.lock():
            inbox.record(self.st, {"type": "message", "direction": "in", "peer": "p", "alias": "a", "text": "two"})
        asyncio.run(d.deliver())
        self.assertEqual(seen, [[1], [3]])

    def test_replies_only_count_from_the_contact_asked(self):
        rid = "c" * 32
        with self.st.lock():
            inbox.record(self.st, {"type": "ack", "direction": "in", "peer": "npub1other", "alias": "o", "id": rid, "ok": True})
            inbox.record(self.st, {"type": "file", "direction": "in", "peer": "npub1other", "alias": "o", "w": ["file", rid]})
        self.assertIsNone(self.agent._find_reply(rid, 1, "npub1worker"))
        with self.st.lock():
            inbox.record(self.st, {"type": "ack", "direction": "in", "peer": "npub1worker", "alias": "w", "id": rid, "ok": False, "error": "no"})
        self.assertEqual(self.agent._find_reply(rid, 1, "npub1worker")["error"], "no")


if __name__ == "__main__":
    unittest.main()

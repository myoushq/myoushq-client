"""Fetching every stored gift wrap, a page at a time, from a relay that
answers as the hub's store does (newest first, at most a page)."""
import asyncio
import json
import random
import unittest
from types import SimpleNamespace
from unittest import mock

from nostr_sdk import Keys

from myous import relay


class FakeEvent:
    def __init__(self, n: int, created: int, kind: int = relay.KIND_GIFT_WRAP):
        self.n, self.created, self.k = n, created, kind

    def id(self):
        return SimpleNamespace(to_hex=lambda: f"{self.n:064x}")

    def created_at(self):
        return SimpleNamespace(as_secs=lambda: self.created)

    def kind(self):
        return SimpleNamespace(as_u16=lambda: self.k)


class FakeRelay:
    """Holds events; a query gets at most its limit (250 without one,
    nothing for "limit": 0), newest first, none newer than its until."""

    def __init__(self, events):
        self.events = sorted(events, key=lambda e: -e.created)
        self.queries, self.subscriptions = [], []
        self.live: asyncio.Queue | None = None
        self.up = True

    async def relays(self):
        status = relay.RelayStatus.CONNECTED if self.up else relay.RelayStatus.DISCONNECTED
        return {"ws://127.0.0.1:9": SimpleNamespace(status=lambda: status)}

    @staticmethod
    def filters(target):
        return [json.loads(f.as_json()) for fs in target.values() for f in fs]

    async def fetch_events(self, target, timeout):
        (f,) = self.filters(target)
        self.queries.append(f)
        limit = f.get("limit", 250)
        until = f.get("until")
        return [e for e in self.events if until is None or e.created <= until][:limit]

    def notifications(self):
        self.live = asyncio.Queue()
        relay_ = self

        class Notes:
            async def next(self):
                return await relay_.live.get()
        return Notes()

    async def subscribe(self, target):
        self.subscriptions.extend(self.filters(target))


def connection(events):
    conn = relay.Connection(Keys.generate(), ["ws://127.0.0.1:9"])
    conn.client = FakeRelay(events)
    return conn


def stored(n, seconds=2 * 86400, seed=3):
    rnd = random.Random(seed)
    return [FakeEvent(i, 1_760_000_000 - rnd.randrange(seconds)) for i in range(n)]


class FetchWraps(unittest.TestCase):
    def setUp(self):
        patcher = mock.patch.object(relay.ReqTarget, "manual", staticmethod(lambda d: d))
        patcher.start()
        self.addCleanup(patcher.stop)

    def fetch(self, conn):
        return sorted(e.n for e in asyncio.run(conn.fetch_wraps()))

    def test_more_than_a_page(self):
        for n in (0, 1, 499, 500, 501, 1234, 2600):
            conn = connection(stored(n))
            self.assertEqual(self.fetch(conn), list(range(n)), n)
            pages = conn.client.queries
            self.assertTrue(all(q["limit"] == relay.FETCH_PAGE for q in pages))
            self.assertLessEqual(len(pages), n // relay.FETCH_PAGE + 2, n)

    def test_many_in_the_same_second(self):
        # until takes in the oldest second again: what was seen is not counted twice
        events = stored(700, seconds=3)
        self.assertEqual(self.fetch(connection(events)), list(range(700)))

    def test_a_page_with_nothing_new_ends_it(self):
        # More than a page in one second can't be paged through with until:
        # it stops instead of asking for the same page for ever.
        conn = connection([FakeEvent(i, 1_760_000_000) for i in range(600)])
        self.assertEqual(len(self.fetch(conn)), relay.FETCH_PAGE)
        self.assertEqual(len(conn.client.queries), 2)

    def test_an_unreachable_relay_is_an_error_not_an_empty_inbox(self):
        conn = connection(stored(3))
        conn.client.up = False
        with self.assertRaises(relay.RelayError):
            asyncio.run(conn.fetch_wraps())
        self.assertEqual(conn.client.queries, [])

    def test_the_stream_hands_over_everything_stored_then_what_arrives(self):
        conn = connection(stored(900))

        async def run():
            batches = []
            stream = conn.stream_wraps()
            batches.append(await stream.__anext__())
            self.assertEqual(conn.client.subscriptions[0]["limit"], 0)   # new ones only: the stored come from fetch_wraps
            # The fetched ones come through the notifications too (nostr-sdk
            # passes on what a fetch gets): they are not handed over again.
            for e in (conn.client.events[0], conn.client.events[-1], FakeEvent(5000, 1)):
                await conn.client.live.put(SimpleNamespace(is_shutdown=lambda: False, is_new_event=lambda: True,
                                                           event=e))
            batches.append(await stream.__anext__())
            await conn.client.live.put(None)
            with self.assertRaises(StopAsyncIteration):
                await stream.__anext__()
            return batches

        batches = asyncio.run(run())
        self.assertEqual(sorted(e.n for e in batches[0]), list(range(900)))
        self.assertEqual([e.n for e in batches[1]], [5000])


if __name__ == "__main__":
    unittest.main()

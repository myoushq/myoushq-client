"""FileStorage.put from several processes at once (no lock: the hub's
settings are refreshed by whichever command finds them stale)."""
import multiprocessing
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from myous.storage import FileStorage


def _writer(home, n, q):
    st, failed = FileStorage(home), 0
    for i in range(n):
        try:
            st.put("hub", {"writer": os.getpid(), "i": i, "pad": "x" * 4000})
        except OSError:
            failed += 1
    q.put(("failed puts", failed))


def _reader(home, n, q):
    st, bad = FileStorage(home), 0
    for _ in range(n):
        try:
            st.get("hub")
        except ValueError:
            bad += 1
    q.put(("invalid reads", bad))


class ConcurrentPuts(unittest.TestCase):
    def test_two_writers_and_a_reader(self):
        with tempfile.TemporaryDirectory() as home:
            FileStorage(home).put("hub", {})
            q = multiprocessing.get_context("spawn").Queue()
            ctx = multiprocessing.get_context("spawn")
            procs = [ctx.Process(target=_writer, args=(home, 300, q)) for _ in range(2)]
            procs.append(ctx.Process(target=_reader, args=(home, 600, q)))
            for p in procs:
                p.start()
            for p in procs:
                p.join(120)
            results = sorted(q.get(timeout=10) for _ in procs)
            self.assertEqual(results, [("failed puts", 0), ("failed puts", 0), ("invalid reads", 0)])
            self.assertEqual([f for f in os.listdir(home) if f.endswith(".tmp")], [])

    def test_a_failed_write_leaves_nothing_behind(self):
        with tempfile.TemporaryDirectory() as home:
            st = FileStorage(home)
            st.put("state", {"next_seq": 1})
            with mock.patch("os.replace", side_effect=OSError("disk full")), self.assertRaises(OSError):
                st.put("state", {"next_seq": 2})
            self.assertEqual(st.get("state"), {"next_seq": 1})
            self.assertEqual(sorted(os.listdir(home)), ["state.json"])


if __name__ == "__main__":
    unittest.main()

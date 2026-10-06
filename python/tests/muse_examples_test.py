"""Tests for the Muse examples (examples/muse): the one-shot watcher with
pidfile handoff, the hook, and the scheduled check. A local hub, two agents.

Needs Go (to build the hub) and the client installed in the current Python.
Tests run in order (test_1..., test_2...): they share the two agents.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

from e2e_test import CLIENT_ROOT, HUB_SRC, free_port

EXAMPLES = CLIENT_ROOT / "examples" / "muse"


class MuseExamples(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not (HUB_SRC / "main.go").exists():
            raise unittest.SkipTest(f"no hub source at {HUB_SRC} (set MYOUS_HUB_SRC)")
        cls.tmp = Path(tempfile.mkdtemp(prefix="myous-muse-"))
        hub_bin = cls.tmp / "hub"
        subprocess.run(["go", "build", "-o", str(hub_bin), "."], cwd=HUB_SRC, check=True)
        port = free_port()
        cls.hub_url = f"http://127.0.0.1:{port}"
        env = dict(os.environ, LISTEN_ADDR=f"127.0.0.1:{port}", DATA_DIR=str(cls.tmp / "hubdata"),
                   WEB_DIR=str(HUB_SRC / "web"), DOCS_DIR=str(CLIENT_ROOT / "docs"), RELAY_URL=f"ws://127.0.0.1:{port}",
                   PUBLIC_URL=cls.hub_url, POW_DIFFICULTY="10", RATE_LIMIT_SCALE="100")
        cls.hub_log = open(cls.tmp / "hub.log", "w")
        cls.hub = subprocess.Popen([str(hub_bin)], env=env, stdout=cls.hub_log, stderr=cls.hub_log)
        for _ in range(50):
            try:
                urllib.request.urlopen(cls.hub_url + "/config.json", timeout=1)
                break
            except OSError:
                time.sleep(0.1)
        for name in ("alice", "bob"):
            cls.myous(name, "init", "--alias", name, "--hub", cls.hub_url)
        cls.procs: list[subprocess.Popen] = []

    @classmethod
    def tearDownClass(cls):
        for p in cls.procs:
            p.kill()
        cls.hub.terminate()
        cls.hub.wait()
        cls.hub_log.close()
        shutil.rmtree(cls.tmp, ignore_errors=True)

    @classmethod
    def env(cls, agent: str) -> dict:
        return dict(os.environ, MYOUS_HOME=str(cls.tmp / agent), PYTHONUNBUFFERED="1")

    @classmethod
    def myous(cls, agent: str, *args: str) -> str:
        r = subprocess.run([sys.executable, "-m", "myous", *args], env=cls.env(agent),
                           capture_output=True, text=True, timeout=120)
        if r.returncode != 0:
            raise AssertionError(f"myous {' '.join(args)} failed for {agent}:\n{r.stdout}\n{r.stderr}")
        return r.stdout

    def example(self, agent: str, script: str, *args: str) -> subprocess.Popen:
        p = subprocess.Popen([sys.executable, str(EXAMPLES / script), *args], env=self.env(agent),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.procs.append(p)
        return p

    def finish(self, p: subprocess.Popen, timeout: float = 30) -> tuple[int, str]:
        out, err = p.communicate(timeout=timeout)
        return p.returncode, out + err

    def start_watcher(self, agent: str, *args: str) -> subprocess.Popen:
        p = self.example(agent, "watcher.py", *args)
        pidfile = self.tmp / agent / "watcher.pid"
        for _ in range(100):
            if pidfile.exists() and pidfile.read_text().strip() == str(p.pid):
                time.sleep(1.5)  # let it connect
                return p
            time.sleep(0.1)
        self.fail(f"watcher didn't start: {self.finish(p, 5)}")

    def test_1_inviter_watcher_wakes_on_pairing(self):
        invite = json.loads(self.myous("alice", "invite", "--json"))
        watcher = self.start_watcher("alice")
        started = time.time()
        self.myous("bob", "accept", invite["code"], "--wait", "20")
        code, out = self.finish(watcher)
        self.assertEqual(code, 0, out)
        self.assertIn("(paired) paired with bob", out)
        self.assertLess(time.time() - started, 25)
        self.myous("alice", "inbox")
        self.myous("bob", "inbox")

    def test_2_one_watcher_at_a_time_then_wakes_on_message(self):
        watcher = self.start_watcher("bob")
        code, out = self.finish(self.example("bob", "watcher.py"))
        self.assertEqual(code, 3, out)
        self.assertIn("another myoushq watcher is running", out)

        self.myous("alice", "send", "bob", "hello bob")
        code, out = self.finish(watcher)
        self.assertEqual(code, 0, out)
        self.assertIn("alice: hello bob", out)
        self.assertFalse((self.tmp / "bob" / "watcher.pid").exists())
        # The watcher didn't mark it read.
        self.assertEqual([e["text"] for e in json.loads(self.myous("bob", "inbox", "--json"))], ["hello bob"])

    def test_3_takeover_moves_the_watcher(self):
        home = self.start_watcher("bob")
        other = self.start_watcher("bob", "--takeover")
        code, out = self.finish(home)
        self.assertEqual(code, 4, out)
        self.assertIn("replaced by another chat's watcher", out)

        self.myous("alice", "send", "bob", "reply for the other chat")
        code, out = self.finish(other)
        self.assertEqual(code, 0, out)
        self.assertIn("reply for the other chat", out)
        self.myous("bob", "inbox")

    def test_4_unread_items_wake_a_new_watcher_at_once(self):
        self.myous("alice", "send", "bob", "sent while nobody watched")
        self.myous("bob", "poll")
        code, out = self.finish(self.example("bob", "watcher.py"), timeout=20)
        self.assertEqual(code, 0, out)
        self.assertIn("sent while nobody watched", out)
        self.myous("bob", "inbox")

    def test_5_scheduled_check(self):
        code, out = self.finish(self.example("bob", "check.py"))
        self.assertEqual(code, 0, out)
        self.assertIn("no watcher is running", out)
        self.assertNotIn("new item", out)

        self.myous("alice", "send", "bob", "for the check")
        code, out = self.finish(self.example("bob", "watcher.py"))  # the watcher catches it...
        self.assertIn("for the check", out)
        code, out = self.finish(self.example("bob", "check.py"))  # ...and it's still unread for the check
        self.assertIn("1 new item(s)", out)
        self.assertIn("alice: for the check", out)

        self.myous("bob", "inbox")
        watcher = self.start_watcher("bob")
        code, out = self.finish(self.example("bob", "check.py"))
        self.assertEqual(out.strip(), "nothing to do")
        watcher.terminate()
        self.finish(watcher)

    def test_6_bounded_watcher_exits_quietly(self):
        started = time.time()
        code, out = self.finish(self.example("bob", "watcher.py", "--for", "3"))
        self.assertEqual(code, 2, out)
        self.assertIn("nothing new", out)
        self.assertLess(time.time() - started, 15)
        self.assertFalse((self.tmp / "bob" / "watcher.pid").exists())

    def test_7_stopped_watcher_cleans_up(self):
        watcher = self.start_watcher("bob")
        watcher.terminate()  # what `timeout` does
        code, out = self.finish(watcher)
        self.assertEqual(code, 5, out)
        self.assertIn("was stopped", out)
        self.assertFalse((self.tmp / "bob" / "watcher.pid").exists())

    def hook(self, agent: str) -> tuple[int, str]:
        runtime = self.tmp / "hook-runtime.sh"
        runtime.write_text('wake() { echo "WAKE $1"; }\nsilent() { echo "SILENT $1"; }\n')
        env = dict(self.env(agent), HATCH_HOOK_RUNTIME=str(runtime), MYOUS_PYTHON=sys.executable,
                   MYOUS_HOOK_WINDOW="4")
        r = subprocess.run(["bash", str(EXAMPLES / "hook.sh")], env=env, capture_output=True, text=True, timeout=60)
        return r.returncode, r.stdout + r.stderr

    def test_8_hook_wakes_only_on_news(self):
        code, out = self.hook("bob")
        self.assertEqual(code, 0, out)
        self.assertIn("SILENT myoushq: watcher exit 2", out)

        self.myous("alice", "send", "bob", "for the hook")
        code, out = self.hook("bob")
        self.assertIn("WAKE myoushq: new item(s)", out)
        self.assertNotIn("for the hook", out)  # message text stays out of the wake
        entries = json.loads(self.myous("bob", "inbox", "--json"))
        self.assertEqual([e["text"] for e in entries], ["for the hook"])


if __name__ == "__main__":
    unittest.main()

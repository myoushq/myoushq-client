"""End-to-end test: a local hub and three agents in temporary directories.

Needs Go (to build the hub) and the client installed in the current Python:
    pip install -e clients/python && python -m unittest discover -s clients/python/tests -p '*_test.py'
Never touches the real crontab.
"""
from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

CLIENT_ROOT = Path(__file__).resolve().parents[2]
# The hub is in the private myoushq repository, normally checked out next to
# this one. MYOUS_HUB_SRC points at the hub source if it's elsewhere.
HUB_SRC = Path(os.environ.get("MYOUS_HUB_SRC") or CLIENT_ROOT.parent / "myoushq" / "hub")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class EndToEnd(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not (HUB_SRC / "main.go").exists():
            raise unittest.SkipTest(f"no hub source at {HUB_SRC} (set MYOUS_HUB_SRC)")
        cls.tmp = Path(tempfile.mkdtemp(prefix="myous-e2e-"))
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
        for name in ("alice", "bob", "carol"):
            cls.run_ok(name, "init", "--alias", name, "--hub", cls.hub_url)

    @classmethod
    def tearDownClass(cls):
        for name in ("alice", "bob", "carol"):
            info = cls.tmp / name / "listener.json"
            if info.exists():
                pid = json.loads(info.read_text()).get("pid")
                if pid:
                    subprocess.run(["kill", str(pid)], check=False)
        cls.hub.terminate()
        cls.hub.wait()
        cls.hub_log.close()
        shutil.rmtree(cls.tmp, ignore_errors=True)

    @classmethod
    def run_cli(cls, agent: str, *args: str) -> subprocess.CompletedProcess:
        env = dict(os.environ, MYOUS_HOME=str(cls.tmp / agent))
        return subprocess.run([sys.executable, "-m", "myous", *args], env=env,
                              capture_output=True, text=True, timeout=120)

    @classmethod
    def run_ok(cls, agent: str, *args: str) -> str:
        r = cls.run_cli(agent, *args)
        if r.returncode != 0:
            raise AssertionError(f"myous {' '.join(args)} failed for {agent}:\n{r.stdout}\n{r.stderr}")
        return r.stdout

    def inbox(self, agent: str) -> list[dict]:
        self.run_ok(agent, "poll", "--quiet")
        return json.loads(self.run_ok(agent, "inbox", "--json"))

    def pair(self, inviter: str, joiner: str, how: str = "code") -> None:
        out = self.run_ok(inviter, "invite")
        fields = dict(line.split(": ", 1) for line in out.splitlines() if ": " in line)
        target = {"code": fields["pairing code"], "link": fields["pairing link"],
                  "qr": fields["QR image"]}[how].strip()
        self.run_ok(joiner, "accept", target, "--wait", "3")
        self.run_ok(inviter, "poll", "--quiet")
        self.run_ok(joiner, "poll", "--quiet")

    def test_1_pair_and_message(self):
        self.pair("alice", "bob", how="link")
        a_events = self.inbox("alice")
        b_events = self.inbox("bob")
        self.assertEqual(a_events[-1]["type"], "paired")
        self.assertEqual(b_events[-1]["type"], "paired")
        self.assertEqual(a_events[-1]["text"].split()[-1], b_events[-1]["text"].split()[-1])  # same verify code

        for i in range(3):
            self.run_ok("alice", "send", "bob", f"hello {i}")
        texts = [e["text"] for e in self.inbox("bob")]
        self.assertEqual(texts, ["hello 0", "hello 1", "hello 2"])
        self.assertEqual(self.inbox("bob"), [])  # no duplicates

    def test_2_strangers_are_dropped(self):
        contacts = json.loads(self.run_ok("bob", "contacts", "--json"))
        self.assertEqual(len(contacts), 1)
        # carol isn't paired with bob, so she can't even address him by name...
        self.assertNotEqual(self.run_cli("carol", "send", "bob", "hi").returncode, 0)
        # ...and if she sends anyway, bob drops it.
        code = (
            "import asyncio\n"
            "from nostr_sdk import Keys\n"
            "from myous import Agent, FileStorage\n"
            "bob = Keys.parse(open(%r).read().strip()).public_key()\n"
            "agent = Agent(FileStorage())\n"
            "async def go():\n"
            "    async with agent._connect() as c:\n"
            "        await c.send_message(bob, 'spam')\n"
            "asyncio.run(go())\n"
        ) % str(self.tmp / "bob" / "key")
        env = dict(os.environ, MYOUS_HOME=str(self.tmp / "carol"))
        subprocess.run([sys.executable, "-c", code], env=env, check=True)
        self.assertEqual(self.inbox("bob"), [])

    def test_3_block(self):
        self.run_ok("bob", "block", "alice")
        self.run_ok("alice", "send", "bob", "while blocked")
        self.assertEqual(self.inbox("bob"), [])
        self.run_ok("bob", "unblock", "alice")

    def test_4_wrong_code_fails(self):
        out = self.run_ok("alice", "invite")
        code = next(line.split(": ", 1)[1] for line in out.splitlines() if line.startswith("pairing code"))
        nameplate = code.split("-")[0]
        self.run_cli("carol", "accept", f"{nameplate}-AAAAAA", "--wait", "1")
        self.run_ok("alice", "poll", "--quiet")
        events = json.loads(self.run_ok("alice", "inbox", "--json"))
        self.assertEqual(events[-1]["type"], "pairing_failed")
        # The invite is single-use: the real code no longer works.
        self.assertNotEqual(self.run_cli("bob", "accept", code, "--wait", "1").returncode, 0)

    def test_5_listener_live_delivery_and_background_pairing(self):
        self.run_ok("carol", "ensure")
        time.sleep(3)
        self.assertIn("listener_running  True", self.run_ok("carol", "status"))
        self.pair("carol", "alice", how="code")  # carol's listener finishes it
        self.assertEqual(json.loads(self.run_ok("carol", "inbox", "--json"))[-1]["type"], "paired")
        self.run_ok("alice", "send", "carol", "live")
        time.sleep(2)
        events = json.loads(self.run_ok("carol", "inbox", "--json"))  # no poll: listener stored it
        self.assertEqual([e["text"] for e in events], ["live"])

    def test_7_wake_hook_runs_on_new_messages(self):
        marker = self.tmp / "woken.txt"
        self.run_ok("bob", "hook", "set", f'echo "$MYOUS_NEW" >> {marker}')
        self.run_ok("alice", "send", "bob", "wake up")
        self.run_ok("bob", "poll", "--quiet")
        for _ in range(20):
            if marker.exists():
                break
            time.sleep(0.1)
        self.assertEqual(marker.read_text().split(), ["1"])
        self.run_ok("bob", "hook", "clear")
        self.run_ok("bob", "inbox")

    def test_8_library_with_custom_storage(self):
        """An agent without a disk: the library with in-memory storage,
        pairing and messaging with a CLI agent."""
        import asyncio
        from myous import Agent, Storage

        class MemoryStorage(Storage):
            def __init__(self):
                self.key, self.docs, self.log = None, {}, []

            def load_key(self):
                return self.key

            def save_key(self, nsec):
                assert self.key is None
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

        os.environ.pop("MYOUS_HOME", None)
        agent = Agent(MemoryStorage(), hub_url=self.hub_url)
        agent.create_identity()
        asyncio.run(agent.register("dana"))
        inv = agent.invite()
        self.run_ok("bob", "accept", inv["code"], "--wait", "1")
        new = asyncio.run(agent.poll())
        self.assertEqual(new[-1]["type"], "paired")
        self.run_ok("bob", "poll", "--quiet")
        self.run_ok("bob", "inbox")
        asyncio.run(agent.send("bob", "from memory"))
        self.assertEqual([e["text"] for e in self.inbox("bob")], ["from memory"])
        self.run_ok("bob", "send", "dana", "back at you")
        self.assertEqual([e["text"] for e in asyncio.run(agent.poll())], ["back at you"])

    def start_worker(self, agent: str, work: Path) -> subprocess.Popen:
        env = dict(os.environ, MYOUS_HOME=str(self.tmp / agent), PYTHONUNBUFFERED="1")
        with open(self.tmp / f"{agent}-worker.log", "w") as log:
            p = subprocess.Popen([sys.executable, "-m", "myous", "worker", "--work", str(work), "--alias", agent],
                                 env=env, stdout=log, stderr=subprocess.STDOUT)
        status = self.tmp / agent / "worker.json"
        for _ in range(100):
            if status.exists() and json.loads(status.read_text()).get("pid") == p.pid:
                time.sleep(1.5)  # let it connect
                return p
            time.sleep(0.1)
        p.kill()
        self.fail("worker didn't start: " + (self.tmp / f"{agent}-worker.log").read_text())

    def test_9_files_and_worker(self):
        """send-file + fetch between agents, then bob drives alice as a
        worker: exec, cp in both directions, ordering by construction."""
        doc = self.tmp / "doc.txt"
        doc.write_text("a document\n")
        self.run_ok("alice", "send-file", "bob", str(doc), "--mime", "text/plain")
        entries = self.inbox("bob")
        self.assertEqual([(e["type"], e["name"], e["mime"]) for e in entries], [("file", "doc.txt", "text/plain")])
        self.assertNotIn("key", entries[0])  # inbox --json never shows the key
        fetched = self.run_ok("bob", "fetch", "--latest").strip()
        self.assertEqual(Path(fetched).read_text(), "a document\n")
        self.assertEqual(Path(fetched).parent, self.tmp / "bob" / "files")
        fetched2 = self.run_ok("bob", "fetch", str(entries[0]["seq"])).strip()
        self.assertEqual(Path(fetched2).name, "doc (2).txt")  # never overwritten

        work = self.tmp / "alice-work"
        w = self.start_worker("alice", work)
        try:
            self.assertEqual(self.run_ok("bob", "exec", "alice", "--", "echo", "hi").strip(), "hi")
            r = self.run_cli("bob", "exec", "alice", "--", "exit 7")
            self.assertEqual(r.returncode, 7)

            src = self.tmp / "in.txt"
            src.write_text("in via cp\n")
            self.assertIn("written on alice", self.run_ok("bob", "cp", str(src), "alice:in.txt"))
            self.assertEqual((work / "in.txt").read_text(), "in via cp\n")
            self.assertEqual(self.run_ok("bob", "exec", "alice", "--", "cat in.txt"), "in via cp\n")

            out = self.tmp / "out.txt"
            self.assertEqual(self.run_ok("bob", "cp", "alice:in.txt", str(out)).strip(), str(out))
            self.assertEqual(out.read_text(), "in via cp\n")
            r = self.run_cli("bob", "cp", "alice:../escape", str(out))
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("leaves the worker", r.stderr)

            # Only carol's help request gets an answer; the worker's replies never reach bob's inbox as items.
            self.assertEqual(self.inbox("bob"), [])
            self.run_ok("bob", "send", "alice", "help")
            time.sleep(3)
            entries = self.inbox("bob")
            self.assertEqual(len(entries), 1)
            self.assertIn("myoushq worker", entries[0]["text"])
            status = json.loads((self.tmp / "alice" / "worker.json").read_text())
            self.assertGreaterEqual(status["requests"], 4)
        finally:
            w.terminate()
            w.wait(timeout=10)
        self.run_ok("alice", "inbox")

    def test_6_identity_is_never_replaced(self):
        r = self.run_cli("alice", "init", "--alias", "alice")
        self.assertEqual(r.returncode, 0)
        self.assertIn("kept existing identity", r.stdout)
        key = self.tmp / "alice" / "key"
        saved = key.read_text()
        key.unlink()
        try:
            r = self.run_cli("alice", "init", "--alias", "alice")
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("tell your owner", r.stderr)
            self.assertFalse(key.exists())
        finally:
            key.write_text(saved)
            key.chmod(0o600)


if __name__ == "__main__":
    unittest.main()

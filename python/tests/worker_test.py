"""The worker's request handling, with a fake agent: no hub, no relay."""
import asyncio
import json
import os
import sys
import signal
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

from unittest import mock

from myous import files, inbox, processes, worker
from myous.storage import FileStorage

PEER = "npub1peer"
RID = "ab" * 16


class FakeAgent:
    alias = "box"
    card = ""

    def __init__(self):
        self.sent: list[tuple[str, str]] = []
        self.files: list[tuple[str, str, list]] = []
        self.blobs: dict[str, bytes] = {}
        self._contacts = {"k": {"alias": "muse", "npub": PEER, "status": "approved"}}
        self.invites = 0

    def contacts(self):
        return self._contacts

    def invite(self):
        self.invites += 1
        return {"code": f"1000-AAAAA{self.invites}", "link": "https://hub.test/p/1000#AAAAA", "expires_at": time.time() + 900}

    async def send(self, to, text):
        self.sent.append((to, text))

    async def sync_cards(self):
        return []

    async def send_file(self, to, path, extra_tags=None, mime=None, file_name=None):
        self.files.append((to, str(path), extra_tags))

    def fetch_bytes(self, entry):
        return self.blobs[entry["x"]]

    class keys:  # noqa: N801 - mimics agent.keys.public_key().to_bech32()
        @staticmethod
        def public_key():
            class PK:
                @staticmethod
                def to_bech32():
                    return "npub1box"
            return PK()


def message(text, **extra):
    e = {"type": "message", "direction": "in", "peer": PEER, "alias": "muse", "text": text, "seq": 1, "at": 0}
    req = inbox.parse_request(text)
    if req and req["myous"] in inbox.REQUEST_OPS:
        e["request"] = req
    e.update(extra)
    return e


class WorkerFixture(unittest.TestCase):
    """A worker with a fake agent, in a temporary home and work directory."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name) / "home"
        self.work = Path(self.tmp.name) / "work"
        self.st = FileStorage(self.home)
        self.agent = FakeAgent()
        self.devnull = open(os.devnull, "w")
        self.w = worker.Worker(self.agent, self.st, self.work, notes=["Browser: http://localhost:9222"], out=self.devnull)

    def tearDown(self):
        self.devnull.close()
        self.tmp.cleanup()

    def run_(self, coro):
        return asyncio.run(coro)

    def replies(self):
        return [json.loads(t) if t.startswith("{") else t for _, t in self.agent.sent]


class WorkerTest(WorkerFixture):
    def test_exec_result(self):
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo out; echo err >&2; exit 3"}))))
        r = self.replies()[-1]
        self.assertEqual((r["myous"], r["id"], r["exit"], r["stdout"], r["stderr"], r["truncated"]),
                         ("result", RID, 3, "out\n", "err\n", False))
        self.assertEqual(self.agent.sent[-1][0], PEER)
        log = [json.loads(line) for line in (self.home / "worker.log").read_text().splitlines()]
        self.assertEqual((log[-1]["op"], log[-1]["decision"], log[-1]["alias"]), ("exec", "allow", "muse"))
        status = json.loads((self.home / "worker.json").read_text()) if (self.home / "worker.json").exists() else None
        self.w.write_status()
        status = json.loads((self.home / "worker.json").read_text())
        self.assertEqual((status["requests"], status["last"]["op"], status["contacts"], status["invite"]), (1, "exec", 1, None))

    def test_status_carries_a_pairing_message_while_unpaired(self):
        from myous.worker import pairing_message
        self.assertIn("1234-ABCDE", pairing_message("1234-ABCDE", "Sam's Mac"))
        self.assertIn("not something to run yourself", pairing_message("1234-ABCDE", "Sam's Mac"))
        self.assertNotIn("describes itself", pairing_message("1234-ABCDE", "Sam's Mac"))
        self.assertIn('It describes itself: "Sam\'s own Mac"', pairing_message("1234-ABCDE", "Sam's Mac", "Sam's own Mac"))

    def test_help_and_pairing_carry_the_description(self):
        self.agent.card = "Sam's own Mac; its browser is the 'myous browser'"
        self.run_(self.w.handle(message("help")))
        self.assertIn("Its owner describes it: Sam's own Mac; its browser is the 'myous browser'", self.agent.sent[0][1])
        self.agent._contacts = {}
        self.w.ensure_invite()
        self.w.write_status()
        status = json.loads((self.home / "worker.json").read_text())
        self.assertIn("It describes itself: \"Sam's own Mac; its browser is the 'myous browser'\"", status["invite"]["message"])

    def test_exec_runs_in_work_dir_and_truncates(self):
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "pwd; head -c 200000 /dev/zero | tr '\\0' x"}))))
        r = self.replies()[-1]
        self.assertTrue(r["stdout"].startswith(str(self.work.resolve()) + "\n"))
        self.assertTrue(r["truncated"])
        self.assertIn("output cut", r["stdout"])
        self.assertLess(len(r["stdout"]), worker.STDOUT_LIMIT + 200)

    def test_exec_timeout_kills_the_command(self):
        started = time.time()
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "sleep 30", "timeout": 1}))))
        r = self.replies()[-1]
        self.assertEqual(r["exit"], -1)
        self.assertIn("killed after 1 seconds", r["stderr"])
        self.assertLess(time.time() - started, 10)

    def test_timeout_does_not_wait_for_an_escaped_grandchild(self):
        # A daemon that leaves the process group and keeps our output open
        # must not hang the worker: the shell is reaped, what was written stays.
        cmd = f"{sys.executable} -c 'import os,time,sys; os.setsid(); print(\"started\", flush=True); time.sleep(20)' & wait"
        started = time.time()
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": cmd, "timeout": 1}))))
        r = self.replies()[-1]
        self.assertEqual(r["exit"], -1)
        self.assertIn("killed after 1 seconds", r["stderr"])
        self.assertIn("started", r["stdout"])
        self.assertLess(time.time() - started, 12)

    def test_owner_stops_a_running_command(self):
        # The app drops commands/stop-<id>; the worker kills the command and
        # tells the agent it was stopped by the owner.
        (self.home / "commands").mkdir(exist_ok=True)

        async def stop_soon():
            await asyncio.sleep(0.7)
            (self.home / "commands" / f"stop-{RID}").touch()

        async def both():
            await asyncio.gather(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo begun; sleep 30", "timeout": 60}))), stop_soon())

        started = time.time()
        self.run_(both())
        r = self.replies()[-1]
        self.assertEqual(r["exit"], -1)
        self.assertTrue(r["stopped"])
        self.assertIn("stopped by the owner", r["stderr"])
        self.assertIn("begun", r["stdout"])
        self.assertLess(time.time() - started, 10)
        self.assertFalse((self.home / "commands" / f"stop-{RID}").exists())
        rec = json.loads((self.home / "requests" / f"{RID}.json").read_text())
        self.assertTrue(rec["stopped"])

    def test_a_request_id_cannot_name_a_stop_file_elsewhere(self):
        # "stop-../../../key" would be <home>/key wherever ".." is resolved
        # by name (Windows; here, through a directory called "stop-..").
        (self.home / "commands" / "stop-..").mkdir(parents=True)
        victim = self.home / "victim"
        victim.write_text("keep me")
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": "../../../victim", "cmd": "echo ran"}))))
        self.assertEqual(self.replies()[-1], {"myous": "ack", "id": "../../../victim", "ok": False, "error": worker.BAD_ID})
        self.assertEqual(victim.read_text(), "keep me")

    def test_request_ids_are_plain_names(self):
        # One rule for ids, as the apps and review.py have it: 1 to 64 of
        # A-Z, a-z, 0-9, - and _. Anything else is refused before it runs.
        for rid in ("a-b_C9", "x" * 64, "0"):
            self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": rid, "cmd": "echo ran"}))))
            self.assertEqual(self.replies()[-1]["stdout"], "ran\n", rid)
            self.assertTrue((self.home / "requests" / f"{rid}.json").exists(), rid)
        for rid in ("x" * 65, "", "caf\u00e9", "\u0661\u0662", "a.b", "a b", "a/b", "a\\b", "a\n"):
            self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": rid, "cmd": "touch ran"}))))
            self.assertEqual(self.replies()[-1], {"myous": "ack", "id": rid, "ok": False, "error": worker.BAD_ID}, repr(rid))
        self.assertFalse((self.work / "ran").exists())
        put = {"type": "file", "direction": "in", "peer": PEER, "alias": "muse", "name": "in.txt", "size": 1,
               "x": "00", "w": ["put", "a.b", "in.txt"], "seq": 2, "at": 0}
        self.run_(self.w.handle(put))
        self.assertEqual(self.replies()[-1], {"myous": "ack", "id": "a.b", "ok": False, "error": worker.BAD_ID})
        self.assertEqual(sorted(p.name for p in (self.home / "requests").iterdir()),
                         sorted(["a-b_C9.json", "x" * 64 + ".json", "0.json"]))

    def test_stopping_the_worker_ends_a_review_that_waits(self):
        # A hook waiting for the owner when the worker is stopped: ended, and
        # the agent is told.
        self.w.review_cmd = "sleep 30"
        threading.Timer(0.5, self.w.stopping.set).start()
        started = time.time()
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "touch ran"}))))
        self.assertEqual(self.replies()[-1], {"myous": "ack", "id": RID, "ok": False, "error": worker.STOPPING})
        self.assertFalse((self.work / "ran").exists())
        self.assertLess(time.time() - started, 5)

    def test_malformed_requests_are_refused_not_fatal(self):
        self.run_(self.w.handle(message('{"myous": "exec", "id": "%s", "cmd": "echo ok", "timeout": Infinity}' % RID)))
        self.assertEqual(self.replies()[-1]["stdout"], "ok\n")  # a bad timeout means the default
        self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": "a\u0000b"}))))
        self.assertFalse(self.replies()[-1]["ok"])
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": ["not", "a", "string"]}))))
        self.assertFalse(self.replies()[-1]["ok"])
        # Something unexpected inside an operation: a refusal, and the worker lives on.
        self.w.do_exec = None
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo x"}))))
        self.assertIn("bad request", self.replies()[-1]["error"])
        # A work directory a command removed comes back.
        import shutil
        shutil.rmtree(self.work)
        self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": "x"}))))
        self.assertTrue(self.work.is_dir())

    def test_review_hook_must_live_outside_the_work_dir(self):
        inside = self.work / "review.py"
        inside.write_text("import sys; sys.exit(0)\n")
        with self.assertRaises(ValueError):
            worker.Worker(self.agent, self.st, self.work, review_cmd=f"{sys.executable} {inside}", out=self.devnull)
        # A relative hook outside it is made absolute, and runs from the home directory, not the work directory.
        outside = Path(self.tmp.name) / "hook.py"
        outside.write_text("import os, sys; print(os.getcwd(), file=sys.stderr); sys.exit(1)\n")
        cwd = os.getcwd()
        os.chdir(self.tmp.name)
        try:
            w = worker.Worker(self.agent, self.st, self.work, review_cmd=f"{sys.executable} hook.py", out=self.devnull)
        finally:
            os.chdir(cwd)
        self.assertIn(str(outside), w.review_cmd)
        self.run_(w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo x"}))))
        self.assertEqual(self.replies()[-1]["error"], str(self.home.resolve()))

    def test_requests_run_one_at_a_time(self):
        marks = self.work / "marks"
        first = message(json.dumps({"myous": "exec", "id": "a" * 32, "cmd": f"echo start1 >> {marks}; sleep 1; echo end1 >> {marks}"}))
        second = message(json.dumps({"myous": "exec", "id": "b" * 32, "cmd": f"echo start2 >> {marks}; echo end2 >> {marks}"}))

        async def both():
            await asyncio.gather(self.w.on_new([first]), self.w.on_new([second]))

        self.run_(both())
        self.assertEqual(marks.read_text().split(), ["start1", "end1", "start2", "end2"])

    def test_review_hook_refuses(self):
        hook = Path(self.tmp.name) / "review.py"
        hook.write_text("import json,sys\nr=json.load(sys.stdin)\n"
                        "sys.exit(0) if 'rm' not in r.get('cmd','') else (print('no deleting', file=sys.stderr), sys.exit(1))\n")
        self.w.review_cmd = f"{sys.executable} {hook}"
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "rm -rf x"}))))
        self.assertEqual(self.replies()[-1], {"myous": "ack", "id": RID, "ok": False, "error": "no deleting"})
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo fine"}))))
        self.assertEqual(self.replies()[-1]["stdout"], "fine\n")

    def test_pause_file(self):
        (self.home / "worker.paused").write_text("")
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo x"}))))
        self.assertEqual(self.replies()[-1]["error"], "worker is paused")
        (self.home / "worker.paused").unlink()
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo x"}))))
        self.assertEqual(self.replies()[-1]["stdout"], "x\n")

    def test_put_and_get(self):
        enc = files.encrypt(b"contents")
        self.agent.blobs[enc.x] = b"contents"
        entry = {"type": "file", "direction": "in", "peer": PEER, "alias": "muse", "name": "in.txt", "size": 24,
                 "x": enc.x, "ox": enc.ox, "key": enc.key.hex(), "nonce": enc.nonce.hex(), "w": ["put", RID, "sub/in.txt"],
                 "seq": 2, "at": 0}
        self.run_(self.w.handle(entry))
        ack = self.replies()[-1]
        self.assertEqual((ack["ok"], ack["size"], ack["sha256"]), (True, 8, files.sha256(b"contents")))
        self.assertEqual(Path(ack["path"]).read_bytes(), b"contents")
        self.assertEqual(Path(ack["path"]), (self.work / "sub/in.txt").resolve())

        self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": "sub/in.txt"}))))
        to, path, tags = self.agent.files[-1]
        self.assertEqual((to, Path(path), tags), (PEER, (self.work / "sub/in.txt").resolve(), [["w", "file", RID]]))

        self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": "missing.txt"}))))
        self.assertEqual(self.replies()[-1]["error"], "no such file on the worker")

    def test_paths_cannot_escape(self):
        for bad in ("../x", "/etc/passwd", "a/../../x", ""):
            self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": bad}))))
            self.assertFalse(self.replies()[-1]["ok"], bad)
        self.w.allow_absolute = True
        target, why = self.w.resolve("/etc/passwd")
        self.assertEqual((str(target), why), ("/etc/passwd", None))

    def test_help_once_a_minute(self):
        self.run_(self.w.handle(message("help")))
        self.run_(self.w.handle(message("what can you do?")))
        texts = [t for _, t in self.agent.sent]
        self.assertEqual(len(texts), 1)
        self.assertIn("myoushq worker", texts[0])
        self.assertIn("Browser: http://localhost:9222", texts[0])
        self.assertIn("myous exec", texts[0])
        self.w.help_sent[PEER] -= worker.HELP_INTERVAL + 1
        self.run_(self.w.handle(message("help")))
        self.assertEqual(len(self.agent.sent), 2)
        # A reply addressed to us by mistake is ignored, not answered with help.
        self.run_(self.w.handle(message(json.dumps({"myous": "ack", "id": RID, "ok": True}))))
        self.assertEqual(len(self.agent.sent), 2)

    def test_invite_until_paired(self):
        self.agent._contacts = {}
        self.w.ensure_invite()
        self.assertEqual(self.agent.invites, 1)
        self.w.ensure_invite()
        self.assertEqual(self.agent.invites, 1)  # still valid
        self.w.invite["expires_at"] = time.time()
        self.w.ensure_invite()
        self.assertEqual(self.agent.invites, 2)
        status = json.loads((self.home / "worker.json").read_text())
        self.assertEqual(status["invite"]["code"], "1000-AAAAA2")
        self.agent._contacts = {"k": {"alias": "muse", "npub": PEER, "status": "approved"}}
        self.w.ensure_invite()
        self.assertIsNone(self.w.invite)

    def test_pairing_result_shows_the_verification_code(self):
        asyncio.run(self.w.on_new([{"type": "paired", "alias": "muse", "peer": PEER, "verify": "358806", "text": "paired"}]))
        status = json.loads((self.home / "worker.json").read_text())
        self.assertEqual((status["paired"]["alias"], status["paired"]["verify"]), ("muse", "358806"))
        log = [json.loads(line) for line in (self.home / "worker.log").read_text().splitlines()]
        self.assertEqual((log[-1]["op"], log[-1]["verify"]), ("paired", "358806"))

    def test_status_has_a_phase(self):
        self.w.write_status()
        self.assertEqual(json.loads((self.home / "worker.json").read_text())["phase"], "running")
        self.w.pause_file.touch()
        self.w.write_status()
        self.assertEqual(json.loads((self.home / "worker.json").read_text())["phase"], "paused")

    def test_status_has_the_version(self):
        # The owner's app compares it with its own (Remove stale containers offers an older worker's container).
        from myous import __version__
        self.w.write_status()
        self.assertEqual(json.loads((self.home / "worker.json").read_text())["version"], __version__)

    def test_request_records_for_the_owner(self):
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "echo hi"}))))
        rec = json.loads((self.home / "requests" / f"{RID}.json").read_text())
        self.assertEqual((rec["op"], rec["alias"], rec["cmd"], rec["decision"], rec["exit"], rec["stdout"]),
                         ("exec", "muse", "echo hi", "allow", 0, "hi\n"))
        self.assertIn("duration", rec)
        self.assertGreaterEqual(rec["done_at"], rec["at"])
        self.w.pause_file.touch()
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": "cd" * 16, "cmd": "echo no"}))))
        rec = json.loads((self.home / "requests" / ("cd" * 16 + ".json")).read_text())
        self.assertEqual((rec["decision"], rec["reason"]), ("refuse", "worker is paused"))
        # Odd ids are refused, and can't name a record elsewhere.
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": "../x", "cmd": "echo"}))))
        self.assertEqual(sorted(p.name for p in (self.home / "requests").iterdir()), sorted([f"{RID}.json", "cd" * 16 + ".json"]))

    def test_request_records_are_pruned(self):
        for i in range(worker.REQUESTS_KEEP + 5):
            self.w.record(f"r{i:04d}", op="exec")
        kept = sorted(p.name for p in (self.home / "requests").glob("*.json"))
        self.assertEqual(len(kept), worker.REQUESTS_KEEP)
        self.assertNotIn("r0000.json", kept)

    def test_windows_says_powershell(self):
        # On Windows the help, the pairing message and a command PowerShell
        # doesn't know tell the agent it is talking to PowerShell.
        cmdlet_error = ("ls : The term 'ls' is not recognized as the name of a cmdlet, function, script file, "
                        "or operable program.\n")
        fake = mock.Mock(side_effect=lambda *a: {"exit": 1, "stdout": "", "stderr": cmdlet_error, "truncated": False})
        with mock.patch.object(processes, "WINDOWS", True):
            self.run_(self.w.handle(message("help")))
            lines = self.agent.sent[-1][1].splitlines()
            self.assertEqual(lines[1], worker.WINDOWS_HELP)
            self.assertIn("Shell: PowerShell on ", self.agent.sent[-1][1])
            self.assertTrue(worker.pairing_message("1234-ABCDE", "PC").endswith(
                "before using it. It runs on Windows and takes PowerShell commands."))
            with mock.patch.object(worker, "run_command", fake):
                self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "ls"}))))
            r = self.replies()[-1]
            self.assertEqual((r["exit"], r["stderr"]), (1, cmdlet_error + worker.WINDOWS_HINT))
            rec = json.loads((self.home / "requests" / f"{RID}.json").read_text())
            self.assertEqual(rec["stderr"], r["stderr"])
        # Elsewhere, none of it.
        self.w.help_sent.clear()
        self.run_(self.w.handle(message("help")))
        self.assertNotIn(worker.WINDOWS_HELP, self.agent.sent[-1][1])
        self.assertNotIn("PowerShell", worker.pairing_message("1234-ABCDE", "PC", windows=False))
        with mock.patch.object(worker, "run_command", fake):
            self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": "ls"}))))
        self.assertEqual(self.replies()[-1]["stderr"], cmdlet_error)

    def test_owner_commands(self):
        self.agent._contacts = {}
        self.w.ensure_invite()
        self.assertEqual(self.agent.invites, 1)
        (self.home / "commands").mkdir()
        (self.home / "commands" / "new-code").touch()
        self.w.tick()
        self.assertEqual(self.agent.invites, 2)
        self.assertFalse((self.home / "commands" / "new-code").exists())
        # Paired: new-code does nothing; unpair drops the contacts and re-invites.
        self.agent._contacts = {"k": {"alias": "muse", "npub": PEER, "status": "approved"}}
        self.st.put("contacts", self.agent._contacts)
        self.w.paired = {"alias": "muse", "verify": "1", "at": 1}
        (self.home / "commands" / "new-code").touch()
        self.w.tick()
        self.assertEqual(self.agent.invites, 2)
        (self.home / "commands" / "unpair").touch()
        self.w.tick()
        self.assertEqual(self.st.get("contacts"), {})
        self.assertIsNone(self.w.paired)
        log = [json.loads(line) for line in (self.home / "worker.log").read_text().splitlines()]
        self.assertEqual(log[-1]["op"], "unpair")
        # quit is run()'s (StopSignalTest): the tick leaves it alone.
        (self.home / "commands" / "quit").touch()
        self.w.tick()
        self.assertTrue((self.home / "commands" / "quit").exists())


NOW = 1_800_000_000  # the worker's clock in the expiry tests


class ExpiryTest(WorkerFixture):
    """A request is good for its ttl from when it was sent (protocol.md
    7.1): one the worker takes up later is answered "expired", and is
    neither reviewed nor run."""

    def setUp(self):
        super().setUp()
        clock = mock.patch.object(worker.time, "time", return_value=float(NOW))
        clock.start()
        self.addCleanup(clock.stop)
        # Anything that reaches the review fails the request (and the test).
        self.reviewed = mock.Mock(side_effect=AssertionError("reviewed"))
        self.w.review = self.reviewed

    def exec_(self, age=None, **fields):
        req = dict({"myous": "exec", "id": RID, "cmd": "touch ran"}, **fields)
        return message(json.dumps(req), **({} if age is None else {"sent_at": NOW - age}))

    def put(self, age, *ttl):
        return {"type": "file", "direction": "in", "peer": PEER, "alias": "muse", "name": "in.txt", "size": 36,
                "x": "00" * 32, "w": ["put", RID, "in.txt", *ttl], "seq": 2, "at": 0, "sent_at": NOW - age}

    def assert_expired(self, error):
        self.assertEqual(self.replies()[-1], {"myous": "ack", "id": RID, "ok": False, "error": error})
        self.reviewed.assert_not_called()
        rec = json.loads((self.home / "requests" / f"{RID}.json").read_text())
        self.assertEqual((rec["decision"], rec["reason"], rec["at"], rec["done_at"]), ("expired", error, NOW, NOW))
        return rec

    def test_expired_exec_get_and_put_are_neither_reviewed_nor_run(self):
        self.run_(self.w.handle(self.exec_(age=120, ttl=60)))
        rec = self.assert_expired("expired: sent 2 minutes ago, its ttl is 60 seconds; not run")
        self.assertEqual((rec["op"], rec["cmd"], rec["alias"], rec["sender"]), ("exec", "touch ran", "muse", PEER))
        self.assertFalse((self.work / "ran").exists())
        log = [json.loads(line) for line in (self.home / "worker.log").read_text().splitlines()]
        self.assertEqual((log[-1]["op"], log[-1]["id"], log[-1]["sender"], log[-1]["decision"]), ("exec", RID, PEER, "expired"))

        self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": "secret.txt", "ttl": 60}),
                                        sent_at=NOW - 61)))
        rec = self.assert_expired("expired: sent 61 seconds ago, its ttl is 60 seconds; not run")
        self.assertEqual((rec["op"], rec["path"]), ("get", "secret.txt"))
        self.assertEqual(self.agent.files, [])

        # A put expires before its blob is downloaded.
        self.agent.fetch_bytes = mock.Mock(side_effect=AssertionError("downloaded"))
        self.run_(self.w.handle(self.put(400, "300")))
        rec = self.assert_expired("expired: sent 6 minutes ago, its ttl is 300 seconds; not run")
        self.assertEqual((rec["op"], rec["path"], rec["size"]), ("put", "in.txt", 36))
        self.agent.fetch_bytes.assert_not_called()
        self.assertFalse((self.work / "in.txt").exists())

        # Counted for the status, but not as a refusal.
        self.w.write_status()
        status = json.loads((self.home / "worker.json").read_text())
        self.assertEqual((status["requests"], status["last"]["op"], status["last"]["ok"]), (3, "expired", False))

    def test_the_age_in_the_error(self):
        for age, ttl, ago in ((59, 30, "59 seconds"), (150, 30, "2 minutes"), (1500, 150, "25 minutes"),
                              (3 * 3600, 600, "3 hours"), (119, 1, "119 seconds"), (120, 1, "2 minutes"),
                              (7199, 1, "119 minutes"), (7200, 1, "2 hours")):
            self.run_(self.w.handle(self.exec_(age=age, ttl=ttl)))
            self.assertEqual(self.replies()[-1]["error"], f"expired: sent {ago} ago, its ttl is {ttl} seconds; not run")
        self.assertEqual(worker.expired_text(1500, 150), "expired: sent 25 minutes ago, its ttl is 150 seconds; not run")
        self.assertFalse((self.work / "ran").exists())

    def test_within_its_ttl_a_request_runs(self):
        self.w.review = lambda *a, **k: None
        self.run_(self.w.handle(self.exec_(age=60, ttl=60)))   # expired only when older than its ttl
        self.assertEqual(self.replies()[-1]["exit"], 0)
        self.assertTrue((self.work / "ran").exists())

    def test_without_a_ttl_600_seconds(self):
        self.run_(self.w.handle(self.exec_(age=601)))
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 10 minutes ago, its ttl is 600 seconds; not run")
        self.run_(self.w.handle(self.put(601)))
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 10 minutes ago, its ttl is 600 seconds; not run")
        self.run_(self.w.handle(self.exec_(age=601, ttl=None)))   # null is no ttl
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 10 minutes ago, its ttl is 600 seconds; not run")
        self.w.review = lambda *a, **k: None
        self.run_(self.w.handle(self.exec_(age=600)))
        self.assertEqual(self.replies()[-1]["exit"], 0)

    def test_a_ttl_above_a_day_counts_as_a_day(self):
        self.run_(self.w.handle(self.exec_(age=86401, ttl=10 ** 9)))
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 24 hours ago, its ttl is 86400 seconds; not run")
        self.run_(self.w.handle(self.exec_(age=86401, ttl=10 ** 400)))   # too big for a float: still a day, as in Go
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 24 hours ago, its ttl is 86400 seconds; not run")
        self.run_(self.w.handle(self.put(86401, "9" * 40)))
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 24 hours ago, its ttl is 86400 seconds; not run")
        self.w.review = lambda *a, **k: None
        self.run_(self.w.handle(self.exec_(age=86400, ttl=10 ** 9)))
        self.assertEqual(self.replies()[-1]["exit"], 0)

    def test_a_fractional_ttl_counts_whole_seconds(self):
        self.run_(self.w.handle(self.exec_(age=91, ttl=90.7)))
        self.assertEqual(self.replies()[-1]["error"], "expired: sent 91 seconds ago, its ttl is 90 seconds; not run")

    def test_a_ttl_that_is_not_a_number_of_seconds_is_refused(self):
        for ttl in (0, -5, 0.5, True, False, "60", [60], {"s": 60}):
            self.run_(self.w.handle(self.exec_(age=1, ttl=ttl)))
            self.assertEqual(self.replies()[-1], {"myous": "ack", "id": RID, "ok": False, "error": worker.BAD_TTL}, repr(ttl))
            self.run_(self.w.handle(message(json.dumps({"myous": "get", "id": RID, "path": "x", "ttl": ttl}), sent_at=NOW)))
            self.assertEqual(self.replies()[-1]["error"], worker.BAD_TTL, repr(ttl))
        for raw in ("NaN", "Infinity", "-Infinity", "1e400"):
            self.run_(self.w.handle(message('{"myous": "exec", "id": "%s", "cmd": "touch ran", "ttl": %s}' % (RID, raw),
                                            sent_at=NOW)))
            self.assertEqual(self.replies()[-1]["error"], worker.BAD_TTL, raw)
        self.agent.fetch_bytes = mock.Mock(side_effect=AssertionError("downloaded"))
        for ttl in ("0", "-1", "1.5", "", " 60", "abc", "\u0661"):
            self.run_(self.w.handle(self.put(1, ttl)))
            self.assertEqual(self.replies()[-1]["error"], worker.BAD_TTL, repr(ttl))
        rec = json.loads((self.home / "requests" / f"{RID}.json").read_text())
        self.assertEqual((rec["decision"], rec["reason"]), ("refuse", worker.BAD_TTL))
        self.reviewed.assert_not_called()
        self.agent.fetch_bytes.assert_not_called()
        self.assertFalse((self.work / "ran").exists())

    def test_when_it_was_sent_is_the_senders_clock(self):
        self.w.review = lambda *a, **k: None
        # No sent_at: never expired.
        self.run_(self.w.handle(self.exec_(ttl=1)))
        self.assertEqual(self.replies()[-1]["exit"], 0)
        # A sent_at in the future counts as now.
        (self.work / "ran").unlink()
        self.run_(self.w.handle(self.exec_(age=-3600, ttl=1)))
        self.assertEqual(self.replies()[-1]["exit"], 0)
        self.assertTrue((self.work / "ran").exists())

    def test_an_old_help_gets_no_reply(self):
        self.run_(self.w.handle(message("help", sent_at=NOW - worker.DEFAULT_TTL - 1)))
        self.run_(self.w.handle(message("what can you do?", sent_at=NOW - 3600)))
        self.assertEqual(self.agent.sent, [])
        self.run_(self.w.handle(message("help", sent_at=NOW - worker.DEFAULT_TTL)))
        self.assertEqual(len(self.agent.sent), 1)
        self.assertIn("myoushq worker", self.agent.sent[0][1])

    def test_a_ttl_is_its_own_and_ids_come_first(self):
        # A bad id is refused as before, whatever the ttl says.
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": "a.b", "cmd": "touch ran", "ttl": 0}),
                                        sent_at=NOW - 10)))
        self.assertEqual(self.replies()[-1]["error"], worker.BAD_ID)
        self.assertEqual(worker.request_ttl(None), worker.DEFAULT_TTL)
        self.assertEqual(worker.request_ttl("0090", tag=True), 90)
        self.assertIsNone(worker.request_ttl(90, tag=True))   # a tag's ttl is a string


# A worker in a process of its own, as `myous worker` runs: its fake agent
# hands it one exec request, writes its replies to a file and waits.
STOP_SCRIPT = r"""
import asyncio, json, sys
from pathlib import Path
from myous import worker
from myous.storage import FileStorage

home, work, cmd = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]

class Agent:
    alias, card = "box", ""
    def contacts(self):
        return {"k": {"alias": "muse", "npub": "npub1peer", "status": "approved"}}
    async def send(self, to, text):
        with open(home / "sent.jsonl", "a") as f:
            f.write(text + "\n")
    async def listen(self, on_new, on_tick, tick):
        req = {"myous": "exec", "id": "ab" * 16, "cmd": cmd, "timeout": 120}
        await on_new([{"type": "message", "direction": "in", "peer": "npub1peer", "alias": "muse",
                       "text": json.dumps(req), "request": req, "seq": 1, "at": 0}])
        await asyncio.sleep(3600)
    class keys:
        @staticmethod
        def public_key():
            class PK:
                @staticmethod
                def to_bech32():
                    return "npub1box"
            return PK()

asyncio.run(worker.Worker(Agent(), FileStorage(home), work).run())
print("worker: exited", flush=True)
"""


@unittest.skipIf(processes.WINDOWS, "POSIX signals and process groups")
class StopSignalTest(unittest.TestCase):
    """Stopping the worker (SIGTERM, as the apps and `docker stop` do; or
    SIGINT; or commands/quit, which the apps write too) ends the command it
    is running with its process group, answers the agent and exits."""

    def stop_with(self, sig=None):
        with tempfile.TemporaryDirectory() as tmp:
            home, work = Path(tmp) / "home", Path(tmp) / "work"
            pid_file = Path(tmp) / "sleep.pid"
            # The shell execs sleep, so the sleep is its process group's leader.
            cmd = f"echo begun; echo $$ > {pid_file}; exec sleep 60"
            env = dict(os.environ, PYTHONPATH=str(Path(worker.__file__).resolve().parents[1]))
            proc = subprocess.Popen([sys.executable, "-c", STOP_SCRIPT, str(home), str(work), cmd], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            try:
                deadline = time.time() + 20
                while not (pid_file.exists() and pid_file.read_text().strip()) and time.time() < deadline:
                    time.sleep(0.05)
                pgid = int(pid_file.read_text())
                os.killpg(pgid, 0)   # running
                started = time.time()
                if sig is None:   # the owner's app's commands/quit
                    (home / "commands").mkdir(exist_ok=True)
                    (home / "commands" / "quit").touch()
                else:
                    proc.send_signal(sig)
                out, _ = proc.communicate(timeout=10)
                self.assertLess(time.time() - started, 5)
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()
            self.assertEqual(proc.returncode, 0, out)
            self.assertIn("worker: exited", out)
            with self.assertRaises(ProcessLookupError):
                os.killpg(pgid, 0)   # the command's group is gone
            sent = [json.loads(line) for line in (home / "sent.jsonl").read_text().splitlines()]
            self.assertEqual(len(sent), 1)
            r = sent[0]
            self.assertEqual((r["myous"], r["id"], r["exit"], r["stopped"]), ("result", "ab" * 16, -1, True))
            self.assertIn(worker.STOPPING, r["stderr"])
            self.assertIn("begun", r["stdout"])
            rec = json.loads((home / "requests" / ("ab" * 16 + ".json")).read_text())
            self.assertTrue(rec["stopped"])
            self.assertFalse((home / "commands" / "quit").exists())

    def test_sigterm(self):
        self.stop_with(signal.SIGTERM)

    def test_sigint(self):
        self.stop_with(signal.SIGINT)

    def test_quit_file(self):
        self.stop_with()


if __name__ == "__main__":
    unittest.main()

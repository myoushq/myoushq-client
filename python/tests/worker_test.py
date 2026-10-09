"""The worker's request handling, with a fake agent: no hub, no relay."""
import asyncio
import json
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

from myous import files, inbox, worker
from myous.storage import FileStorage

PEER = "npub1peer"
RID = "ab" * 16


class FakeAgent:
    alias = "box"

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


class WorkerTest(unittest.TestCase):
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
        # A daemon that leaves the process group and keeps our pipes open
        # must not hang the worker: output stops, the shell is reaped.
        cmd = f"{sys.executable} -c 'import os,time,sys; os.setsid(); print(\"started\", flush=True); time.sleep(20)' & wait"
        started = time.time()
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": RID, "cmd": cmd, "timeout": 1}))))
        r = self.replies()[-1]
        self.assertEqual(r["exit"], -1)
        self.assertIn("kept running", r["stderr"])
        self.assertLess(time.time() - started, 12)

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
        # Odd ids can't escape the directory.
        self.run_(self.w.handle(message(json.dumps({"myous": "exec", "id": "../x", "cmd": "echo"}))))
        self.assertEqual(sorted(p.name for p in (self.home / "requests").iterdir()), sorted([f"{RID}.json", "cd" * 16 + ".json", "x.json"]))

    def test_request_records_are_pruned(self):
        for i in range(worker.REQUESTS_KEEP + 5):
            self.w.record(f"r{i:04d}", op="exec")
        kept = sorted(p.name for p in (self.home / "requests").glob("*.json"))
        self.assertEqual(len(kept), worker.REQUESTS_KEEP)
        self.assertNotIn("r0000.json", kept)

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


if __name__ == "__main__":
    unittest.main()

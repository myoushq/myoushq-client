"""worker/review.py, the example review hook: levels, the read-only
check, and asking the owner through the approvals directory."""
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

HOOK = Path(__file__).resolve().parents[2] / "worker" / "review.py"


def load(home):
    os.environ["MYOUS_HOME"] = str(home)
    os.environ["MYOUS_APPROVAL_WAIT"] = "3"
    spec = importlib.util.spec_from_file_location("review_hook", HOOK)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


class ReadOnlyTest(unittest.TestCase):
    def test_read_only_commands(self):
        r = load(Path(tempfile.mkdtemp()))
        for ok in ["ls -la", "cat a.txt | grep x | wc -l", "find . -name '*.py' && echo done", "pwd"]:
            self.assertTrue(r.read_only(ok, windows=False), ok)
        for bad in ["rm -rf x", "cat a > b", "echo $(whoami)", "ls; python3 x.py", "sed -i s/a/b/ f", "cat `ls`", "curl http://x"]:
            self.assertFalse(r.read_only(bad, windows=False), bad)

    def test_code_inside_a_read_only_command(self):
        r = load(Path(tempfile.mkdtemp()))
        for bad in ["find . -exec rm {} ;", "find . -delete", "awk 'BEGIN{system(\"rm x\")}'"]:
            self.assertFalse(r.read_only(bad, windows=False), bad)

    def test_powershell(self):
        r = load(Path(tempfile.mkdtemp()))
        for ok in ["Get-ChildItem -Recurse | Select-String foo", "gc C:\\work\\a.txt | Measure-Object -Line", "dir; Get-Date"]:
            self.assertTrue(r.read_only(ok, windows=True), ok)
        for bad in ["Remove-Item x", "gci | Remove-Item", "gc (Remove-Item x)", "& notepad", "gci | where { Remove-Item $_ }",
                    "Get-Content a > b", "iex 'rm x'", "Invoke-WebRequest http://x"]:
            self.assertFalse(r.read_only(bad, windows=True), bad)


class HookTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)
        self.env = dict(os.environ, MYOUS_HOME=str(self.home), MYOUS_APPROVAL_WAIT="3")

    def tearDown(self):
        self.tmp.cleanup()

    def run_hook(self, req):
        r = subprocess.run([sys.executable, str(HOOK)], input=json.dumps(req), capture_output=True, text=True, env=self.env, timeout=30)
        return r.returncode, r.stderr.strip()

    def level(self, lv):
        (self.home / "review.json").write_text(json.dumps({"level": lv}))

    def test_trust_allows_and_pause_refuses(self):
        self.assertEqual(self.run_hook({"op": "exec", "id": "a", "cmd": "rm -rf x"})[0], 0)
        (self.home / "worker.paused").touch()
        self.assertEqual(self.run_hook({"op": "exec", "id": "a", "cmd": "ls"}), (1, "the worker is paused by its owner"))

    def test_default_level_for_an_unconfigured_worker(self):
        # No review.json: the level is $MYOUS_DEFAULT_REVIEW (the container's
        # entrypoint sets "changes"), else "trust".
        self.env["MYOUS_DEFAULT_REVIEW"] = "changes"
        self.env["MYOUS_APPROVAL_WAIT"] = "1"
        self.assertEqual(self.run_hook({"op": "exec", "id": "a", "cmd": "ls -la"})[0], 0)  # a read runs
        code, why = self.run_hook({"op": "exec", "id": "b", "cmd": "touch x"})            # a change asks
        self.assertEqual((code, why), (1, "the owner didn't answer in time"))
        # An unset or invalid value keeps today's default (trust): a change runs.
        for val in ("", "bogus"):
            self.env["MYOUS_DEFAULT_REVIEW"] = val
            self.assertEqual(self.run_hook({"op": "exec", "id": "c", "cmd": "rm -rf x"})[0], 0, repr(val))
        del self.env["MYOUS_DEFAULT_REVIEW"]
        self.assertEqual(self.run_hook({"op": "exec", "id": "d", "cmd": "rm -rf x"})[0], 0)
        # A written review.json still wins over the default.
        self.env["MYOUS_DEFAULT_REVIEW"] = "changes"
        self.level("trust")
        self.assertEqual(self.run_hook({"op": "exec", "id": "e", "cmd": "rm -rf x"})[0], 0)

    def test_changes_level(self):
        self.level("changes")
        self.assertEqual(self.run_hook({"op": "exec", "id": "a", "cmd": "ls -la"})[0], 0)
        self.assertEqual(self.run_hook({"op": "get", "id": "b", "path": "x"})[0], 0)
        # A change with nobody answering: refused after the wait, question removed.
        code, why = self.run_hook({"op": "put", "id": "c", "path": "x", "size": 1})
        self.assertEqual((code, why), (1, "the owner didn't answer in time"))
        self.assertEqual(list((self.home / "approvals").glob("*")), [])

    def test_all_level_asks_and_takes_the_answer(self):
        self.level("all")
        approvals = self.home / "approvals"

        def answer(verdict):
            for _ in range(40):
                qs = list(approvals.glob("*.json")) if approvals.exists() else []
                if qs:
                    q = json.loads(qs[0].read_text())
                    self.assertEqual((q["op"], q["cmd"]), ("exec", "ls"))
                    qs[0].with_suffix(".answer").write_text(verdict)
                    return
                time.sleep(0.1)

        t = threading.Thread(target=answer, args=("allow",)); t.start()
        self.assertEqual(self.run_hook({"op": "exec", "id": "q1", "cmd": "ls"})[0], 0)
        t.join()
        t = threading.Thread(target=answer, args=("refuse",)); t.start()
        self.assertEqual(self.run_hook({"op": "exec", "id": "q2", "cmd": "ls"}), (1, "refused by the owner"))
        t.join()
        # Ids with - and _ are asked about under their own name, which the apps answer.
        t = threading.Thread(target=answer, args=("allow",)); t.start()
        self.assertEqual(self.run_hook({"op": "exec", "id": "q-3_x", "cmd": "ls"})[0], 0)
        t.join()
        self.assertEqual(list(approvals.glob("*")), [])

    def test_ids_the_apps_cannot_answer_are_refused(self):
        self.level("all")
        bad = "bad request: the id must be 1 to 64 characters from A-Z, a-z, 0-9, - and _"
        for rid in ("x" * 65, "", "caf\u00e9", "a.b", "../x", 7, None):
            self.assertEqual(self.run_hook({"op": "exec", "id": rid, "cmd": "ls"}), (1, bad), repr(rid))
        self.assertEqual(list(self.home.glob("approvals/*")), [])


if __name__ == "__main__":
    unittest.main()

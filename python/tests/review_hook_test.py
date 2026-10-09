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
            self.assertTrue(r.read_only(ok), ok)
        for bad in ["rm -rf x", "cat a > b", "echo $(whoami)", "ls; python3 x.py", "sed -i s/a/b/ f", "cat `ls`", "curl http://x"]:
            self.assertFalse(r.read_only(bad), bad)


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
        self.assertEqual(list(approvals.glob("*")), [])


if __name__ == "__main__":
    unittest.main()

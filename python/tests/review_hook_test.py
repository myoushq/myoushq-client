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

    def test_background_operator_is_a_separator(self):
        # A single & backgrounds the first command and runs the next; both
        # segments must be read-only, not just the one before the &.
        r = load(Path(tempfile.mkdtemp()))
        for bad in ["echo hi & rm -rf x", "ls & python3 x.py", "cat a & curl http://x"]:
            self.assertFalse(r.read_only(bad, windows=False), bad)
        # A real background of a read-only command is still read-only.
        self.assertTrue(r.read_only("cat a & grep x b", windows=False))

    def test_allowlisted_commands_that_write(self):
        # A "read-only" command can still write through a flag.
        r = load(Path(tempfile.mkdtemp()))
        for bad in ["sort -o out in", "sort --output=out in", "sort -oout in",
                    "find . -fprintf out %p", "find . -fprint out", "find . -fls out", "cat a | sort -o b"]:
            self.assertFalse(r.read_only(bad, windows=False), bad)
        # The same flag letters where they only read stay read-only.
        for ok in ["grep -o pat f", "sort -n in", "find . -name x", "sort in"]:
            self.assertTrue(r.read_only(ok, windows=False), ok)

    def test_powershell(self):
        r = load(Path(tempfile.mkdtemp()))
        for ok in ["Get-ChildItem -Recurse | Select-String foo", "gc a.txt | Measure-Object -Line", "dir; Get-Date"]:
            self.assertTrue(r.read_only(ok, windows=True), ok)
        for bad in ["Remove-Item x", "gci | Remove-Item", "gc (Remove-Item x)", "& notepad", "gci | where { Remove-Item $_ }",
                    "Get-Content a > b", "iex 'rm x'", "Invoke-WebRequest http://x"]:
            self.assertFalse(r.read_only(bad, windows=True), bad)

    def test_reads_are_confined_to_work(self):
        # W3: at the "changes" level a read-only command is auto-allowed only
        # when every path argument stays under the work directory (the cwd).
        r = load(Path(tempfile.mkdtemp()))
        for ok in ["cat a.txt", "grep -r x .", "find . -name '*.py'", "cat ./sub/f", "head -n5 a b c",
                   "cat 'a b.txt'", "diff a.txt b.txt", "wc -l logs/app.log"]:
            self.assertTrue(r.read_only(ok, windows=False), ok)
        for bad in ["cat /etc/passwd", "cat /home/worker/.myous/key", "grep -r . /home/worker/.myous",
                    "cat ~/.ssh/id_rsa", "find / -name id_rsa", "cat ../key", "cat a/../../key",
                    "cat sub/../../etc/x", "head a.txt /etc/hostname"]:
            self.assertFalse(r.read_only(bad, windows=False), bad)
        # Windows: an absolute path or a drive is outside; a relative one is in.
        self.assertTrue(r.read_only("gc a.txt", windows=True))
        for bad in ["gc C:\\work\\a.txt", "gc \\\\host\\share\\x", "type ..\\key", "gc ~/x"]:
            self.assertFalse(r.read_only(bad, windows=True), bad)

    def test_flags_are_known_safe(self):
        # At "changes" a read-only command runs unasked only with flags known
        # safe for it (SAFE_FLAGS), operands OPERANDS allows, and paths in
        # the work directory; anything else asks.
        r = load(Path(tempfile.mkdtemp()))
        for ok in RUNS:
            self.assertTrue(r.read_only(ok, windows=False), ok)
        for bad in ASKS:
            self.assertFalse(r.read_only(bad, windows=False), bad)

    def test_flags_on_windows(self):
        # PowerShell's cmdlets keep their parameters (paths confined, also in
        # -Path:x, a comma list or another drive); the POSIX names and the
        # Windows programs take the strict rule.
        r = load(Path(tempfile.mkdtemp()))
        for ok in WINDOWS_RUNS:
            self.assertTrue(r.read_only(ok, windows=True), ok)
        for bad in WINDOWS_ASKS:
            self.assertFalse(r.read_only(bad, windows=True), bad)

    def test_the_flag_table(self):
        r = load(Path(tempfile.mkdtemp()))
        for name, (flags, valued, files) in r.SAFE_FLAGS.items():
            flags, valued, files = set(flags.split()), set(valued.split()), set(files.split())
            self.assertIn(name, r.READ_ONLY)
            self.assertLessEqual(valued, flags, name)    # a value flag is a safe flag
            self.assertLessEqual(files, valued, name)    # a file flag takes a value
            self.assertTrue(all(f.startswith("-") and f not in ("-", "--") for f in flags), name)
        self.assertLessEqual(set(r.OPERANDS), r.READ_ONLY)
        self.assertLessEqual(set(r.WINDOWS_PROGRAMS), r.READ_ONLY_POWERSHELL)


# Read-only commands with safe flags, in the work directory: they run.
RUNS = [
    "ls -la", "grep -rn x .", "find . -name '*.go' -type f", "head -n 20 a.txt", "wc -l logs/app.log", "sort -rn a.txt",
    "cut -d: -f1 a.txt", "date +%s", "uname -a", "du -sh .", "tree -L 2", "diff -u a b", "head -20 a.txt",
    "head -n5 a b c", "tail -n +5 a.txt", "grep -A3 x f", "grep -A 3 x f", "grep --context=2 x f", "grep -e -x f",
    "grep --include='*.go' -r x .", "rg -n --type py foo", "rg -g '!vendor' -tgo x", "less -p b f", "sort -t: -k2,2n f",
    "awk -F: 'NR==1' a.txt", "test -f x && cat x", "test x -nt y", "echo hi", "md5sum -c sums.txt",
    "find . -newer a.txt", "find . -path ./x -prune -o -print", "uniq -c a.txt", "ls -- -weird", "cat -", "date -u +%F",
    "stat -c %s a.txt", "file -b a.txt", "id -un", "free -h", "nproc", "uptime", "hostname", "whoami", "pwd",
    "realpath -e a", "readlink -f a", "which -a ls", "basename -s .txt a.txt", "tr -d x", "df -h .", "ls .hidden",
    "cat ./sub/f", "grep -c 'x$' f", "find . -name '.*'",
]
# They ask: an unknown or unsafe flag, an operand or a flag's file outside
# the work directory, an expansion the review can't see, or an operand that
# isn't what the command allows.
ASKS = [
    "sort -no x.txt", "sort --out=x a", "tree -o out.txt .", "less -o out a", "rg --pre sh x",
    "awk 'system(\"id\")' a", "date -s now", "file -C -m magic", "hostname box", "ls --unknown",
    "grep -f /etc/passwd a", "cat /etc/passwd", "find / -name id_rsa", "cat ~/.ssh/id_rsa", "cat ../key",
    "sort -o out in", "sort --output=out in", "find . -fprint out", "ls --color /etc", "grep -C /etc/passwd x",
    "cat $HOME/.ssh/id_rsa", "echo $AWS_SECRET_ACCESS_KEY", "date +$SECRET", "cat .*/x", "ls .*",
    "cat -- -/../../etc/passwd", "uniq a.txt out.txt", "date 0101", "less +!id a", "more +5 a", "cut -d/ ../x",
    "find . -newer ../x", "diff --from-file=/etc/x a", "awk 'getline' a", "awk '@load \"x\"' a",
    "awk 'ENVIRON[\"K\"]' a", "awk -f prog a", "printf -v x y", "whoami --help", "true -x", "date --set=x",
    "tail -f /var/log/x", "rg --pre=sh x", "rg -z x", "head -n", "grep -e", "file -z a", "tree -R -H x -o y",
    "diff -l a b", "md5sum -c ../sums", "ls -I x", "stat -f %z /etc", "less -k keys a", "sort -T /tmp a",
    "sort --compress-program=sh a", "du -sh ~", "realpath ..", "find . -L", "grep -r x ..",
]
WINDOWS_RUNS = [
    "Get-ChildItem -Recurse | Select-String foo", "gc a.txt | Measure-Object -Line", "dir; Get-Date", "gc a.txt",
    "ipconfig", "systeminfo", "where.exe python", "findstr foo a.txt", "ls -la", "type a.txt", "gci -Path sub",
    "gc -Path:a.txt", "cat a.txt",
]
WINDOWS_ASKS = [
    "gc env:AWS_SECRET_ACCESS_KEY", "gci HKCU:\\Software", "gc a.txt,C:\\x", "gc -Path:C:\\x\\y", "gc -Path C:\\x",
    "ipconfig /all", "findstr /s x *", "findstr -g:x a", "ls -Recurse", "cat -Raw a.txt", "cat \"-o\" x",
    "rg x \"--\" --pre=sh", "sort -o x a", "gc $env:USERPROFILE\\.ssh\\id_rsa", "gc ..\\key", "gc .*\\x",
]


class HookTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)
        self.env = dict(os.environ, MYOUS_HOME=str(self.home), MYOUS_APPROVAL_WAIT="3")
        self.env.pop("MYOUS_DEFAULT_REVIEW", None)   # no review.json is trust here, unless a test sets one

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

    def test_missing_or_corrupt_review_fails_closed(self):
        # No review.json at all: never configured, so the default (trust
        # without MYOUS_DEFAULT_REVIEW) - even a change runs.
        self.assertEqual(self.run_hook({"op": "exec", "id": "a", "cmd": "rm -rf x"})[0], 0)
        # Present but unreadable, not JSON, or an invalid level: fail closed
        # to "all", so even a read is asked about (W2 truncated review.json
        # to drop the worker to trust; this closes that escalation). A
        # default doesn't change that: the file is there.
        self.env["MYOUS_APPROVAL_WAIT"] = "1"
        self.env["MYOUS_DEFAULT_REVIEW"] = "trust"
        for bad in ["", "{", "not json", "[1, 2, 3]", '{"level": "bogus"}', '{"level": 7}', '"changes"', "null"]:
            (self.home / "review.json").write_text(bad)
            code, why = self.run_hook({"op": "get", "id": "b", "path": "x"})
            self.assertEqual((code, why), (1, "the owner didn't answer in time"), repr(bad))
        (self.home / "review.json").unlink()
        (self.home / "review.json").mkdir()   # there, but can't be read
        self.assertEqual(self.run_hook({"op": "get", "id": "b", "path": "x"}), (1, "the owner didn't answer in time"))
        (self.home / "review.json").rmdir()
        # A valid level is still honoured.
        self.level("changes")
        self.assertEqual(self.run_hook({"op": "get", "id": "c", "path": "x"})[0], 0)

    def test_default_level_for_an_unconfigured_worker(self):
        # No review.json: the level is $MYOUS_DEFAULT_REVIEW (the container's
        # entrypoint sets "changes"), else "trust".
        self.env["MYOUS_DEFAULT_REVIEW"] = "changes"
        self.env["MYOUS_APPROVAL_WAIT"] = "1"
        self.assertEqual(self.run_hook({"op": "exec", "id": "a", "cmd": "ls -la"})[0], 0)  # a read runs
        code, why = self.run_hook({"op": "exec", "id": "b", "cmd": "touch x"})            # a change asks
        self.assertEqual((code, why), (1, "the owner didn't answer in time"))
        self.assertFalse((self.home / "worker.log").exists())
        # Unset or empty: trust, a change runs.
        self.env["MYOUS_DEFAULT_REVIEW"] = ""
        self.assertEqual(self.run_hook({"op": "exec", "id": "c", "cmd": "rm -rf x"})[0], 0)
        del self.env["MYOUS_DEFAULT_REVIEW"]
        self.assertEqual(self.run_hook({"op": "exec", "id": "d", "cmd": "rm -rf x"})[0], 0)
        # Set but not a level: an unreadable safety setting asks about
        # everything, even a read, and worker.log says why.
        for val in ("bogus", "Trust", " changes"):
            self.env["MYOUS_DEFAULT_REVIEW"] = val
            code, why = self.run_hook({"op": "get", "id": "f", "path": "x"})
            self.assertEqual((code, why), (1, "the owner didn't answer in time"), repr(val))
            line = json.loads((self.home / "worker.log").read_text().splitlines()[-1])
            self.assertEqual((line["op"], line["value"]), ("review", val))
            self.assertIn("MYOUS_DEFAULT_REVIEW isn't trust, changes or all", line["error"])
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

    def test_a_stale_answer_does_not_approve_a_reused_id(self):
        # The owner once (late) approved id r1; the answer lingered. A new
        # request reusing r1 must still be asked, not auto-allowed.
        self.level("all")
        approvals = self.home / "approvals"
        approvals.mkdir()
        (approvals / "r1.answer").write_text("allow")
        self.env["MYOUS_APPROVAL_WAIT"] = "1"
        code, why = self.run_hook({"op": "exec", "id": "r1", "cmd": "rm -rf /work"})
        self.assertEqual((code, why), (1, "the owner didn't answer in time"))
        self.assertEqual(list(approvals.glob("*")), [])

    def test_ids_the_apps_cannot_answer_are_refused(self):
        self.level("all")
        bad = "bad request: the id must be 1 to 64 characters from A-Z, a-z, 0-9, - and _"
        for rid in ("x" * 65, "", "caf\u00e9", "a.b", "../x", 7, None):
            self.assertEqual(self.run_hook({"op": "exec", "id": rid, "cmd": "ls"}), (1, bad), repr(rid))
        self.assertEqual(list(self.home.glob("approvals/*")), [])


if __name__ == "__main__":
    unittest.main()

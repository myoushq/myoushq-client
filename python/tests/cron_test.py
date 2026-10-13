"""`myous cron` as a scheduled task on Windows (schtasks mocked, so it runs
anywhere), the crontab path elsewhere, and the listener restart's pid
check."""
import argparse
import io
import os
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from myous import cli, processes, vm
from myous.storage import FileStorage

REAL_PYTHON = sys.executable   # the tests below pretend it's elsewhere
NOT_FOUND = b"ERROR: The system cannot find the file specified.\r\n"


def task_xml(command: str, arguments: str) -> bytes:
    """What `schtasks /Query /XML` prints (UTF-16, as the task file is)."""
    xml = ('<?xml version="1.0" encoding="UTF-16"?>\r\n<Task version="1.2">\r\n  <Actions Context="Author">\r\n'
           f"    <Exec>\r\n      <Command>{command}</Command>\r\n      <Arguments>{arguments}</Arguments>\r\n"
           "    </Exec>\r\n  </Actions>\r\n</Task>\r\n")
    return xml.replace('"', "&quot;").replace("&quot;1.0&quot;", '"1.0"').encode("utf-16")


class Schtasks:
    """A stand-in for subprocess.run that answers schtasks calls."""

    def __init__(self, query=(1, b"", NOT_FOUND), create=(0, b"SUCCESS", b""), delete=(0, b"SUCCESS", b"")):
        self.answers = {"/Query": query, "/Create": create, "/Delete": delete}
        self.calls = []

    def __call__(self, argv, **kwargs):
        self.calls.append(argv)
        assert argv[0] == "schtasks", argv
        code, out, err = self.answers[argv[1]]
        return subprocess.CompletedProcess(argv, code, out, err)

    def verbs(self):
        return [c[1] for c in self.calls]


class TaskSchedulerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.st = FileStorage(root / ".myous")
        scripts = root / "venv" / "Scripts"
        scripts.mkdir(parents=True)
        (scripts / "python.exe").touch()
        (scripts / "pythonw.exe").touch()
        self.pythonw = str(scripts / "pythonw.exe")
        for patch in (mock.patch.object(vm, "WINDOWS", True),
                      mock.patch.object(vm, "DEFAULT_HOME", str(root / ".myous")),
                      mock.patch.object(sys, "executable", str(scripts / "python.exe"))):
            patch.start()
            self.addCleanup(patch.stop)

    def tearDown(self):
        self.tmp.cleanup()

    def run_with(self, fake, fn, *args):
        with mock.patch.object(vm.subprocess, "run", fake):
            return fn(*args)

    def wanted_tr(self):
        return f'"{self.pythonw}" "{self.st.path("ensure.pyw")}"'

    def test_install(self):
        fake = Schtasks()
        self.assertEqual(self.run_with(fake, vm.install_cron, self.st), "scheduled task installed")
        self.assertEqual(fake.calls, [
            ["schtasks", "/Query", "/TN", "myous ensure", "/XML"],
            ["schtasks", "/Create", "/F", "/SC", "MINUTE", "/MO", "1", "/TN", "myous ensure", "/TR", self.wanted_tr()],
        ])
        script = self.st.path("ensure.pyw").read_text(encoding="utf-8")
        self.assertIn(f"HOME = {str(self.st.home)!r}", script)
        self.assertIn('sys.argv = ["myous", "ensure", "--quiet"]', script)
        compile(script, "ensure.pyw", "exec")

    def test_ensure_pyw_logs_what_ensure_says(self):
        self.run_with(Schtasks(), vm.install_cron, self.st)
        # Not registered yet: ensure exits with a message, which must land
        # in cron.log (pythonw has nowhere else to print). It must not
        # depend on MYOUS_HOME being set by the caller.
        env = {k: v for k, v in os.environ.items() if k != "MYOUS_HOME"}
        r = subprocess.run([REAL_PYTHON, str(self.st.path("ensure.pyw"))], env=env,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual((r.returncode, r.stdout, r.stderr), (0, "", ""))
        self.assertIn("not registered with the hub yet", self.st.path("cron.log").read_text(encoding="utf-8"))

    def test_already_present(self):
        # schtasks splits /TR into Command and Arguments; quoting may differ.
        xml = task_xml(self.pythonw, f'"{self.st.path("ensure.pyw")}"')
        fake = Schtasks(query=(0, xml, b""))
        self.assertEqual(self.run_with(fake, vm.install_cron, self.st), "scheduled task already present")
        self.assertEqual(fake.verbs(), ["/Query"])
        self.assertTrue(self.st.path("ensure.pyw").exists())

    def test_replaces_a_different_task(self):
        fake = Schtasks(query=(0, task_xml(r"C:\Old\pythonw.exe", r'"C:\Old\ensure.pyw"'), b""))
        self.assertEqual(self.run_with(fake, vm.install_cron, self.st), "scheduled task installed")
        self.assertEqual(fake.verbs(), ["/Query", "/Create"])

    def test_show(self):
        xml = task_xml(self.pythonw, f'"{self.st.path("ensure.pyw")}"')
        shown = self.run_with(Schtasks(query=(0, xml, b"")), vm.show_cron, self.st)
        self.assertTrue(shown.startswith("myous ensure: "), shown)
        self.assertIn(str(self.st.path("ensure.pyw")), shown)
        self.assertEqual(self.run_with(Schtasks(), vm.show_cron, self.st), "no scheduled task")

    def test_remove(self):
        xml = task_xml(self.pythonw, f'"{self.st.path("ensure.pyw")}"')
        fake = Schtasks(query=(0, xml, b""))
        self.assertEqual(self.run_with(fake, vm.remove_cron, self.st), "scheduled task removed")
        self.assertEqual(fake.calls[-1], ["schtasks", "/Delete", "/F", "/TN", "myous ensure"])
        fake = Schtasks()
        self.assertEqual(self.run_with(fake, vm.remove_cron, self.st), "no scheduled task to remove")
        self.assertEqual(fake.verbs(), ["/Query"])

    def test_task_name_for_another_home(self):
        other = FileStorage(Path(self.tmp.name) / "agent2")
        self.assertEqual(vm.task_name(self.st), "myous ensure")
        self.assertEqual(vm.task_name(other), "myous ensure (agent2)")
        fake = Schtasks()
        self.run_with(fake, vm.install_cron, other)
        self.assertEqual(fake.calls[1][8:], ["myous ensure (agent2)", "/TR",
                                             f'"{self.pythonw}" "{other.path("ensure.pyw")}"'])
        self.assertIn(repr(str(other.home)), other.path("ensure.pyw").read_text(encoding="utf-8"))

    def test_not_available(self):
        def missing(argv, **kwargs):
            raise FileNotFoundError(2, "The system cannot find the file specified")
        for fn in (vm.install_cron, vm.show_cron, vm.remove_cron):
            out = self.run_with(missing, fn, self.st)
            self.assertTrue(out.startswith("Task Scheduler is not available here: "), out)
            self.assertIn("cannot find the file", out)
        denied = Schtasks(create=(1, b"", b"ERROR: Access is denied.\r\n"))
        self.assertEqual(self.run_with(denied, vm.install_cron, self.st),
                         "Task Scheduler is not available here: ERROR: Access is denied.")

    def test_no_pythonw(self):
        Path(self.pythonw).unlink()
        fake = Schtasks()
        out = self.run_with(fake, vm.install_cron, self.st)
        self.assertTrue(out.startswith("scheduled task installed (there is no pythonw.exe"), out)
        self.assertEqual(fake.calls[1][-1], f'"{sys.executable}" "{self.st.path("ensure.pyw")}"')

    def test_cli_passes_the_home(self):
        out = io.StringIO()
        other = FileStorage(Path(self.tmp.name) / "agent2")
        with mock.patch.object(vm.subprocess, "run", Schtasks()), redirect_stdout(out):
            cli.cmd_cron(None, other, argparse.Namespace(action="show"))
            cli.cmd_cron(None, other, argparse.Namespace(action="remove"))
        self.assertEqual(out.getvalue(), "no scheduled task\nno scheduled task to remove\n")


class CrontabTest(unittest.TestCase):
    """Off Windows, `myous cron` still edits the crontab."""

    def test_install_uses_crontab(self):
        with tempfile.TemporaryDirectory() as d, mock.patch.object(vm, "WINDOWS", False):
            st = FileStorage(d)
            calls = []

            def fake(argv, **kwargs):
                calls.append((argv, kwargs.get("input")))
                return subprocess.CompletedProcess(argv, 0, "MAILTO=x\n" if argv == ["crontab", "-l"] else "", "")
            with mock.patch.object(vm.subprocess, "run", fake):
                self.assertEqual(vm.install_cron(st), "cron entry installed")
            self.assertEqual(calls[1], (["crontab", "-"], f"MAILTO=x\n{vm.cron_line(st)}\n"))
            self.assertFalse(st.path("ensure.pyw").exists())


class ListenerRestartTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.st = FileStorage(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def restart(self, created):
        """Whether start_listener ends the saved pid (last heartbeat 10 minutes ago)."""
        self.st.put("listener", {"pid": 4242, "heartbeat": int(time.time()) - 600})
        with mock.patch.object(processes, "created_at", return_value=created), \
                mock.patch.object(processes, "terminate") as terminate, \
                mock.patch.object(processes, "spawn_background") as spawn:
            vm.start_listener(self.st)
        self.assertEqual(spawn.call_args.args[0], [sys.executable, "-m", "myous", "listen"])
        return terminate.called

    def test_ends_the_old_listener(self):
        self.assertTrue(self.restart(None))                    # can't tell: as before
        self.assertTrue(self.restart(time.time() - 3600))      # started before its heartbeat

    def test_leaves_a_reused_pid_alone(self):
        self.assertFalse(self.restart(time.time() - 60))       # started after the listener's last heartbeat


class TrimLogsTest(unittest.TestCase):
    def test_trims_bytes_in_any_encoding(self):
        with tempfile.TemporaryDirectory() as d:
            st = FileStorage(d)
            st.path("cron.log").write_bytes(b"\x81\xff" * vm.MAX_LOG_BYTES)
            vm.trim_logs(st)
            self.assertEqual(st.path("cron.log").stat().st_size, vm.MAX_LOG_BYTES // 2)


if __name__ == "__main__":
    unittest.main()

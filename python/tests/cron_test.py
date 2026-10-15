"""`myous cron` as a scheduled task on Windows (schtasks mocked, so it runs
anywhere), the crontab path elsewhere, and the listener restart's pid
check."""
import argparse
import datetime
import io
import os
import subprocess
import sys
import tempfile
import time
import unittest
import xml.etree.ElementTree as ET
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from myous import cli, processes, vm
from myous.storage import FileStorage

REAL_PYTHON = sys.executable   # the tests below pretend it's elsewhere
NOT_FOUND = b"ERROR: The system cannot find the file specified.\r\n"


NS = {"t": vm.TASK_NS}


def task_xml(command: str, arguments: str, current: bool = True) -> bytes:
    """What `schtasks /Query /XML` prints (UTF-16, as the task file is);
    not current: a task as /SC MINUTE made it (no logon trigger, the
    battery defaults)."""
    battery = "false" if current else "true"
    logon = "    <LogonTrigger>\r\n      <Enabled>true</Enabled>\r\n    </LogonTrigger>\r\n" if current else ""
    xml = ('<?xml version="1.0" encoding="UTF-16"?>\r\n<Task version="1.2">\r\n'
           f"  <Triggers>\r\n    <TimeTrigger>\r\n    </TimeTrigger>\r\n{logon}  </Triggers>\r\n"
           f"  <Settings>\r\n    <DisallowStartIfOnBatteries>{battery}</DisallowStartIfOnBatteries>\r\n"
           f"    <StopIfGoingOnBatteries>{battery}</StopIfGoingOnBatteries>\r\n  </Settings>\r\n"
           '  <Actions Context="Author">\r\n'
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
        if "/XML" in argv and argv[1] == "/Create":
            # The definition as schtasks reads it, at the time it does.
            self.definition = Path(argv[argv.index("/XML") + 1]).read_bytes()
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
                      mock.patch.object(sys, "executable", str(scripts / "python.exe")),
                      mock.patch.dict(os.environ, {"USERDOMAIN": "LAPTOP", "USERNAME": "max"})):
            patch.start()
            self.addCleanup(patch.stop)

    def tearDown(self):
        self.tmp.cleanup()

    def run_with(self, fake, fn, *args):
        with mock.patch.object(vm.subprocess, "run", fake):
            return fn(*args)

    def definition(self, fake):
        """The task definition schtasks was given, parsed."""
        return ET.fromstring(fake.definition)

    def test_install(self):
        fake = Schtasks()
        self.assertEqual(self.run_with(fake, vm.install_cron, self.st), "scheduled task installed")
        xml_file = str(self.st.path("ensure-task.xml"))
        self.assertEqual(fake.calls, [
            ["schtasks", "/Query", "/TN", "myous ensure", "/XML"],
            ["schtasks", "/Create", "/F", "/XML", xml_file, "/TN", "myous ensure"],
        ])
        action = self.definition(fake).find("t:Actions/t:Exec", NS)
        self.assertEqual(action.findtext("t:Command", namespaces=NS), self.pythonw)
        self.assertEqual(action.findtext("t:Arguments", namespaces=NS), f'"{self.st.path("ensure.pyw")}"')
        script = self.st.path("ensure.pyw").read_text(encoding="utf-8")
        self.assertIn(f"HOME = {str(self.st.home)!r}", script)
        self.assertIn('sys.argv = ["myous", "ensure", "--quiet"]', script)
        compile(script, "ensure.pyw", "exec")

    def test_definition_settings(self):
        fake = Schtasks()
        self.run_with(fake, vm.install_cron, self.st)
        task = self.definition(fake)
        self.assertEqual(task.tag, f"{{{vm.TASK_NS}}}Task")
        settings = {e.tag.split("}")[1]: e.text for e in task.find("t:Settings", NS)}
        for key, value in {"DisallowStartIfOnBatteries": "false", "StopIfGoingOnBatteries": "false",
                           "MultipleInstancesPolicy": "IgnoreNew", "StartWhenAvailable": "true",
                           "ExecutionTimeLimit": "PT10M", "AllowHardTerminate": "true",
                           "RunOnlyIfNetworkAvailable": "false", "RunOnlyIfIdle": "false",
                           "Enabled": "true", "Hidden": "false"}.items():
            self.assertEqual(settings[key], value, key)
        principal = task.find("t:Principals/t:Principal", NS)
        self.assertEqual(principal.findtext("t:UserId", namespaces=NS), "LAPTOP\\max")
        self.assertEqual(principal.findtext("t:LogonType", namespaces=NS), "InteractiveToken")
        self.assertEqual(principal.findtext("t:RunLevel", namespaces=NS), "LeastPrivilege")
        self.assertEqual(task.find("t:Actions", NS).get("Context"), principal.get("id"))
        self.assertIsNone(principal.find("t:Password", NS))

    def test_definition_triggers(self):
        fake = Schtasks()
        before = datetime.datetime.now().replace(microsecond=0)
        self.run_with(fake, vm.install_cron, self.st)
        triggers = self.definition(fake).find("t:Triggers", NS)
        self.assertEqual([t.tag.split("}")[1] for t in triggers], ["TimeTrigger", "LogonTrigger"])
        timer, logon = triggers
        start = datetime.datetime.fromisoformat(timer.findtext("t:StartBoundary", namespaces=NS))
        self.assertTrue(before <= start <= datetime.datetime.now(), start)
        self.assertEqual(logon.findtext("t:UserId", namespaces=NS), "LAPTOP\\max")
        for trigger in triggers:
            self.assertEqual(trigger.findtext("t:Repetition/t:Interval", namespaces=NS), "PT1M")
            self.assertIsNone(trigger.find("t:Repetition/t:Duration", NS))   # indefinitely
            # The schema's order for a trigger (a sequence).
            order = ["Enabled", "StartBoundary", "EndBoundary", "Repetition", "ExecutionTimeLimit", "UserId", "Delay"]
            tags = [e.tag.split("}")[1] for e in trigger]
            self.assertEqual(tags, sorted(tags, key=order.index))

    def test_definition_file_is_utf16_with_bom(self):
        fake = Schtasks()
        self.run_with(fake, vm.install_cron, self.st)
        data = fake.definition
        self.assertTrue(data.startswith(b"\xff\xfe<\x00?\x00x\x00m\x00l\x00"), data[:12])
        text = data[2:].decode("utf-16-le")
        self.assertTrue(text.startswith('<?xml version="1.0" encoding="UTF-16"?>\r\n'), text[:60])
        self.assertNotIn("\n", text.replace("\r\n", ""))
        self.assertEqual(data, self.st.path("ensure-task.xml").read_bytes())

    def test_definition_escapes(self):
        st = FileStorage(Path(self.tmp.name) / "a&b <c>")
        task = ET.fromstring(vm.task_definition(st, "PC\\o'neil & co", datetime.datetime(2026, 10, 14, 9, 30, 5, 7)))
        self.assertEqual(task.findtext("t:Principals/t:Principal/t:UserId", namespaces=NS), "PC\\o'neil & co")
        self.assertEqual(task.findtext("t:Actions/t:Exec/t:Arguments", namespaces=NS), f'"{st.path("ensure.pyw")}"')
        self.assertEqual(task.findtext("t:Triggers/t:TimeTrigger/t:StartBoundary", namespaces=NS), "2026-10-14T09:30:05")

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

    def test_replaces_a_task_from_before_the_definition(self):
        # Same command, but made with /SC MINUTE: not on battery, no logon trigger.
        xml = task_xml(self.pythonw, f'"{self.st.path("ensure.pyw")}"', current=False)
        fake = Schtasks(query=(0, xml, b""))
        self.assertEqual(self.run_with(fake, vm.install_cron, self.st), "scheduled task installed")
        self.assertEqual(fake.verbs(), ["/Query", "/Create"])
        self.assertIn("/XML", fake.calls[1])

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
        self.assertEqual(fake.calls[1][3:], ["/XML", str(other.path("ensure-task.xml")), "/TN", "myous ensure (agent2)"])
        self.assertEqual(self.definition(fake).findtext("t:Actions/t:Exec/t:Arguments", namespaces=NS),
                         f'"{other.path("ensure.pyw")}"')
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
        self.assertEqual(self.definition(fake).findtext("t:Actions/t:Exec/t:Command", namespaces=NS), sys.executable)

    def test_cli_passes_the_home(self):
        out = io.StringIO()
        other = FileStorage(Path(self.tmp.name) / "agent2")
        with mock.patch.object(vm.subprocess, "run", Schtasks()), redirect_stdout(out):
            cli.cmd_cron(None, other, argparse.Namespace(action="show"))
            cli.cmd_cron(None, other, argparse.Namespace(action="remove"))
        self.assertEqual(out.getvalue(), "no scheduled task\nno scheduled task to remove\n")


@unittest.skipUnless(sys.platform == "win32" and os.environ.get("GITHUB_ACTIONS"),
                     "registers a real task: Windows CI only")
class RealTaskSchedulerTest(unittest.TestCase):
    """schtasks itself accepts the definition (encoding, schema, the user's
    account without a password) and keeps its settings."""

    NAME = f"myous ci test {os.getpid()}"

    def test_schtasks_accepts_the_definition(self):
        with tempfile.TemporaryDirectory() as d:
            st = FileStorage(d)
            path = Path(d) / "task.xml"
            vm.write_task_definition(path, vm.task_definition(st, vm._windows_user()))
            r = subprocess.run(["schtasks", "/Create", "/F", "/XML", str(path), "/TN", self.NAME],
                               stdin=subprocess.DEVNULL, capture_output=True)
            self.assertEqual(r.returncode, 0, vm._text(r.stderr) + vm._text(r.stdout))
            try:
                query = subprocess.run(["schtasks", "/Query", "/TN", self.NAME, "/XML"],
                                       stdin=subprocess.DEVNULL, capture_output=True)
                self.assertEqual(query.returncode, 0)
                self.assertTrue(vm._task_current(query), vm._text(query.stdout))
                self.assertIn(str(st.path("ensure.pyw")), vm._task_line(query))
            finally:
                subprocess.run(["schtasks", "/Delete", "/F", "/TN", self.NAME],
                               stdin=subprocess.DEVNULL, capture_output=True)


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



class Crontab:
    """A stand-in for subprocess.run that keeps a crontab."""

    def __init__(self, lines=()):
        self.lines = list(lines)

    def __call__(self, argv, **kwargs):
        if argv == ["crontab", "-l"]:
            return subprocess.CompletedProcess(argv, 0, "".join(line + "\n" for line in self.lines), "")
        assert argv == ["crontab", "-"], argv
        self.lines = kwargs["input"].splitlines()
        return subprocess.CompletedProcess(argv, 0, "", "")


class CrontabPerHomeTest(unittest.TestCase):
    """One crontab line per home, as Windows has one task per home."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.a = FileStorage(Path(self.tmp.name) / "a")
        self.b = FileStorage(Path(self.tmp.name) / "b")
        for patch in (mock.patch.object(vm, "WINDOWS", False),
                      mock.patch.object(vm, "DEFAULT_HOME", str(Path(self.tmp.name) / "a"))):
            patch.start()
            self.addCleanup(patch.stop)

    def tearDown(self):
        self.tmp.cleanup()

    def run_with(self, fake, fn, *args):
        with mock.patch.object(vm.subprocess, "run", fake):
            return fn(*args)

    def test_line_names_its_home(self):
        self.assertTrue(vm.cron_line(self.a).endswith(f" # myous {self.a.home}"))

    def test_two_homes_get_a_line_each(self):
        own = "0 3 * * * backup.sh # myoushq is not us"
        fake = Crontab(["MAILTO=x", own])
        self.assertEqual(self.run_with(fake, vm.install_cron, self.a), "cron entry installed")
        self.assertEqual(self.run_with(fake, vm.install_cron, self.b), "cron entry installed")
        self.assertEqual(fake.lines, ["MAILTO=x", own, vm.cron_line(self.a), vm.cron_line(self.b)])
        self.assertEqual(self.run_with(fake, vm.install_cron, self.a), "cron entry already present")
        self.assertEqual(self.run_with(fake, vm.show_cron, self.b),
                         f"{vm.cron_line(self.a)}\n{vm.cron_line(self.b)}")
        self.assertEqual(self.run_with(fake, vm.remove_cron, self.a), "cron entry removed")
        self.assertEqual(fake.lines, ["MAILTO=x", own, vm.cron_line(self.b)])
        self.assertEqual(self.run_with(fake, vm.remove_cron, self.a), "no cron entry to remove")
        self.assertEqual(self.run_with(fake, vm.remove_cron, self.b), "cron entry removed")
        self.assertEqual(fake.lines, ["MAILTO=x", own])
        self.assertEqual(self.run_with(fake, vm.show_cron, self.a), "no cron entry")

    def test_reinstall_replaces_this_homes_line(self):
        old = f"* * * * * MYOUS_HOME={self.a.home} /old/python -m myous ensure --quiet # myous {self.a.home}"
        fake = Crontab([old, vm.cron_line(self.b)])
        self.assertEqual(self.run_with(fake, vm.install_cron, self.a), "cron entry installed")
        self.assertEqual(fake.lines, [vm.cron_line(self.b), vm.cron_line(self.a)])

    def test_unmarked_line_from_before_is_this_homes(self):
        legacy = f"* * * * * MYOUS_HOME={self.b.home} /usr/bin/python3 -m myous ensure --quiet >> x 2>&1 # myous"
        fake = Crontab(["MAILTO=x", legacy])
        self.assertEqual(self.run_with(fake, vm.show_cron, self.a), legacy)
        self.assertEqual(self.run_with(fake, vm.install_cron, self.a), "cron entry installed")
        self.assertEqual(fake.lines, ["MAILTO=x", vm.cron_line(self.a)])
        fake = Crontab([legacy, vm.cron_line(self.b)])
        self.assertEqual(self.run_with(fake, vm.remove_cron, self.a), "cron entry removed")
        self.assertEqual(fake.lines, [vm.cron_line(self.b)])

    def test_without_a_home_means_the_default(self):
        fake = Crontab([vm.cron_line(self.a), vm.cron_line(self.b)])
        with mock.patch.dict(os.environ, {"MYOUS_HOME": ""}):
            self.assertEqual(self.run_with(fake, vm.remove_cron, None), "cron entry removed")
        self.assertEqual(fake.lines, [vm.cron_line(self.b)])

    def test_cli_passes_the_home(self):
        fake = Crontab([vm.cron_line(self.a)])
        out = io.StringIO()
        with mock.patch.object(vm.subprocess, "run", fake), redirect_stdout(out):
            cli.cmd_cron(None, self.b, argparse.Namespace(action="install"))
            cli.cmd_cron(None, self.b, argparse.Namespace(action="remove"))
        self.assertEqual(out.getvalue(), "cron entry installed\ncron entry removed\n")
        self.assertEqual(fake.lines, [vm.cron_line(self.a)])

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


class StopListenerTest(unittest.TestCase):
    """`myous listen --stop`: this directory's listener, never a process
    that got its pid later."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.st = FileStorage(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def start(self):
        p = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        self.addCleanup(lambda: (p.kill(), p.wait()))
        reaper = __import__("threading").Thread(target=p.wait, daemon=True)   # no zombie to look alive
        reaper.start()
        return p

    def stop(self) -> str:
        args = cli.build_parser().parse_args(["listen", "--stop"])
        out = io.StringIO()
        with redirect_stdout(out):
            args.func(None, self.st, args)
        return out.getvalue()

    def test_stops_this_directorys_listener(self):
        p = self.start()
        self.st.put("listener", {"pid": p.pid, "heartbeat": int(time.time())})
        self.assertIn(f"stopped the listener (pid {p.pid})", self.stop())
        self.assertFalse(processes.alive(p.pid))
        self.assertIsNone(self.st.get("listener"))
        self.assertEqual(self.stop(), "no listener was running for this directory\n")

    def test_leaves_a_reused_pid_alone(self):
        p = self.start()
        self.st.put("listener", {"pid": p.pid, "heartbeat": int(time.time()) - 600})   # it started after that
        self.assertEqual(self.stop(), "no listener was running for this directory\n")
        self.assertTrue(processes.alive(p.pid))


class TrimLogsTest(unittest.TestCase):
    def test_trims_bytes_in_any_encoding(self):
        with tempfile.TemporaryDirectory() as d:
            st = FileStorage(d)
            st.path("cron.log").write_bytes(b"\x81\xff" * vm.MAX_LOG_BYTES)
            vm.trim_logs(st)
            self.assertEqual(st.path("cron.log").stat().st_size, vm.MAX_LOG_BYTES // 2)


if __name__ == "__main__":
    unittest.main()

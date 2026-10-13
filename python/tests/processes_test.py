"""The shell, stopping and pid checks on this system (CI runs it on Linux,
macOS and Windows)."""
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from myous import processes
from myous.storage import FileStorage
from myous.worker import run_command

WIN = processes.WINDOWS


class ShellTest(unittest.TestCase):
    def run_(self, cmd, timeout=30):
        with tempfile.TemporaryDirectory() as d:
            return run_command(cmd, Path(d), timeout)

    def test_output_and_exit(self):
        r = self.run_("Write-Output 'héllo'; exit 3" if WIN else "echo héllo; exit 3")
        self.assertEqual(r["exit"], 3)
        self.assertEqual(r["stdout"].strip(), "héllo")

    def test_failing_program_sets_exit(self):
        py = f'& "{sys.executable}"' if WIN else f'"{sys.executable}"'
        r = self.run_(f"{py} -c \"import sys; sys.exit(7)\"")
        self.assertEqual(r["exit"], 7)

    def test_success_is_zero(self):
        r = self.run_("Get-Location | Out-Null" if WIN else "true")
        self.assertEqual(r["exit"], 0, r)

    def test_timeout_ends_the_children(self):
        started = time.time()
        r = self.run_("Start-Sleep -Seconds 30" if WIN else "sleep 30", timeout=2)
        self.assertLess(time.time() - started, 20)
        self.assertEqual(r["exit"], -1)
        self.assertIn("killed after 2 seconds", r["stderr"])

    def test_shell_name(self):
        self.assertEqual(processes.shell_name(), "PowerShell" if WIN else "/bin/sh")


class PidTest(unittest.TestCase):
    def test_alive(self):
        self.assertTrue(processes.alive(os.getpid()))
        p = subprocess.Popen([sys.executable, "-c", "pass"])
        p.wait()
        self.assertFalse(processes.alive(p.pid))

    def test_terminate(self):
        p = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        self.assertTrue(processes.alive(p.pid))
        processes.terminate(p.pid)
        p.wait(timeout=10)
        self.assertFalse(processes.alive(p.pid))


class LockTest(unittest.TestCase):
    def test_second_holder_waits(self):
        with tempfile.TemporaryDirectory() as d:
            st = FileStorage(d)
            with st.lock("state") as got:
                self.assertTrue(got)
                code = ("import sys; from myous.storage import FileStorage\n"
                        f"with FileStorage({d!r}).lock('state', wait=False) as got: sys.exit(0 if got else 9)")
                self.assertEqual(subprocess.run([sys.executable, "-c", code]).returncode, 9)
            self.assertEqual(subprocess.run([sys.executable, "-c", code]).returncode, 0)


if __name__ == "__main__":
    unittest.main()


class BackgroundTest(unittest.TestCase):
    def test_spawn_background(self):
        with tempfile.TemporaryDirectory() as d:
            out = Path(d) / "out"
            p = processes.spawn_background([sys.executable, "-c", f"open({str(out)!r}, 'w').write('ran')"],
                                           stdin=subprocess.DEVNULL)
            p.wait(timeout=30)
            self.assertEqual(out.read_text(), "ran")

    def test_created_at(self):
        created = processes.created_at(os.getpid())
        if not WIN:
            self.assertIsNone(created)
            return
        # This process started before now, and not long before.
        self.assertLess(created, time.time() + 1)
        self.assertGreater(created, time.time() - 3600)
        p = subprocess.Popen([sys.executable, "-c", "pass"])
        p.wait()
        self.assertIsNone(processes.created_at(p.pid))

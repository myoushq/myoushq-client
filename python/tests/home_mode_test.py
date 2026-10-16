"""Who can read a myous directory on a shared machine. mkdir's mode only
applies to a directory it creates: one that already exists (made by
`python3 -m venv "$MYOUS_HOME/venv"` before the first command, as the
source build does, or by hand) is usually 0755, and then every file
written without a mode of its own (the logs, the invite's QR image with
the pairing link, a worker's request records) is readable by every
user. Unix only: Windows has no modes."""
import os
import stat
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

from myous import qr, vm
from myous.storage import FileStorage


@unittest.skipIf(os.name == "nt", "no modes on Windows")
class HomeMode(unittest.TestCase):
    def setUp(self):
        self.old = os.umask(0o022)
        self.addCleanup(os.umask, self.old)
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.home = Path(tmp.name) / "home"
        self.home.mkdir(mode=0o755)
        (self.home / "venv").mkdir()

    def mode(self, p: Path) -> int:
        return stat.S_IMODE(p.stat().st_mode)

    def test_an_existing_home_becomes_private(self):
        FileStorage(self.home)
        self.assertEqual(self.mode(self.home), 0o700)

    def test_logs_are_private(self):
        st = FileStorage(self.home)
        os.chmod(self.home, 0o755)   # as if made private some other way, or not at all
        st.put("settings", {"on_message": "true"})
        vm.run_hook(st, [{"type": "message", "direction": "in", "text": "hi", "seq": 1, "at": 1}])
        for _ in range(50):
            if (self.home / "hook.log").exists():
                break
            time.sleep(0.05)
        self.assertEqual(self.mode(self.home / "hook.log"), 0o600)
        with mock.patch.object(vm, "_crontab", return_value=[]), mock.patch.object(vm.subprocess, "run"):
            vm.install_cron(st)
        self.assertEqual(self.mode(self.home / "cron.log"), 0o600, "made before cron's shell appends to it")
        with mock.patch.object(vm.processes, "spawn_background"):
            vm.start_listener(st)
        self.assertEqual(self.mode(self.home / "listener.log"), 0o600)

    def test_the_invite_qr_image_is_private(self):
        try:
            import qrcode  # noqa: F401
        except ImportError:
            self.skipTest("qrcode not installed")
        svg = self.home / "invite-4821.svg"
        qr.write_svg("https://hub.test/p/4821#K7F3QX", str(svg))
        self.assertEqual(self.mode(svg), 0o600)
        self.assertIn(b"<svg", svg.read_bytes()[:200])


if __name__ == "__main__":
    unittest.main()

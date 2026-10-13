"""`myous hook-script [muse|claude-code]`: the paths of the hook scripts
shipped as package data.

The Claude Code hooks have two copies: examples/claude-code/ is the source
(the readable example the guide and its tests use); myous/claude_code/ is a
copy shipped in the package. test_claude_code_copies_match keeps them
byte-identical: change the example, then copy it over."""
from __future__ import annotations

import io
import os
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from myous import cli

ROOT = Path(__file__).resolve().parents[2]


class HookScriptTest(unittest.TestCase):
    def run_cmd(self, *argv: str, os_name: str | None = None) -> str:
        """Run the command; os_name stands in for os.name while it runs."""
        args = cli.build_parser().parse_args(["hook-script", *argv])
        out = io.StringIO()
        with redirect_stdout(out):
            if os_name is None:
                args.func(None, None, args)
            else:
                with mock.patch.object(cli, "os", SimpleNamespace(name=os_name)):
                    args.func(None, None, args)
        return out.getvalue().strip()

    def assert_script(self, path: str, suffix: str):
        self.assertTrue(os.path.isabs(path), path)
        self.assertTrue(Path(path).as_posix().endswith(suffix), path)
        self.assertTrue(Path(path).is_file(), path)

    def test_default_is_muse(self):
        path = self.run_cmd()
        self.assert_script(path, "myous/muse/hook.sh")
        self.assertEqual(self.run_cmd("muse"), path)

    def test_claude_code_posix(self):
        self.assert_script(self.run_cmd("claude-code", os_name="posix"), "myous/claude_code/inbox-hook.sh")

    def test_claude_code_windows(self):
        self.assert_script(self.run_cmd("claude-code", os_name="nt"), "myous/claude_code/inbox-hook.ps1")

    def test_claude_code_current_platform(self):
        name = "inbox-hook.ps1" if os.name == "nt" else "inbox-hook.sh"
        self.assert_script(self.run_cmd("claude-code"), "myous/claude_code/" + name)

    def test_unknown_agent_rejected(self):
        with redirect_stdout(io.StringIO()), mock.patch("sys.stderr", io.StringIO()):
            with self.assertRaises(SystemExit):
                cli.build_parser().parse_args(["hook-script", "other"])

    def test_claude_code_copies_match(self):
        package = Path(cli.__file__).resolve().parent / "claude_code"
        for name in ("inbox-hook.sh", "inbox-hook.ps1"):
            source = ROOT / "examples" / "claude-code" / name
            if not source.is_file():
                self.skipTest("not in a checkout of the repository")
            self.assertEqual((package / name).read_bytes(), source.read_bytes(),
                             f"myous/claude_code/{name} differs from examples/claude-code/{name}; copy the example over")


if __name__ == "__main__":
    unittest.main()

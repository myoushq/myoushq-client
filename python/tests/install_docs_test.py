"""The install blocks in skill.md and muse.md put `latest_release` from
the hub's config.json into the download addresses. Run as written, with
a fake `curl` and `pip`: a value that isn't a version number (one that
walks up the address to another repository's release, say) must stop
them before anything is downloaded or installed."""
import os
import re
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

DOCS = Path(__file__).resolve().parents[2] / "docs"
CRAFTED = "/../../../../../attacker/repo/releases/download/t"


def block(doc: str, after: str) -> str:
    """The first ```sh block after the line `after`."""
    text = (DOCS / doc).read_text()
    return re.search(r"```sh\n(.*?)```", text[text.index(after):], re.S).group(1)


class InstallBlocks(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="myous-install-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.bin, self.log = self.tmp / "bin", self.tmp / "calls.log"
        self.bin.mkdir()
        # curl: config.json from $FAKE_CONFIG; anything else is logged (and -o written).
        self.script(self.bin / "curl", '''for a; do case $a in *config.json) cat "$FAKE_CONFIG"; exit 0;; esac; done
echo "curl $*" >> "$CALLS"; out=; prev=
for a; do [ "$prev" = -o ] && out=$a; prev=$a; done
[ -n "$out" ] && : > "$out"; exit 0''')

    def script(self, path: Path, body: str):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def run_block(self, code: str, latest: str, home: Path) -> str:
        # A venv already there, with a pip that logs what it was asked.
        self.script(home / "venv/bin/python", "exit 0")
        self.script(home / "venv/bin/pip", 'echo "pip $*" >> "$CALLS"')
        self.script(home / "venv/bin/myous", "exit 0")
        (self.tmp / "config.json").write_text('{"latest_release": "%s"}' % latest)
        env = dict(os.environ, HOME=str(self.tmp), MYOUS_HOME=str(home), PATH=f"{self.bin}:{os.environ['PATH']}",
                   FAKE_CONFIG=str(self.tmp / "config.json"), CALLS=str(self.log))
        subprocess.run(["sh", "-c", code], env=env, capture_output=True, text=True, timeout=60)
        return self.log.read_text() if self.log.exists() else ""

    def check(self, doc: str, after: str, args: list[str] = ()):
        code = block(doc, after)
        if args:
            code = "set -- " + " ".join(args) + "\n" + code
        home = self.tmp / ".myous"
        calls = self.run_block(code, "v" + CRAFTED, home)
        self.assertEqual(calls, "", f"{doc} downloaded or installed with latest_release {CRAFTED!r}:\n{calls}")
        calls = self.run_block(code, "v0.7.5", home)
        self.assertIn("releases/download/v0.7.5/requirements-python.lock", calls)
        self.assertIn("myous==0.7.5", calls)

    def test_skill_md(self):
        self.check("skill.md", "### Install a published package")

    def test_muse_md(self):
        self.check("muse.md", "Save this as a script", ["Sam"])


if __name__ == "__main__":
    unittest.main()

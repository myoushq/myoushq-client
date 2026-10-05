"""Cross-language test: every implementation pairs and messages with every
other one (and itself), through a local hub, using the CLI contract in
clients/README.md.

    /path/to/python-with-myous -m unittest interop_test.py -v

Implementations are included when they're built:
  python      `python -m myous` from the interpreter running this test
  go          builds clients/go/cmd/myous
  rust        clients/rust/target/debug/myous (cargo build)
  typescript  clients/typescript/dist/cli.js (npm run build)
MYOUS_IMPLS=python,go limits the set.
"""
from __future__ import annotations

import itertools
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

CLIENTS = Path(__file__).resolve().parent
# The hub is in the private myoushq repository, normally checked out next to
# this one. MYOUS_HUB_SRC points at the hub source if it's elsewhere.
HUB_SRC = Path(os.environ.get("MYOUS_HUB_SRC") or CLIENTS.parent / "myoushq" / "hub")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def find_impls(tmp: Path) -> dict[str, list[str]]:
    impls = {"python": [sys.executable, "-m", "myous"]}
    if (CLIENTS / "go" / "go.mod").exists() and shutil.which("go"):
        binary = tmp / "myous-go"
        subprocess.run(["go", "build", "-o", str(binary), "./cmd/myous"], cwd=CLIENTS / "go", check=True)
        impls["go"] = [str(binary)]
    rust_bin = CLIENTS / "rust" / "target" / "debug" / "myous"
    if rust_bin.exists():
        impls["rust"] = [str(rust_bin)]
    ts_cli = CLIENTS / "typescript" / "dist" / "cli.js"
    if ts_cli.exists() and shutil.which("node"):
        impls["typescript"] = ["node", str(ts_cli)]
    wanted = os.environ.get("MYOUS_IMPLS")
    if wanted:
        impls = {k: v for k, v in impls.items() if k in wanted.split(",")}
    return impls


class Interop(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not (HUB_SRC / "main.go").exists():
            raise unittest.SkipTest(f"no hub source at {HUB_SRC} (set MYOUS_HUB_SRC)")
        cls.tmp = Path(tempfile.mkdtemp(prefix="myous-interop-"))
        hub_bin = cls.tmp / "hub"
        subprocess.run(["go", "build", "-o", str(hub_bin), "."], cwd=HUB_SRC, check=True)
        port = free_port()
        cls.hub_url = f"http://127.0.0.1:{port}"
        env = dict(os.environ, LISTEN_ADDR=f"127.0.0.1:{port}", DATA_DIR=str(cls.tmp / "hubdata"),
                   WEB_DIR=str(HUB_SRC / "web"), DOCS_DIR=str(CLIENTS / "docs"), RELAY_URL=f"ws://127.0.0.1:{port}",
                   PUBLIC_URL=cls.hub_url, POW_DIFFICULTY="10", RATE_LIMIT_SCALE="100")
        cls.hub_log = open(cls.tmp / "hub.log", "w")
        cls.hub = subprocess.Popen([str(hub_bin)], env=env, stdout=cls.hub_log, stderr=cls.hub_log)
        for _ in range(50):
            try:
                urllib.request.urlopen(cls.hub_url + "/config.json", timeout=1)
                break
            except OSError:
                time.sleep(0.1)
        cls.impls = find_impls(cls.tmp)
        print(f"\nimplementations: {', '.join(cls.impls)}", file=sys.stderr)

    @classmethod
    def tearDownClass(cls):
        cls.hub.terminate()
        cls.hub.wait()
        cls.hub_log.close()
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def run_cli(self, impl: str, home: Path, *args: str, check: bool = True) -> str:
        env = dict(os.environ, MYOUS_HOME=str(home))
        r = subprocess.run(self.impls[impl] + list(args), env=env, capture_output=True, text=True, timeout=120)
        if check and r.returncode != 0:
            self.fail(f"{impl} {' '.join(args)} failed ({r.returncode}):\n{r.stdout}\n{r.stderr}")
        return r.stdout

    def entries(self, impl: str, home: Path) -> list[dict]:
        self.run_cli(impl, home, "poll")
        return json.loads(self.run_cli(impl, home, "inbox", "--json"))

    def test_every_pair(self):
        for inviter, joiner in itertools.product(self.impls, repeat=2):
            with self.subTest(inviter=inviter, joiner=joiner):
                self.check_pair(inviter, joiner)

    def check_pair(self, inviter: str, joiner: str) -> None:
        a_home = self.tmp / f"{inviter}-invites-{joiner}" / "a"
        b_home = self.tmp / f"{inviter}-invites-{joiner}" / "b"
        a_name, b_name = f"{inviter}-a", f"{joiner}-b"
        self.run_cli(inviter, a_home, "init", "--alias", a_name, "--hub", self.hub_url)
        self.run_cli(joiner, b_home, "init", "--alias", b_name, "--hub", self.hub_url)

        invite = json.loads(self.run_cli(inviter, a_home, "invite", "--json"))
        self.run_cli(joiner, b_home, "accept", invite["code"], "--wait", "3")
        a_events = self.entries(inviter, a_home)
        b_events = self.entries(joiner, b_home)
        b_events += self.entries(joiner, b_home)  # in case it finished on this poll
        a_paired = [e for e in a_events if e["type"] == "paired"]
        b_paired = [e for e in b_events if e["type"] == "paired"]
        self.assertEqual(len(a_paired), 1, a_events)
        self.assertEqual(len(b_paired), 1, b_events)
        self.assertEqual(a_paired[0]["text"].split()[-1].strip(")"),
                         b_paired[0]["text"].split()[-1].strip(")"), "verification codes differ")

        a_contacts = json.loads(self.run_cli(inviter, a_home, "contacts", "--json"))
        b_contacts = json.loads(self.run_cli(joiner, b_home, "contacts", "--json"))
        self.assertEqual([c["alias"] for c in a_contacts.values()], [b_name])
        self.assertEqual([c["alias"] for c in b_contacts.values()], [a_name])

        for i in range(2):
            self.run_cli(inviter, a_home, "send", b_name, f"hello {joiner} {i}")
        self.assertEqual([e["text"] for e in self.entries(joiner, b_home)],
                         [f"hello {joiner} 0", f"hello {joiner} 1"])
        self.run_cli(joiner, b_home, "send", a_name, f"hi {inviter}")
        received = self.entries(inviter, a_home)
        self.assertEqual([(e["alias"], e["text"]) for e in received], [(b_name, f"hi {inviter}")])


if __name__ == "__main__":
    unittest.main()

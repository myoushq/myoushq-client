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

import base64
import itertools
import json
import os
import select
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.parse
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
    HUB_HOST = "127.0.0.1"  # the name clients use for the hub and relay
    LATEST_RELEASE = "v0.0.1"  # what the hub announces; older than every client, so no notices

    @classmethod
    def setUpClass(cls):
        if not (HUB_SRC / "main.go").exists():
            raise unittest.SkipTest(f"no hub source at {HUB_SRC} (set MYOUS_HUB_SRC)")
        cls.tmp = Path(tempfile.mkdtemp(prefix="myous-interop-"))
        hub_bin = cls.tmp / "hub"
        subprocess.run(["go", "build", "-o", str(hub_bin), "."], cwd=HUB_SRC, check=True)
        port = free_port()
        cls.hub_url = f"http://{cls.HUB_HOST}:{port}"
        env = dict(os.environ, LISTEN_ADDR=f"127.0.0.1:{port}", DATA_DIR=str(cls.tmp / "hubdata"),
                   WEB_DIR=str(HUB_SRC / "web"), DOCS_DIR=str(CLIENTS / "docs"),
                   RELAY_URL=f"ws://{cls.HUB_HOST}:{port}",
                   PUBLIC_URL=cls.hub_url, POW_DIFFICULTY="10", RATE_LIMIT_SCALE="100",
                   LATEST_RELEASE=cls.LATEST_RELEASE, **cls.extra_hub_env())
        cls.hub_log = open(cls.tmp / "hub.log", "w")
        cls.hub = subprocess.Popen([str(hub_bin)], env=env, stdout=cls.hub_log, stderr=cls.hub_log)
        for _ in range(50):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{port}/config.json", timeout=1)
                break
            except OSError:
                time.sleep(0.1)
        cls.impls = find_impls(cls.tmp)
        print(f"\nimplementations: {', '.join(cls.impls)}", file=sys.stderr)

    @classmethod
    def extra_hub_env(cls) -> dict:
        return {}

    @classmethod
    def tearDownClass(cls):
        cls.hub.terminate()
        cls.hub.wait()
        cls.hub_log.close()
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def cli_env(self, home: Path) -> dict:
        return dict(os.environ, MYOUS_HOME=str(home))

    def run_cli(self, impl: str, home: Path, *args: str, check: bool = True) -> str:
        env = self.cli_env(home)
        r = subprocess.run(self.impls[impl] + list(args), env=env, capture_output=True, text=True, timeout=120)
        if check and r.returncode != 0:
            self.fail(f"{impl} {' '.join(args)} failed ({r.returncode}):\n{r.stdout}\n{r.stderr}")
        return r.stdout

    def entries(self, impl: str, home: Path) -> list[dict]:
        # `inbox` fetches first: no separate poll needed.
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
        # Accepting again resumes the pairing (as after a dropped connection)
        # instead of failing with "invite already used".
        self.run_cli(joiner, b_home, "accept", invite["code"], "--wait", "0")
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


class HubNotices(Interop):
    """What the hub announces reaches every client's agent once: a newer
    release as an "update" entry, and notices as "notice" entries, with
    off-site links dropped and notices for other versions skipped."""

    LATEST_RELEASE = "v99.0.0"

    @classmethod
    def extra_hub_env(cls) -> dict:
        notices = [
            {"id": "n-maint", "text": "Hub restart at 02:00 UTC.", "url": cls.hub_url + "/changelog.md"},
            {"id": "n-offsite", "text": "Read\u0007 this.", "url": "https://example.com/x"},
            {"id": "n-old", "text": "Only for old clients.", "max_version": "v0.0.1"},
            {"id": "n-future", "text": "Only for future clients.", "min_version": "v99.0.0"},
            {"id": "n-expired", "text": "Already over.", "expires": 1},
            {"id": "", "text": "No id."},
        ]
        path = cls.tmp / "notices.json"
        path.write_text(json.dumps(notices))
        return {"NOTICES_FILE": str(path)}

    def test_every_pair(self):
        host = urllib.parse.urlsplit(self.hub_url).netloc
        for impl in self.impls:
            with self.subTest(impl=impl):
                home = self.tmp / f"notices-{impl}"
                self.run_cli(impl, home, "init", "--alias", f"{impl}-n", "--hub", self.hub_url)
                got = self.entries(impl, home)
                updates = [e for e in got if e["type"] == "update"]
                self.assertEqual(len(updates), 1, got)
                self.assertEqual(updates[0]["version"], "v99.0.0")
                self.assertIn("myous v99.0.0 is available", updates[0]["text"])
                notices = sorted((e for e in got if e["type"] == "notice"), key=lambda e: e["id"])
                self.assertEqual([(e["id"], e["text"], e.get("url")) for e in notices], [
                    ("n-maint", f"Notice from {host}: Hub restart at 02:00 UTC. (more: {self.hub_url}/changelog.md)",
                     f"{self.hub_url}/changelog.md"),
                    ("n-offsite", f"Notice from {host}: Read this.", None),
                ])
                self.assertEqual(self.entries(impl, home), [])  # only once


class TestProxy:
    """An HTTP proxy that wants a password and is the only way to reach
    *.myous.test (it resolves those to 127.0.0.1). Handles CONNECT and
    absolute-URL requests, and records the hosts it was asked for."""

    USER, PASSWORD = "agent", "s3cret:@"

    def __init__(self):
        self.server = socket.create_server(("127.0.0.1", 0))
        self.port = self.server.getsockname()[1]
        user, password = urllib.parse.quote(self.USER), urllib.parse.quote(self.PASSWORD, safe="")
        self.url = f"http://{user}:{password}@127.0.0.1:{self.port}"
        self.auth = "Basic " + base64.b64encode(f"{self.USER}:{self.PASSWORD}".encode()).decode()
        self.seen: list[tuple[str, str]] = []  # (method, host)
        threading.Thread(target=self._serve, daemon=True).start()

    def close(self):
        self.server.close()

    def _serve(self):
        while True:
            try:
                client, _ = self.server.accept()
            except OSError:
                return
            threading.Thread(target=self._handle, args=(client,), daemon=True).start()

    def _handle(self, client: socket.socket):
        upstream = None
        try:
            head = b""
            while b"\r\n\r\n" not in head:
                chunk = client.recv(4096)
                if not chunk:
                    return
                head += chunk
            head, rest = head.split(b"\r\n\r\n", 1)
            first, *headers = head.decode().split("\r\n")
            method, target, version = first.split()
            fields = {h.split(":", 1)[0].lower(): h.split(":", 1)[1].strip() for h in headers}
            if fields.get("proxy-authorization") != self.auth:
                client.sendall(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n")
                return
            if method == "CONNECT":
                host, port = target.rsplit(":", 1)
            else:
                u = urllib.parse.urlsplit(target)
                host, port = u.hostname, u.port or 80
            self.seen.append((method, host))
            if not host.endswith(".myous.test"):
                client.sendall(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                return
            upstream = socket.create_connection(("127.0.0.1", int(port)))
            if method == "CONNECT":
                client.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
            else:
                path = urllib.parse.urlsplit(target)._replace(scheme="", netloc="").geturl() or "/"
                kept = [h for h in headers if not h.lower().startswith("proxy-")]
                upstream.sendall(("\r\n".join([f"{method} {path} {version}"] + kept) + "\r\n\r\n").encode())
            if rest:
                upstream.sendall(rest)
            socks = [client, upstream]
            while True:
                readable, _, _ = select.select(socks, [], [], 60)
                if not readable:
                    return
                for s in readable:
                    data = s.recv(65536)
                    if not data:
                        return
                    (upstream if s is client else client).sendall(data)
        except (OSError, ValueError):
            pass
        finally:
            client.close()
            if upstream:
                upstream.close()


class ProxyInterop(Interop):
    """Every client pairs and messages with itself when the hub and relay
    can only be reached through an HTTP proxy that wants a password."""

    HUB_HOST = "hub.myous.test"

    @classmethod
    def setUpClass(cls):
        cls.proxy = TestProxy()
        super().setUpClass()

    @classmethod
    def tearDownClass(cls):
        super().tearDownClass()
        cls.proxy.close()

    def cli_env(self, home: Path) -> dict:
        env = {k: v for k, v in os.environ.items() if k.lower() not in ("no_proxy", "all_proxy")}
        for name in ("HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"):
            env[name] = self.proxy.url
        return dict(env, MYOUS_HOME=str(home))

    def test_every_pair(self):
        for impl in self.impls:
            with self.subTest(impl=impl):
                self.check_pair(impl, impl)
        self.assertIn(("CONNECT", self.HUB_HOST), self.proxy.seen)


if __name__ == "__main__":
    unittest.main()

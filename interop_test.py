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
import re
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


# About 70 KB, so three parts, with characters that are awkward to split or
# escape: multi-byte UTF-8, emoji, quotes, backslashes, newlines, <>&.
LONG_TEXT = "".join(f"line {i}: naïve café 日本語 😀👍🏽 \"quoted\" back\\slash <tag> & more\n" for i in range(1000))


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
        cls.hub_env, cls.hub_port = env, port
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

    def run_cli(self, impl: str, home: Path, *args: str, check: bool = True, stdin: str | None = None) -> str:
        env = self.cli_env(home)
        r = subprocess.run(self.impls[impl] + list(args), env=env, capture_output=True, text=True, timeout=120,
                           input=stdin)
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

        invite = json.loads(self.run_cli(inviter, a_home, "invite", "--json",
                                         "--relationship", "friend", "--sharing", "calendar yes"))
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
        code_of = lambda e: re.search(r"verification code (\d{6})", e["text"]).group(1)  # noqa: E731
        self.assertEqual(code_of(a_paired[0]), code_of(b_paired[0]), "verification codes differ")

        a_contacts = json.loads(self.run_cli(inviter, a_home, "contacts", "--json"))
        b_contacts = json.loads(self.run_cli(joiner, b_home, "contacts", "--json"))
        self.assertEqual([c["alias"] for c in a_contacts.values()], [b_name])
        self.assertEqual([c["alias"] for c in b_contacts.values()], [a_name])
        # Relationship context: set at invite on one side, not yet on the other.
        a_contact, b_contact = next(iter(a_contacts.values())), next(iter(b_contacts.values()))
        self.assertEqual((a_contact.get("relationship"), a_contact.get("sharing")), ("friend", "calendar yes"))
        self.assertIsNone(b_contact.get("relationship"))
        self.assertIn("Ask your owner", b_paired[0]["text"])
        self.assertNotIn("Ask your owner", a_paired[0]["text"])

        for i in range(2):
            self.run_cli(inviter, a_home, "send", b_name, f"hello {joiner} {i}")
        got = self.entries(joiner, b_home)
        self.assertEqual([e["text"] for e in got], [f"hello {joiner} 0", f"hello {joiner} 1"])
        self.assertTrue(all(e.get("relationship") is None for e in got), got)
        set_out = json.loads(self.run_cli(joiner, b_home, "context", a_name, "--relationship", "family",
                                          "--sharing", "anything", "--json"))
        self.assertEqual((set_out["relationship"], set_out["sharing"]), ("family", "anything"))
        r = subprocess.run(self.impls[joiner] + ["context", a_name, "--relationship", "enemy"],
                           env=self.cli_env(b_home), capture_output=True, text=True, timeout=60)
        self.assertNotEqual(r.returncode, 0, "an unknown relationship was accepted")
        self.run_cli(joiner, b_home, "send", a_name, f"hi {inviter}")
        received = self.entries(inviter, a_home)
        self.assertEqual([(e["alias"], e["text"], e.get("relationship"), e.get("sharing")) for e in received],
                         [(b_name, f"hi {inviter}", "friend", "calendar yes")])

        # Cards (protocol section 4): a's description reaches b as a card
        # entry and rides along with a's later messages; a rename is
        # announced to b and never changes b's alias for a.
        set_card = json.loads(self.run_cli(inviter, a_home, "card", "my", "own", "test", "agent", "--json"))
        self.assertEqual((set_card["name"], set_card["about"], set_card["told"]), (a_name, "my own test agent", [b_name]))
        got = self.entries(joiner, b_home)
        self.assertEqual([(e["type"], e.get("name"), e.get("about")) for e in got], [("card", a_name, "my own test agent")])
        self.assertIn(f"{a_name} describes itself: my own test agent", got[0]["text"])
        b_contacts = json.loads(self.run_cli(joiner, b_home, "contacts", "--json"))
        self.assertEqual(next(iter(b_contacts.values()))["card"]["about"], "my own test agent")
        self.run_cli(inviter, a_home, "send", b_name, "with card")
        got = self.entries(joiner, b_home)
        self.assertEqual([(e["text"], e.get("about")) for e in got], [("with card", "my own test agent")])
        self.run_cli(inviter, a_home, "init", "--alias", a_name + "-renamed", "--hub", self.hub_url, "--rename")
        got = self.entries(joiner, b_home)
        self.assertEqual([e["type"] for e in got], ["card"])
        self.assertIn(f'{a_name} now calls itself "{a_name}-renamed"; you call it "{a_name}"', got[0]["text"])
        b_contacts = json.loads(self.run_cli(joiner, b_home, "contacts", "--json"))
        self.assertEqual([c["alias"] for c in b_contacts.values()], [a_name])
        self.run_cli(inviter, a_home, "init", "--alias", a_name, "--hub", self.hub_url, "--rename")   # back, for the checks below
        self.entries(joiner, b_home)
        self.assertEqual(json.loads(self.run_cli(inviter, a_home, "card", "--json")), {"name": a_name, "about": "my own test agent"})

        # A long message goes out in parts and arrives whole.
        self.run_cli(inviter, a_home, "send", b_name, "-", stdin=LONG_TEXT)
        got = self.entries(joiner, b_home)
        self.assertEqual([(e["text"], e.get("relationship")) for e in got], [(LONG_TEXT, "family")])
        # Over the limit: refused before sending, with a clear error.
        r = subprocess.run(self.impls[inviter] + ["send", b_name, "-"], env=self.cli_env(a_home),
                           capture_output=True, text=True, timeout=60, input="x" * 300_000)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("limit", r.stdout + r.stderr)


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


class StorageQuota(Interop):
    """An agent that stores too much on the relay is refused, clearly, while
    others carry on, and the hub still remembers after a restart."""

    @classmethod
    def extra_hub_env(cls) -> dict:
        return {"SENDER_QUOTA_BYTES": "100000"}

    def restart_hub(self) -> None:
        cls = type(self)
        cls.hub.terminate()
        cls.hub.wait()
        cls.hub = subprocess.Popen(cls.hub.args, env=cls.hub_env, stdout=cls.hub_log, stderr=cls.hub_log)
        for _ in range(50):
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{cls.hub_port}/config.json", timeout=1)
                return
            except OSError:
                time.sleep(0.1)

    def test_every_pair(self):
        for impl in self.impls:
            with self.subTest(impl=impl):
                a, b = self.tmp / f"quota-{impl}-a", self.tmp / f"quota-{impl}-b"
                self.run_cli(impl, a, "init", "--alias", f"{impl}-qa", "--hub", self.hub_url)
                self.run_cli("python", b, "init", "--alias", f"{impl}-qb", "--hub", self.hub_url)
                code = json.loads(self.run_cli("python", b, "invite", "--json"))["code"]
                self.run_cli(impl, a, "accept", code, "--wait", "3")
                self.entries("python", b)
                self.entries(impl, a)
                # ~79 KB of text is ~150 KB of wraps: over the 100 KB quota partway.
                r = subprocess.run(self.impls[impl] + ["send", f"{impl}-qb", "-"], env=self.cli_env(a),
                                   capture_output=True, text=True, timeout=60, input=LONG_TEXT)
                self.assertNotEqual(r.returncode, 0, r.stdout)
                self.assertIn("storage quota", r.stdout + r.stderr)
                # Only that agent is limited: its contact can still write to it.
                self.run_cli("python", b, "send", f"{impl}-qa", "still fine")
                self.assertEqual([e["text"] for e in self.entries(impl, a)], ["still fine"])
        # The hub remembers who stored what across a restart.
        self.restart_hub()
        impl = next(iter(self.impls))
        r = subprocess.run(self.impls[impl] + ["send", f"{impl}-qb", "-"], env=self.cli_env(self.tmp / f"quota-{impl}-a"),
                           capture_output=True, text=True, timeout=60, input=LONG_TEXT)
        self.assertIn("storage quota", r.stdout + r.stderr)


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

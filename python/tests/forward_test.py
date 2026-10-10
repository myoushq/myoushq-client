"""worker/forward.py: the container's 127.0.0.1:9222 relayed to the port in
browser.json (the browser on the host). Standard library only, like the
forwarder itself."""
import asyncio
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

FORWARD = os.path.join(os.path.dirname(__file__), "..", "..", "worker", "forward.py")


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


async def echo(reader, writer):
    while True:
        data = await reader.read(1024)
        if not data:
            break
        writer.write(b"echo:" + data)
        await writer.drain()
    writer.close()


def wait_listening(port, seconds=5):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            socket.create_connection(("127.0.0.1", port), 0.2).close()
            return True
        except OSError:
            time.sleep(0.05)
    return False


class Forwarder(unittest.TestCase):
    """One forwarder per test, on a free port, with a fresh worker home."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = Path(self.tmp.name)
        self.port = free_port()
        self.proc = subprocess.Popen([sys.executable, FORWARD],
                                     env={**os.environ, "MYOUS_HOME": str(self.home), "MYOUS_CDP_PORT": str(self.port)},
                                     stderr=subprocess.PIPE, text=True)
        self.assertTrue(wait_listening(self.port), "the forwarder didn't listen")
        self.assertIn("goes to the host", self.proc.stderr.readline())

    def tearDown(self):
        self.proc.terminate()
        self.proc.wait(5)
        self.tmp.cleanup()

    def test_relays_to_the_port_in_browser_json(self):
        async def run():
            server = await asyncio.start_server(echo, "127.0.0.1", 0)
            target = server.sockets[0].getsockname()[1]
            (self.home / "browser.json").write_text(json.dumps({"host": "127.0.0.1", "port": target}))
            async with server:
                r, w = await asyncio.open_connection("127.0.0.1", self.port)
                w.write(b"GET /json/version\r\n")
                await w.drain()
                got = await asyncio.wait_for(r.read(64), 5)
                w.close()
                return got

        self.assertEqual(asyncio.run(run()), b"echo:GET /json/version\r\n")

    def test_without_browser_json_the_connection_is_closed(self):
        s = socket.create_connection(("127.0.0.1", self.port), 2)
        s.settimeout(5)
        self.assertEqual(s.recv(16), b"")
        s.close()
        self.assertIn("no browser.json", self.proc.stderr.readline())

    def test_browser_json_is_read_per_connection(self):
        async def run():
            out = []
            for n in (1, 2):
                server = await asyncio.start_server(echo, "127.0.0.1", 0)
                target = server.sockets[0].getsockname()[1]
                (self.home / "browser.json").write_text(json.dumps({"host": "127.0.0.1", "port": target}))
                async with server:
                    r, w = await asyncio.open_connection("127.0.0.1", self.port)
                    w.write(b"%d" % n)
                    await w.drain()
                    out.append(await asyncio.wait_for(r.read(64), 5))
                    w.close()
            return out

        self.assertEqual(asyncio.run(run()), [b"echo:1", b"echo:2"])


if __name__ == "__main__":
    unittest.main()

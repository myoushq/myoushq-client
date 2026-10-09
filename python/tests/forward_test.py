"""worker/forward.py: the container's 127.0.0.1:9222 relayed to the port in
browser.json (the browser on the host)."""
import asyncio
import json
import os
import socket
import subprocess
import sys
import time

import pytest

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


@pytest.fixture
def forwarder(tmp_path):
    port = free_port()
    proc = subprocess.Popen([sys.executable, FORWARD], env={**os.environ, "MYOUS_HOME": str(tmp_path), "MYOUS_CDP_PORT": str(port)},
                            stderr=subprocess.PIPE, text=True)
    assert wait_listening(port)
    assert "goes to the host" in proc.stderr.readline()
    yield port, tmp_path, proc
    proc.terminate()
    proc.wait(5)


def test_relays_to_the_port_in_browser_json(forwarder):
    port, home, _ = forwarder

    async def run():
        server = await asyncio.start_server(echo, "127.0.0.1", 0)
        target = server.sockets[0].getsockname()[1]
        (home / "browser.json").write_text(json.dumps({"host": "127.0.0.1", "port": target}))
        async with server:
            r, w = await asyncio.open_connection("127.0.0.1", port)
            w.write(b"GET /json/version\r\n")
            await w.drain()
            got = await asyncio.wait_for(r.read(64), 5)
            w.close()
            return got

    assert asyncio.run(run()) == b"echo:GET /json/version\r\n"


def test_without_browser_json_the_connection_is_closed(forwarder):
    port, _, proc = forwarder
    s = socket.create_connection(("127.0.0.1", port), 2)
    s.settimeout(5)
    assert s.recv(16) == b""
    s.close()
    assert "no browser.json" in proc.stderr.readline()


def test_browser_json_is_read_per_connection(forwarder):
    port, home, _ = forwarder

    async def run():
        out = []
        for n in (1, 2):
            server = await asyncio.start_server(echo, "127.0.0.1", 0)
            target = server.sockets[0].getsockname()[1]
            (home / "browser.json").write_text(json.dumps({"host": "127.0.0.1", "port": target}))
            async with server:
                r, w = await asyncio.open_connection("127.0.0.1", port)
                w.write(b"%d" % n)
                await w.drain()
                out.append(await asyncio.wait_for(r.read(64), 5))
                w.close()
        return out

    assert asyncio.run(run()) == [b"echo:1", b"echo:2"]

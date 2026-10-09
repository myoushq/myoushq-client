#!/usr/bin/env python3
"""Forward the worker's browser port to a browser on the host.

With "Browser: on this Mac" (myous for Mac), the browser is a real Chrome
on the host with the worker's own profile; the app writes its DevTools
port into the worker folder as browser.json ({"host": ..., "port": N}).
This forwarder listens on 127.0.0.1:9222 inside the container and relays
each connection to that port, so agents' scripts connect to
http://localhost:9222 exactly as with the browser in the container (and
Chrome's DevTools server, which refuses any Host header that isn't
localhost or an address, is happy). The file is read per connection, so a
browser restarted on a new port needs nothing here.

Environment: MYOUS_HOME (where browser.json is), MYOUS_CDP_PORT (default
9222), MYOUS_BROWSER_HOST (default host.docker.internal, which Docker
Desktop routes to the host's loopback; browser.json's "host" overrides).
"""
from __future__ import annotations

import asyncio
import json
import os
import sys

HOME = os.path.expanduser(os.environ.get("MYOUS_HOME", "~/.myous"))
PORT = int(os.environ.get("MYOUS_CDP_PORT", "9222"))
HOST = os.environ.get("MYOUS_BROWSER_HOST", "host.docker.internal")


def log(msg: str) -> None:
    print(f"forward: {msg}", file=sys.stderr, flush=True)


def target() -> tuple[str, int] | None:
    try:
        with open(os.path.join(HOME, "browser.json")) as f:
            d = json.load(f)
        port = int(d.get("port") or 0)
    except (OSError, ValueError, AttributeError):
        return None
    return (d.get("host") or HOST, port) if port else None


async def pump(src: asyncio.StreamReader, dst: asyncio.StreamWriter) -> None:
    try:
        while True:
            data = await src.read(65536)
            if not data:
                break
            dst.write(data)
            await dst.drain()
    except Exception:
        pass
    finally:
        try:
            if dst.can_write_eof():
                dst.write_eof()
        except Exception:
            pass


async def handle(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    t = target()
    if not t:
        log("no browser.json in the worker folder: is the browser running on the host?")
        writer.close()
        return
    try:
        r2, w2 = await asyncio.wait_for(asyncio.open_connection(*t), 5)
    except Exception as e:
        log(f"can't reach the host's browser at {t[0]}:{t[1]} ({type(e).__name__})")
        writer.close()
        return
    await asyncio.gather(pump(reader, w2), pump(r2, writer))
    for w in (writer, w2):
        try:
            w.close()
        except Exception:
            pass


async def main() -> None:
    server = await asyncio.start_server(handle, "127.0.0.1", PORT)
    log(f"127.0.0.1:{PORT} goes to the host's browser ({HOST}; port from {os.path.join(HOME, 'browser.json')})")
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass

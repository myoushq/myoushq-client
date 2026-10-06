"""myoushq watcher for Muse: wait for new messages or pairing results, then
exit, so that Muse wakes up and handles them.

Muse wakes up when a background job it started finishes, in the chat that
started it. The watcher uses that: start it as a background job and it
stays connected to the relay until something arrives, prints it, and exits.

    ~/.myous/venv/bin/python watcher.py              # from your home chat
    ~/.myous/venv/bin/python watcher.py --takeover   # from another chat that's waiting for a reply

Rules (worked out by two Muses in October 2026):

- One watcher at a time, recorded in $MYOUS_HOME/watcher.pid. Starting a
  second one exits at once, unless --takeover, which stops the running one
  and replaces it. Use --takeover from a chat that expects a reply, so the
  reply lands there.
- One shot: after it reports something it exits and doesn't restart itself.
  Read the items with `myous inbox` (which marks them read), handle them,
  then start a new watcher. A scheduled check (check.py) restarts the home
  chat's watcher if none is running.
- Nothing is lost: the watcher never marks anything read, so whatever it
  didn't hand over is still unread for the next watcher or check.

Exit status: 0 new items printed; 3 another watcher is running; 4 replaced
by another chat's watcher (nothing to do).
"""
from __future__ import annotations

import argparse
import asyncio
import os
import signal
import sys
import time

from myous import Agent, FileStorage

EXIT_NEWS, EXIT_BUSY, EXIT_REPLACED = 0, 3, 4


def read_pid(path) -> int | None:
    try:
        return int(path.read_text().strip())
    except (OSError, ValueError):
        return None


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


def claim(pidfile, takeover: bool) -> bool:
    """Become the one watcher. False if another is running and we may not
    replace it."""
    other = read_pid(pidfile)
    if other and other != os.getpid() and alive(other):
        if not takeover:
            return False
        os.kill(other, signal.SIGTERM)
        for _ in range(50):
            if not alive(other):
                break
            time.sleep(0.1)
    tmp = pidfile.with_suffix(".tmp")
    tmp.write_text(str(os.getpid()))
    tmp.replace(pidfile)
    return True


def release(pidfile) -> None:
    if read_pid(pidfile) == os.getpid():
        pidfile.unlink(missing_ok=True)


def describe(e: dict) -> str:
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(e.get("sent_at", e["at"])))
    if e["type"] == "message":
        return f"[{when}] {e['alias']}: {e['text']}"
    return f"[{when}] ({e['type']}) {e['text']}"


async def wait_for_news(agent: Agent, retry: float) -> list[dict]:
    """Return unread items as soon as there are any. Listens live; if the
    connection fails, polls every `retry` seconds until it can listen again."""
    while True:
        try:
            await agent.poll()  # finishes pairings, fetches anything waiting
        except Exception as e:
            print(f"poll failed: {e}", file=sys.stderr, flush=True)
        unread = agent.unread(mark_read=False)
        if unread:
            return unread

        news = asyncio.Event()

        def check_unread() -> None:
            # Something else (a scheduled check) may have fetched the item.
            if agent.unread(mark_read=False):
                news.set()

        listener = asyncio.ensure_future(agent.listen(on_new=lambda entries: news.set(), on_tick=check_unread))
        waiter = asyncio.ensure_future(news.wait())
        done, _ = await asyncio.wait({listener, waiter}, return_when=asyncio.FIRST_COMPLETED)
        listener.cancel()
        waiter.cancel()
        if listener in done and not listener.cancelled() and listener.exception():
            print(f"listener stopped: {listener.exception()}; retrying in {retry:.0f}s", file=sys.stderr, flush=True)
            await asyncio.sleep(retry)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--takeover", action="store_true", help="replace a running watcher")
    parser.add_argument("--retry", type=float, default=45, help="seconds between polls while the live connection is down")
    args = parser.parse_args()

    st = FileStorage()
    agent = Agent(st)
    pidfile = st.path("watcher.pid")
    if not claim(pidfile, args.takeover):
        print(f"another myoushq watcher is running (pid {read_pid(pidfile)}); nothing to do. "
              "Use --takeover to move it to this chat.")
        sys.exit(EXIT_BUSY)

    def replaced(*_) -> None:
        print("this myoushq watcher was replaced by another chat's watcher; nothing to do here.", flush=True)
        os._exit(EXIT_REPLACED)

    signal.signal(signal.SIGTERM, replaced)
    try:
        entries = asyncio.run(wait_for_news(agent, args.retry))
    finally:
        release(pidfile)
    print(f"myoushq: {len(entries)} new item(s)")
    for e in entries:
        print(describe(e))
    print("Next: run `myous inbox` to mark them read, handle them, then start the watcher again.")
    sys.exit(EXIT_NEWS)


if __name__ == "__main__":
    main()

"""myoushq watcher for Muse: wait for new messages, pairing results or
notices, then exit, so that whoever started it wakes Muse up.

It stays connected to the relay until something arrives, prints it, and
exits 0. Two ways to run it (see docs/muse.md):

    watcher.py --for 55     # from a hook that runs every minute (recommended; hook.sh)
    watcher.py --takeover   # as a background job from a chat that's waiting for a reply

Rules (worked out by two Muses in October 2026):

- One watcher at a time, recorded in $MYOUS_HOME/watcher.pid. Starting a
  second one exits at once, unless --takeover, which stops the running one
  and replaces it. Use --takeover from a chat that expects a reply, so the
  reply lands there.
- One shot: after it reports something it exits and doesn't restart itself.
  Read the items with `myous inbox` (which marks them read) and handle them.
  The hook runs the next watcher; without a hook, start a new one.
- Nothing is lost: the watcher never marks anything read, so whatever it
  didn't hand over is still unread for the next watcher or check.

Exit status: 0 new items printed; 2 nothing arrived within --for; 3 another
watcher is running; 4 replaced by another chat's watcher; 5 stopped (e.g. by
`timeout`). Only 0 means there's something to do.
"""
from __future__ import annotations

import argparse
import asyncio
import os
import signal
import sys
import time

from myous import Agent, FileStorage

EXIT_NEWS, EXIT_QUIET, EXIT_BUSY, EXIT_REPLACED, EXIT_STOPPED = 0, 2, 3, 4, 5


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
    replacing = other and other != os.getpid() and alive(other)
    if replacing and not takeover:
        return False
    # Record ourselves first, so the one we stop sees it was replaced.
    tmp = pidfile.with_suffix(".tmp")
    tmp.write_text(str(os.getpid()))
    tmp.replace(pidfile)
    if replacing:
        os.kill(other, signal.SIGTERM)
        for _ in range(50):
            if not alive(other):
                break
            time.sleep(0.1)
    return True


def release(pidfile) -> None:
    if read_pid(pidfile) == os.getpid():
        pidfile.unlink(missing_ok=True)


def describe(e: dict) -> str:
    when = time.strftime("%Y-%m-%d %H:%M", time.localtime(e.get("sent_at", e["at"])))
    if e["type"] == "message":
        return f"[{when}] {e['alias']}: {e['text']}"
    return f"[{when}] ({e['type']}) {e['text']}"


async def wait_for_news(agent: Agent, retry: float, deadline: float | None) -> list[dict]:
    """Return unread items as soon as there are any, or [] at the deadline.
    One live connection does everything: it delivers what the relay holds,
    then new messages, and finishes pairings. If it fails, poll and retry
    every `retry` seconds."""
    loop = asyncio.get_running_loop()
    while True:
        unread = agent.unread(mark_read=False)
        if unread:
            return unread
        left = None if deadline is None else deadline - loop.time()
        if left is not None and left <= 0:
            return []

        news = asyncio.Event()

        def check_unread() -> None:
            # Something else (a scheduled check, `myous inbox`) may have fetched the item.
            if agent.unread(mark_read=False):
                news.set()

        listener = asyncio.ensure_future(agent.listen(on_new=lambda entries: news.set(), on_tick=check_unread))
        waiter = asyncio.ensure_future(news.wait())
        done, _ = await asyncio.wait({listener, waiter}, timeout=left, return_when=asyncio.FIRST_COMPLETED)
        listener.cancel()
        waiter.cancel()
        if listener in done and not listener.cancelled() and listener.exception():
            print(f"listener stopped: {listener.exception()}", file=sys.stderr, flush=True)
            try:
                await agent.poll()
            except Exception as e:
                print(f"poll failed: {e}", file=sys.stderr, flush=True)
            if not agent.unread(mark_read=False):
                wait = retry if deadline is None else max(0, min(retry, deadline - loop.time()))
                await asyncio.sleep(wait)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--takeover", action="store_true", help="replace a running watcher")
    parser.add_argument("--for", dest="seconds", type=float, metavar="SECONDS",
                        help="give up after this long (exit 2); for a hook that runs it every minute")
    parser.add_argument("--retry", type=float, default=45, help="seconds between polls while the live connection is down")
    args = parser.parse_args()

    st = FileStorage()
    agent = Agent(st)
    pidfile = st.path("watcher.pid")
    if not claim(pidfile, args.takeover):
        print(f"another myoushq watcher is running (pid {read_pid(pidfile)}); nothing to do. "
              "Use --takeover to move it to this chat.")
        sys.exit(EXIT_BUSY)

    def stopped(*_) -> None:
        if read_pid(pidfile) not in (None, os.getpid()):
            print("this myoushq watcher was replaced by another chat's watcher; nothing to do here.", flush=True)
            os._exit(EXIT_REPLACED)
        release(pidfile)
        print("this myoushq watcher was stopped; nothing to do.", flush=True)
        os._exit(EXIT_STOPPED)

    signal.signal(signal.SIGTERM, stopped)

    async def run() -> list[dict]:
        deadline = None if args.seconds is None else asyncio.get_running_loop().time() + args.seconds
        return await wait_for_news(agent, args.retry, deadline)

    try:
        entries = asyncio.run(run())
    finally:
        release(pidfile)
    if not entries:
        print("myoushq: nothing new.")
        sys.exit(EXIT_QUIET)
    print(f"myoushq: {len(entries)} new item(s)")
    for e in entries:
        print(describe(e))
    print("Next: run `myous inbox` to mark them read, then handle them.")
    sys.exit(EXIT_NEWS)


if __name__ == "__main__":
    main()

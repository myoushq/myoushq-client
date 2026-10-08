"""Scheduled myoushq check, for setups without the hook (hook.sh): run it from
a Muse scheduled task every 15 minutes or so, as the backstop for a watcher
started as a background job:

    myous check        # or: python -m myous.muse.check

It polls once (finishing pairings and fetching messages), then prints what
needs doing:

- new items, to handle like the watcher's (read them with `myous inbox`);
- "start the watcher", if none is running. Start it from the home chat as a
  background job, so the next message wakes that chat.

When the first line is "nothing to do", stay quiet: don't tell the owner.
Like the watcher, it never marks anything read.
"""
from __future__ import annotations

import asyncio
import sys

from myous import Agent, FileStorage
from myous.muse.watcher import alive, describe, read_pid


def run(agent: Agent, st: FileStorage) -> int:
    """Poll once and print what needs doing; always exits 0."""
    todo = []
    try:
        asyncio.run(agent.poll())
    except Exception as e:
        todo.append(f"poll failed ({e}); try again on the next check, and tell your owner if it keeps failing")
    unread = agent.unread(mark_read=False)
    if unread:
        todo.append(f"{len(unread)} new item(s); run `myous inbox` to mark them read, then handle them:")
        todo.extend("  " + describe(e) for e in unread)
    pid = read_pid(st.path("watcher.pid"))
    if not (pid and alive(pid)):
        todo.append("no watcher is running; start `myous watcher` as a background job from the home chat")
    print("\n".join(todo) if todo else "nothing to do")
    return 0


def main() -> None:
    st = FileStorage()
    sys.exit(run(Agent(st), st))


if __name__ == "__main__":
    main()

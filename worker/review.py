#!/usr/bin/env python3
"""The worker's review hook: the one place that decides what runs.

The worker calls it before every request, with one JSON object on stdin
(protocol.md 7.2):

    {"op": "exec" | "get" | "put", "id": ..., "sender": <npub>, "alias": ...,
     "cmd": ...}            for exec
     "path": ..., "size": ...}   for get / put

Exit 0 to allow. Any other status refuses, and the first line of stderr
goes back to the requester as the reason.

Policy here (version 1): allow everything from approved contacts and
refuse while the pause file exists. Only approved contacts' messages
reach the worker at all, so "everything" means your own paired agents.
The worker itself logs every request and decision to worker.log, so this
hook doesn't need to. Tighten it by editing this file; some ideas are
marked below.
"""
from __future__ import annotations

import json
import os
import sys

HOME = os.path.expanduser(os.environ.get("MYOUS_HOME", "~/.myous"))
PAUSE_FILE = os.environ.get("MYOUS_PAUSE_FILE", os.path.join(HOME, "worker.paused"))


def main() -> int:
    try:
        req = json.load(sys.stdin)
    except ValueError:
        print("review: bad request", file=sys.stderr)
        return 2

    if os.path.exists(PAUSE_FILE):
        print("the worker is paused by its owner", file=sys.stderr)
        return 1

    # Ideas for tightening, in the order most owners want them:
    # - a denylist: refuse commands matching patterns (rm -rf /, curl | sh, ...)
    #   or paths outside the work directory;
    # - per-sender rules: some contacts may only `get`, never `exec`;
    # - ask the owner: write the request to a file the Dock app shows, or
    #   send a macOS notification, and wait for an answer file (with a
    #   timeout that refuses);
    # - rate limits: at most N exec requests per minute.

    return 0


if __name__ == "__main__":
    sys.exit(main())

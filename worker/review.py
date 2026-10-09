#!/usr/bin/env python3
"""The worker's review hook: the one place that decides what runs.

The worker calls it before every request, with one JSON object on stdin
(protocol.md 7.2):

    {"op": "exec" | "get" | "put", "id": ..., "sender": <npub>, "alias": ...,
     "cmd": ...}            for exec
     "path": ..., "size": ...}   for get / put

Exit 0 to allow. Any other status refuses, and the first line of stderr
goes back to the requester as the reason.

Policy: only approved contacts' messages reach the worker at all, so
"everything" below means the owner's own paired agents. The pause file
refuses everything. Then the review level in <home>/review.json (the
desktop app's setting) decides:

- "trust" (default): everything runs.
- "changes": reads run (get, and exec of a read-only command, see
  READ_ONLY); anything else asks the owner.
- "all": every request asks the owner.

Asking: the hook writes <home>/approvals/<id>.json (the request) and waits
up to MYOUS_APPROVAL_WAIT seconds (default 120, inside an agent's default
wait for a reply) for <home>/approvals/<id>.answer containing "allow" or
"refuse"; the desktop app shows the question and writes the answer. No
answer in time is a refusal that says so.

The worker itself logs every request and decision to worker.log.
"""
from __future__ import annotations

import json
import os
import shlex
import sys
import time

HOME = os.path.expanduser(os.environ.get("MYOUS_HOME", "~/.myous"))
PAUSE_FILE = os.environ.get("MYOUS_PAUSE_FILE", os.path.join(HOME, "worker.paused"))
REVIEW_FILE = os.path.join(HOME, "review.json")
APPROVALS = os.path.join(HOME, "approvals")
WAIT = float(os.environ.get("MYOUS_APPROVAL_WAIT", "120"))

# Commands that only read, for the "changes" level. A command line is
# read-only when every segment (split on |, &&, ||, ;) starts with one of
# these and nothing redirects output or substitutes commands.
READ_ONLY = {
    "ls", "cat", "head", "tail", "less", "more", "grep", "rg", "find", "wc", "stat", "file", "pwd",
    "du", "df", "uname", "date", "whoami", "id", "echo", "printf", "sort", "uniq", "cut", "tr",
    "awk", "diff", "tree", "which", "type", "true", "test", "basename", "dirname", "realpath",
    "readlink", "md5sum", "sha256sum", "shasum", "hostname", "nproc", "free", "uptime",
}
UNSAFE_TOKENS = (">", "<", "$(", "`", "sed -i", "tee ")


def level() -> str:
    try:
        with open(REVIEW_FILE) as f:
            lv = json.load(f).get("level")
        return lv if lv in ("trust", "changes", "all") else "trust"
    except (OSError, ValueError, AttributeError):
        return "trust"


def read_only(cmd: str) -> bool:
    if any(tok in cmd for tok in UNSAFE_TOKENS):
        return False
    for sep in ("||", "&&", "|", ";"):
        cmd = cmd.replace(sep, "\n")
    for segment in cmd.splitlines():
        try:
            words = shlex.split(segment)
        except ValueError:
            return False
        if not words:
            continue
        if words[0] not in READ_ONLY:
            return False
    return True


def ask(req: dict) -> tuple[bool, str]:
    """Write the question, wait for the owner's answer file."""
    os.makedirs(APPROVALS, exist_ok=True)
    rid = "".join(c for c in str(req.get("id", "")) if c.isalnum() or c in "-_")[:64] or str(int(time.time()))
    question = os.path.join(APPROVALS, rid + ".json")
    answer = os.path.join(APPROVALS, rid + ".answer")
    with open(question + ".tmp", "w") as f:
        json.dump(dict(req, asked_at=int(time.time()), wait=WAIT), f)
    os.replace(question + ".tmp", question)
    deadline = time.time() + WAIT
    try:
        while time.time() < deadline:
            try:
                with open(answer) as f:
                    verdict = f.read().strip().lower()
                os.unlink(answer)
                return verdict == "allow", "refused by the owner"
            except FileNotFoundError:
                time.sleep(0.5)
        return False, "the owner didn't answer in time"
    finally:
        try:
            os.unlink(question)
        except OSError:
            pass


def decide(req: dict, lv: str) -> tuple[bool, str]:
    if os.path.exists(PAUSE_FILE):
        return False, "the worker is paused by its owner"
    if lv == "trust":
        return True, ""
    if lv == "changes" and (req.get("op") == "get" or (req.get("op") == "exec" and read_only(str(req.get("cmd", ""))))):
        return True, ""
    return ask(req)


def main() -> int:
    try:
        req = json.load(sys.stdin)
    except ValueError:
        print("review: bad request", file=sys.stderr)
        return 2
    ok, why = decide(req, level())
    if not ok:
        print(why, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

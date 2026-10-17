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

- "trust": everything runs.
- "changes": reads run (get, and exec of a read-only command, see
  READ_ONLY); anything else asks the owner.
- "all": every request asks the owner.

Without a review.json the level is $MYOUS_DEFAULT_REVIEW (the container
sets "changes"), or "trust" when that is unset. A review.json, or a
MYOUS_DEFAULT_REVIEW, that can't be read as a level asks about everything
("all") until it is fixed.

Asking: the hook writes <home>/approvals/<id>.json (the request) and waits
up to MYOUS_APPROVAL_WAIT seconds (default 120, inside an agent's default
wait for a reply) for <home>/approvals/<id>.answer containing "allow" or
"refuse"; the desktop app shows the question and writes the answer. No
answer in time is a refusal that says so. The id names those files, so it
must be a plain name: 1 to 64 characters from A-Z, a-z, 0-9, - and _ (the
rule the workers and the apps hold ids to); a request with any other id
is refused rather than asked about.

The worker itself logs every request and decision to worker.log.
"""
from __future__ import annotations

import json
import os
import re
import shlex
import sys
import time

HOME = os.path.expanduser(os.environ.get("MYOUS_HOME", "~/.myous"))
PAUSE_FILE = os.environ.get("MYOUS_PAUSE_FILE", os.path.join(HOME, "worker.paused"))
REVIEW_FILE = os.path.join(HOME, "review.json")
APPROVALS = os.path.join(HOME, "approvals")
WAIT = float(os.environ.get("MYOUS_APPROVAL_WAIT", "120"))
ID_RULE = re.compile(r"[A-Za-z0-9_-]{1,64}")
BAD_ID = "bad request: the id must be 1 to 64 characters from A-Z, a-z, 0-9, - and _"

# Commands that only read, for the "changes" level. A command line is
# read-only when every segment (split on |, &&, ||, ;, &) starts with one
# of these, nothing redirects output, substitutes commands or runs code
# inside one (find -exec, awk '{...}'), and no argument points outside the
# worker's work directory (W3: see _escapes_work; the work directory is the
# command's cwd, so a plain relative path stays inside it). A single &
# backgrounds a command and starts another, so it is a separator too:
# without it, `echo & rm x` was judged by `echo` alone and ran `rm x`
# unreviewed.
READ_ONLY = {
    "ls", "cat", "head", "tail", "less", "more", "grep", "rg", "find", "wc", "stat", "file", "pwd",
    "du", "df", "uname", "date", "whoami", "id", "echo", "printf", "sort", "uniq", "cut", "tr",
    "awk", "diff", "tree", "which", "type", "true", "test", "basename", "dirname", "realpath",
    "readlink", "md5sum", "sha256sum", "shasum", "hostname", "nproc", "free", "uptime",
}
UNSAFE_TOKENS = (">", "<", "$(", "`", "sed -i", "tee ", "{", "-exec", "-ok", "-delete")
# A few allowlisted commands still write with the right flag: `sort -o/
# --output FILE` writes (and overwrites) a file, and GNU find's -fprint,
# -fprintf, -fprint0 and -fls write one. These are command-specific (a
# bare "-o" is harmless for grep), so they can't be global UNSAFE_TOKENS;
# _writes_a_file checks them against the command's own name.
FIND_WRITE_PRIMARIES = ("-fprint", "-fprintf", "-fprint0", "-fls")
# On Windows the worker's shell is PowerShell: its read-only commands, and
# what runs code inside an otherwise harmless one ((...), & and script blocks).
WINDOWS = os.name == "nt"
READ_ONLY_POWERSHELL = {
    "get-childitem", "gci", "dir", "get-content", "gc", "type", "get-item", "gi", "get-itemproperty",
    "get-location", "gl", "select-string", "sls", "measure-object", "measure", "get-date", "get-process",
    "gps", "get-command", "gcm", "test-path", "resolve-path", "get-filehash", "get-psdrive", "get-volume",
    "get-service", "write-output", "write-host", "format-table", "ft", "format-list", "fl", "out-string",
    "select-object", "select", "sort-object", "where.exe", "findstr", "ipconfig", "systeminfo",
}
UNSAFE_POWERSHELL = ("(", "&", "@", "--%", "Invoke-", "iex")


LEVELS = ("trust", "changes", "all")
# A review.json that is present but can't be read as a valid level fails
# closed to "all" (ask about everything), not open to "trust": a worker
# whose setting file was truncated or corrupted (W2 used a truncated
# review.json to drop the worker to "trust") asks rather than running
# everything unreviewed. An absent file is a worker that was never
# configured, and keeps the default (default_level).
FAIL_CLOSED = "all"
LOG_FILE = os.path.join(HOME, "worker.log")


def note(record: dict) -> None:
    """One line in worker.log, as the worker writes its own (the owner's
    app shows the log): for what the review can't say in a reason."""
    try:
        with open(LOG_FILE, "a") as f:
            f.write(json.dumps(dict(record, at=int(time.time())), sort_keys=True) + "\n")
    except OSError:
        pass


def default_level() -> str:
    """The level of a worker that has never been configured (no
    review.json): $MYOUS_DEFAULT_REVIEW, or "trust" when that is unset or
    empty. The container's entrypoint sets it to "changes", so a fresh
    container asks before anything that changes things, as direct mode (the
    desktop app) already does; a level picked in the app writes
    review.json, which then wins. A value that isn't a level fails closed
    like a corrupt review.json (an unreadable safety setting asks, it
    doesn't run everything), and says so in worker.log."""
    lv = os.environ.get("MYOUS_DEFAULT_REVIEW", "")
    if not lv:
        return "trust"
    if lv in LEVELS:
        return lv
    note({"op": "review", "value": lv,
          "error": "MYOUS_DEFAULT_REVIEW isn't trust, changes or all, so every request is asked about (all)"})
    return FAIL_CLOSED


def level() -> str:
    """review.json's level. Absent: the default (default_level). Present
    but unreadable, not JSON, not an object or not a level: FAIL_CLOSED."""
    try:
        with open(REVIEW_FILE) as f:
            data = json.load(f)
    except FileNotFoundError:
        return default_level()  # never configured
    except (OSError, ValueError):
        return FAIL_CLOSED  # present but unreadable or not JSON
    lv = data.get("level") if isinstance(data, dict) else None
    return lv if lv in LEVELS else FAIL_CLOSED


def _writes_a_file(words: list[str]) -> bool:
    """Whether an otherwise read-only command writes a file through one of
    its own flags: `sort -o/--output FILE` (also `-oFILE`), and GNU find's
    -fprint/-fprintf/-fprint0/-fls."""
    name, rest = words[0], words[1:]
    if name == "sort":
        for a in rest:
            if a in ("-o", "--output") or a.startswith("--output=") or (a.startswith("-o") and not a.startswith("--") and len(a) > 2):
                return True
    elif name == "find":
        if any(a in FIND_WRITE_PRIMARIES for a in rest):
            return True
    return False


def _escapes_work(token: str, windows: bool) -> bool:
    """Whether a command argument could point outside the work directory
    (W3). The work directory is the command's cwd, so a plain relative path
    stays inside it; an absolute path, a ~ the shell expands to a home
    directory, or a `..` component can leave it. A flag (starting with -)
    is not a path. Conservative: a token that merely looks like a path
    (a /regex/, say) is treated as one, so the owner is asked rather than
    the command run unseen -- more prompts, never fewer."""
    if not token or token.startswith("-"):
        return False
    t = token
    if windows and len(t) >= 2 and t[0] == t[-1] and t[0] in "'\"":
        t = t[1:-1]   # non-posix shlex keeps the quotes around a word
    if not t:
        return False
    if t[0] == "~" or t[0] == "/":
        return True
    if windows:
        if t[0] == "\\":
            return True
        if len(t) >= 2 and t[1] == ":" and t[0].isalpha():  # a drive: C:\ or C:/
            return True
    return ".." in re.split(r"[\\/]" if windows else "/", t)


def read_only(cmd: str, windows: bool = WINDOWS) -> bool:
    if any(tok in cmd for tok in UNSAFE_TOKENS):
        return False
    if windows and any(tok.lower() in cmd.lower() for tok in UNSAFE_POWERSHELL):
        return False
    # && before &: once && is gone, the remaining & are single backgrounders.
    for sep in ("||", "&&", "|", ";", "&"):
        cmd = cmd.replace(sep, "\n")
    for segment in cmd.splitlines():
        try:
            words = shlex.split(segment, posix=not windows)
        except ValueError:
            return False
        if not words:
            continue
        if windows:
            if words[0].lower() not in READ_ONLY | READ_ONLY_POWERSHELL:
                return False
        elif words[0] not in READ_ONLY:
            return False
        elif _writes_a_file(words):
            return False
        if any(_escapes_work(w, windows) for w in words[1:]):
            return False
    return True


def ask(req: dict) -> tuple[bool, str]:
    """Write the question, wait for the owner's answer file."""
    rid = req.get("id")
    if not isinstance(rid, str) or not ID_RULE.fullmatch(rid):
        return False, BAD_ID   # the owner's app could not answer it
    os.makedirs(APPROVALS, exist_ok=True)
    question = os.path.join(APPROVALS, rid + ".json")
    answer = os.path.join(APPROVALS, rid + ".answer")
    # Only an answer written for *this* question counts. A leftover
    # <id>.answer (the owner clicked after the wait ran out, so the earlier
    # question was already gone; or a stop killed the hook before it read
    # the answer) must not approve a later request that reuses the id: the
    # agent chooses ids, so it could reuse one the owner once approved.
    try:
        os.unlink(answer)
    except OSError:
        pass
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

"""A worker: an agent that executes requests from its approved contacts
(protocol.md, section 7). Typically a container on its owner's machine,
paired with the owner's other agents, which push files, pull files and run
commands on it with `myous cp` and `myous exec`.

The worker does what it's asked, one request at a time, after its review
hook allows it. The default hook allows everything from approved contacts,
refuses while the pause file exists, and logs every request; tightening
is a matter of giving `myous worker --review` a command of your own.
"""
from __future__ import annotations

import asyncio
import importlib.util
import json
import math
import os
import platform
import shlex
import signal
import subprocess
import sys
import time
from pathlib import Path

from myous import files, inbox, relay
from myous.agent import Agent
from myous.storage import Storage

ABOUT = "myoushq worker"
MAX_TIMEOUT = 600
DEFAULT_TIMEOUT = 120
STDOUT_LIMIT = 150_000  # bytes of each stream kept in a result, so the message fits
STDERR_LIMIT = 40_000
HELP_INTERVAL = 60  # seconds between help replies to the same contact
REVIEW_TIMEOUT = 60
STATUS_EVERY = 5


class Worker:
    def __init__(self, agent: Agent, st: Storage, work_dir: str | os.PathLike, review_cmd: str | None = None,
                 pause_file: str | os.PathLike | None = None, allow_absolute: bool = False,
                 notes: list[str] | None = None, out=sys.stdout):
        self.agent, self.st = agent, st
        self.work = Path(work_dir).expanduser().resolve()
        self.work.mkdir(parents=True, exist_ok=True)
        home = getattr(st, "home", None)
        self.home = Path(home) if home else None
        self.review_cmd = self._check_review_cmd(review_cmd)
        self.pause_file = Path(pause_file) if pause_file else (self.home / "worker.paused" if self.home else None)
        self.log_file = Path(home) / "worker.log" if home else None
        self.allow_absolute = allow_absolute
        self.notes = notes or []
        self.out = out
        self.started = int(time.time())
        self.requests = 0
        self.last: dict | None = None
        self.invite: dict | None = None
        self.help_sent: dict[str, float] = {}
        self._status_written = 0.0
        self._busy: asyncio.Lock | None = None  # one request at a time, whichever path delivered it

    def _check_review_cmd(self, cmd: str | None) -> str | None:
        """The review command with its relative paths made absolute now,
        while the current directory is still the owner's. It is the one
        control point, so it must not be something a request could
        replace: nothing in it may live under the work directory."""
        if not cmd:
            return None
        words = shlex.split(cmd)
        for i, word in enumerate(words):
            p = Path(word).expanduser()
            if ("/" in word or p.exists()) and p.exists():
                p = p.resolve()
                words[i] = str(p)
                if p == self.work or self.work in p.parents:
                    raise ValueError(f"the review hook ({word}) is inside the work directory {self.work}, "
                                     "where requests can replace it; keep it elsewhere")
        return shlex.join(words)

    # --- running -------------------------------------------------------------

    async def run(self) -> None:
        """Serve requests until stopped. Reconnects if the relay drops."""
        self.ensure_invite()
        self.write_status()
        while True:
            try:
                await self.agent.listen(on_new=self.on_new, on_tick=self.tick, tick=10)
            except (OSError, relay.RelayError) as e:
                self.say(f"connection lost ({e}); retrying in 10 seconds")
                await asyncio.sleep(10)

    def tick(self) -> None:
        self.ensure_invite()
        if time.time() - self._status_written > STATUS_EVERY:
            self.write_status()

    def ensure_invite(self) -> None:
        """Until the worker has a contact, keep a fresh invite on offer so
        the owner can pair their agent with it."""
        if self.agent.contacts():
            self.invite = None
            return
        if self.invite and self.invite["expires_at"] > time.time() + 30:
            return
        self.invite = self.agent.invite()
        self.say(f"pair your agent with this worker: code {self.invite['code']} or link {self.invite['link']} "
                 f"(valid 15 minutes; a new one is issued when it expires)")
        self.write_status()

    async def on_new(self, entries: list[dict]) -> None:
        if self._busy is None:
            self._busy = asyncio.Lock()
        async with self._busy:
            for e in entries:
                if e.get("direction") == "out":
                    continue
                if e["type"] == "paired":
                    self.say(f"paired with {e.get('alias')}")
                    self.ensure_invite()
                elif e["type"] in ("message", "file"):
                    await self.handle(e)
                self.write_status()

    async def handle(self, e: dict) -> None:
        """One request, in arrival order. Whatever is wrong with a request
        gets a refusal, never a dead worker."""
        rid = None
        try:
            self.work.mkdir(parents=True, exist_ok=True)  # a command may have removed it
            if e["type"] == "file":
                w = e.get("w") or []
                if len(w) >= 3 and w[0] == "put":
                    rid = w[1]
                    await self.do_put(e, w[1], w[2])
                return
            req = e.get("request")
            if req:
                rid = req["id"]
            if req and req["myous"] == "exec":
                await self.do_exec(e, req)
            elif req and req["myous"] == "get":
                await self.do_get(e, req)
            elif inbox.parse_request(e["text"]) is None:
                await self.send_help(e)
        except (OSError, relay.RelayError, files.BlobError) as err:
            # Our side (relay, hub, disk): log it; the requester's command times out.
            self.log({"op": "error", "id": rid, "sender": e.get("peer"), "error": str(err)})
            self.say(f"request {rid or '?'} failed: {err}")
        except Exception as err:  # noqa: BLE001 - a malformed request must not kill the worker
            self.log({"op": "error", "id": rid, "sender": e.get("peer"), "error": repr(err)})
            if rid:
                try:
                    await self.refuse(e, rid, f"bad request: {err}")
                except Exception:  # noqa: BLE001
                    self.say(f"could not answer request {rid}: {err}")

    # --- the three operations ----------------------------------------------------

    async def do_exec(self, e: dict, req: dict) -> None:
        cmd = req.get("cmd")
        if not isinstance(cmd, str) or not cmd.strip():
            await self.refuse(e, req["id"], "exec needs a cmd string")
            return
        timeout = req.get("timeout", DEFAULT_TIMEOUT)
        if not isinstance(timeout, (int, float)) or isinstance(timeout, bool) or not math.isfinite(timeout):
            timeout = DEFAULT_TIMEOUT
        timeout = min(max(int(timeout), 1), MAX_TIMEOUT)
        why = self.review("exec", e, req["id"], cmd=cmd)
        if why:
            await self.refuse(e, req["id"], why)
            return
        result = await asyncio.get_running_loop().run_in_executor(None, run_command, cmd, self.work, timeout)
        self.note(e, "exec", True)
        await self.reply(e, dict(result, myous="result", id=req["id"]))

    async def do_get(self, e: dict, req: dict) -> None:
        path = req.get("path")
        target, why = self.resolve(path)
        why = why or self.review("get", e, req["id"], path=path)
        if not why and not target.is_file():
            why = "no such file on the worker"
        if why:
            await self.refuse(e, req["id"], why)
            return
        try:
            await self.agent.send_file(e["peer"], target, extra_tags=[["w", "file", req["id"]]])
        except (OSError, ValueError, files.BlobError) as err:
            await self.refuse(e, req["id"], f"could not send the file: {err}")
            return
        self.note(e, "get", True)

    async def do_put(self, e: dict, rid: str, path: str) -> None:
        target, why = self.resolve(path)
        why = why or self.review("put", e, rid, path=path, size=e.get("size"))
        if why:
            await self.refuse(e, rid, why)
            return
        try:
            data = await asyncio.get_running_loop().run_in_executor(None, self.agent.fetch_bytes, e)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        except (OSError, ValueError, files.BlobError) as err:
            await self.refuse(e, rid, f"could not write the file: {err}")
            return
        self.note(e, "put", True)
        await self.reply(e, {"myous": "ack", "id": rid, "ok": True, "path": str(target), "size": len(data),
                             "sha256": files.sha256(data)})

    # --- review, paths, replies -----------------------------------------------------

    def review(self, op: str, e: dict, rid: str, **fields) -> str | None:
        """Why the request is refused, or None to go ahead (protocol.md 7.2)."""
        request = {"op": op, "id": rid, "sender": e.get("peer"), "alias": e.get("alias"), **fields}
        if self.review_cmd:
            try:
                # Not in the work directory: the hook must not pick up anything a request wrote there.
                r = subprocess.run(self.review_cmd, shell=True, input=json.dumps(request), capture_output=True,
                                   text=True, timeout=REVIEW_TIMEOUT, cwd=self.home)
            except subprocess.TimeoutExpired:
                why = "the review hook didn't answer"
            except OSError as err:
                why = f"the review hook could not run ({err})"
            else:
                why = None if r.returncode == 0 else ((r.stderr.strip().splitlines() or ["refused by the review hook"])[0])
        else:
            why = "worker is paused" if self.paused() else None
        self.log(dict(request, decision="allow" if why is None else f"refuse: {why}"))
        return why

    def paused(self) -> bool:
        return bool(self.pause_file and self.pause_file.exists())

    def resolve(self, path) -> tuple[Path | None, str | None]:
        """Where a request's path points, if it stays within the work
        directory (or is absolute and absolute paths are allowed)."""
        if not isinstance(path, str) or not path.strip() or "\0" in path:
            return None, "a path is needed (a plain path, no NUL bytes)"
        p = Path(path)
        if p.is_absolute():
            if not self.allow_absolute:
                return None, "absolute paths are not allowed on this worker; use a path relative to its work directory"
            return p, None
        try:
            target = (self.work / p).resolve()
        except (OSError, RuntimeError, ValueError) as err:
            return None, f"the path can't be resolved ({err})"
        if target != self.work and self.work not in target.parents:
            return None, "the path leaves the worker's work directory"
        return target, None

    async def refuse(self, e: dict, rid: str, why: str) -> None:
        self.note(e, "refuse", False)
        await self.reply(e, {"myous": "ack", "id": rid, "ok": False, "error": why})

    async def reply(self, e: dict, obj: dict) -> None:
        await self.agent.send(e["peer"], json.dumps(obj))

    async def send_help(self, e: dict) -> None:
        last = self.help_sent.get(e["peer"], 0)
        if time.time() - last < HELP_INTERVAL:
            return
        self.help_sent[e["peer"]] = time.time()
        await self.agent.send(e["peer"], self.help_text())

    def help_text(self) -> str:
        has_playwright = importlib.util.find_spec("playwright") is not None
        lines = [
            f"{self.agent.alias} is a myoushq worker: it runs commands and moves files for its owner's agents.",
            "Use `myous exec <me> -- CMD`, `myous cp FILE <me>:PATH` and `myous cp <me>:PATH FILE` "
            "(protocol.md, section 7). Wait for each reply before the next request.",
            f"Shell: /bin/sh on {platform.system()} {platform.machine()}; Python {platform.python_version()}"
            + ("; Playwright installed" if has_playwright else ""),
            f"Work directory: {self.work} (paths are relative to it"
            + ("; absolute paths allowed)" if self.allow_absolute else "; absolute paths refused)"),
            f"Limits: commands time out after {DEFAULT_TIMEOUT} s by default ({MAX_TIMEOUT} s max); "
            f"output is cut to {STDOUT_LIMIT // 1000} KB; files up to {files.MAX_BLOB >> 20} MB.",
        ]
        lines += self.notes
        return "\n".join(lines)

    # --- status, logs ----------------------------------------------------------------

    def note(self, e: dict, op: str, ok: bool) -> None:
        self.requests += 1
        self.last = {"op": op, "at": int(time.time()), "alias": e.get("alias"), "ok": ok}

    def write_status(self) -> None:
        try:
            npub = self.agent.keys.public_key().to_bech32()
        except Exception:
            npub = None
        self.st.put("worker", {
            "pid": os.getpid(), "started": self.started, "alias": self.agent.alias, "npub": npub,
            "contacts": len(self.agent.contacts()),
            "invite": ({"code": self.invite["code"], "link": self.invite["link"], "expires_at": self.invite["expires_at"]}
                       if self.invite else None),
            "paused": self.paused(), "requests": self.requests, "last": self.last, "work": str(self.work),
            "updated": int(time.time()),
        })
        self._status_written = time.time()

    def log(self, record: dict) -> None:
        if not self.log_file:
            return
        with open(self.log_file, "a") as f:
            f.write(json.dumps(dict(record, at=int(time.time())), sort_keys=True) + "\n")

    def say(self, text: str) -> None:
        print(text, file=self.out, flush=True)


def run_command(cmd: str, cwd: Path, timeout: int) -> dict:
    """Run a shell command in its own process group, so a timeout kills
    whatever it started too. Output is cut to fit a message."""
    proc = subprocess.Popen(cmd, shell=True, cwd=cwd, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    try:
        out, err = proc.communicate(timeout=timeout)
        exit_code = proc.returncode
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            out, err = proc.communicate(timeout=5)
            note = f"[myous: killed after {timeout} seconds]"
        except subprocess.TimeoutExpired as still_open:
            # Something the command started left the process group (setsid,
            # a daemon) and still holds our pipes: stop reading rather than
            # wait for it. The shell itself is dead, so wait() returns.
            out, err = still_open.stdout or b"", still_open.stderr or b""
            proc.stdout.close()
            proc.stderr.close()
            proc.wait()
            note = f"[myous: killed after {timeout} seconds; a process it started kept running and its output was cut]"
        err += b"\n" + note.encode()
        exit_code = -1
    stdout, cut_out = _cut(out, STDOUT_LIMIT)
    stderr, cut_err = _cut(err, STDERR_LIMIT)
    return {"exit": exit_code, "stdout": stdout, "stderr": stderr, "truncated": cut_out or cut_err}


def _cut(data: bytes, limit: int) -> tuple[str, bool]:
    if len(data) <= limit:
        return data.decode("utf-8", "replace"), False
    return data[:limit].decode("utf-8", "replace") + f"\n[myous: output cut at {limit} bytes]", True

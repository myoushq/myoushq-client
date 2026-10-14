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
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

from myous import files, inbox, processes, relay
from myous.agent import Agent
from myous.storage import Storage

ABOUT = "myoushq worker"
MAX_TIMEOUT = 600
DEFAULT_TIMEOUT = 120
STDOUT_LIMIT = 150_000  # bytes of each stream kept in a result, so the message fits
STDERR_LIMIT = 40_000
HELP_INTERVAL = 60  # seconds between help replies to the same contact
REVIEW_TIMEOUT = 150  # the hook may wait for the owner (worker/review.py: 120 s), inside an agent's default wait
STATUS_EVERY = 5
TICK = 5  # seconds between ticks: status writes and the owner's commands
REQUESTS_KEEP = 200  # request records kept in <home>/requests for the owner's app
# On Windows the shell is PowerShell: agents used to the container's Linux
# workers are told so in the help, the pairing message and a failed command.
WINDOWS_HELP = ("This worker runs on Windows: commands are PowerShell, not Linux shell commands "
                "(use `Get-ChildItem` or `dir`, not `ls`; `Select-String`, not `grep`).")
WINDOWS_HINT = "This worker runs PowerShell on Windows; send `help` for what it takes."
NOT_RECOGNIZED = "is not recognized as the name of a cmdlet"  # PowerShell's CommandNotFoundException
# Request ids name files the owner's app reads and writes (requests/<id>.json,
# approvals/<id>.json and .answer, commands/stop-<id>), so they must be plain
# names: 1 to 64 ASCII letters, digits, - and _ (clients send 32 hex). The
# apps and review.py hold ids to the same rule.
ID_RULE = re.compile(r"[A-Za-z0-9_-]{1,64}")
BAD_ID = "bad request: the id must be 1 to 64 characters from A-Z, a-z, 0-9, - and _"
# Stopping the worker (SIGTERM, SIGINT; CTRL_BREAK on Windows) ends the
# command it is running, answers that request, then exits.
STOPPING = "stopped by the owner (the worker is shutting down)"
SHUTDOWN_WAIT = 10  # seconds the request being handled has to be answered before the worker exits anyway


def valid_id(rid) -> bool:
    """Whether a request id is a plain name (ID_RULE)."""
    return isinstance(rid, str) and ID_RULE.fullmatch(rid) is not None


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
        # The owner's side (the desktop app): commands it drops as files, and
        # one record per request it can show, with the output.
        self.commands_dir = self.home / "commands" if self.home else None
        self.requests_dir = self.home / "requests" if self.home else None
        self.allow_absolute = allow_absolute
        self.notes = notes or []
        self.out = out
        self.started = int(time.time())
        self.requests = 0
        self.last: dict | None = None
        self.paired: dict | None = None  # the most recent pairing, with its verification code
        self.invite: dict | None = None
        self.help_sent: dict[str, float] = {}
        self._status_written = 0.0
        self._busy: asyncio.Lock | None = None  # one request at a time, whichever path delivered it
        self.stopping = threading.Event()  # set by SIGTERM / SIGINT: finish the current request, then exit

    def _check_review_cmd(self, cmd: str | None) -> str | None:
        """The review command with its relative paths made absolute now,
        while the current directory is still the owner's. It is the one
        control point, so it must not be something a request could
        replace: nothing in it may live under the work directory."""
        if not cmd:
            return None
        # Windows paths have backslashes, which POSIX quoting would eat; there
        # the words keep their quotes and the command runs under cmd.exe.
        words = shlex.split(cmd, posix=not processes.WINDOWS)
        for i, word in enumerate(words):
            bare = word.strip('"') if processes.WINDOWS else word
            p = Path(bare).expanduser()
            if p.exists():
                p = p.resolve()
                words[i] = f'"{p}"' if processes.WINDOWS else str(p)
                if p == self.work or self.work in p.parents:
                    raise ValueError(f"the review hook ({bare}) is inside the work directory {self.work}, "
                                     "where requests can replace it; keep it elsewhere")
        return " ".join(words) if processes.WINDOWS else shlex.join(words)

    # --- running -------------------------------------------------------------

    async def run(self) -> None:
        """Serve requests until stopped. Reconnects if the relay drops. A
        SIGTERM or SIGINT (CTRL_BREAK on Windows) stops it, and so does
        <home>/commands/quit from the owner's app: the command running is
        ended with its process group, the agent that asked gets its answer
        ("stopped by the owner"), and run returns."""
        self.ensure_invite()
        self.write_status()
        main = asyncio.current_task()
        loop = asyncio.get_running_loop()

        def on_signal(signum, frame, why=None):
            if self.stopping.is_set():
                return
            self.stopping.set()   # run_command and review see it within a quarter second
            loop.call_soon_threadsafe(lambda: loop.create_task(self._shut_down(main, why or f"signal {signum}")))

        quit_file = self.commands_dir / "quit" if self.commands_dir else None
        watcher = None
        if quit_file:
            try:
                if quit_file.stat().st_mtime < self.started:
                    quit_file.unlink()   # from before this worker started: not for it
            except OSError:
                pass

            async def watch_quit():
                while not self.stopping.is_set():
                    await asyncio.sleep(0.25)
                    if quit_file.exists():
                        quit_file.unlink(missing_ok=True)
                        on_signal(None, None, "the owner's app asked (commands/quit)")
            watcher = loop.create_task(watch_quit())

        installed = {}
        for name in ("SIGTERM", "SIGINT", "SIGBREAK"):
            sig = getattr(signal, name, None)
            if sig is None:
                continue
            try:
                installed[sig] = signal.signal(sig, on_signal)
            except (ValueError, OSError):
                pass   # not the main thread, or not this system's
        try:
            while not self.stopping.is_set():
                try:
                    await self.agent.listen(on_new=self.on_new, on_tick=self.tick, tick=TICK)
                except (OSError, relay.RelayError) as e:
                    self.say(f"connection lost ({e}); retrying in 10 seconds")
                    await asyncio.sleep(10)
        except asyncio.CancelledError:
            if not self.stopping.is_set():
                raise
        finally:
            if watcher:
                watcher.cancel()
                quit_file.unlink(missing_ok=True)   # left by an app that also sent SIGTERM
            for sig, old in installed.items():
                try:
                    signal.signal(sig, old)
                except (ValueError, OSError):
                    pass

    async def _shut_down(self, main: asyncio.Task, why: str) -> None:
        """After a stop signal: let the request being handled be answered
        (its command is being ended), then end run()."""
        self.say(f"stopping ({why})")
        if self._busy is not None:
            try:
                await asyncio.wait_for(self._busy.acquire(), SHUTDOWN_WAIT)
            except asyncio.TimeoutError:
                self.say("the current request wasn't answered in time; stopping anyway")
        main.cancel()

    def tick(self) -> None:
        self.handle_commands()
        self.ensure_invite()
        if time.time() - self._status_written > STATUS_EVERY:
            self.write_status()

    # --- the owner's commands ------------------------------------------------

    def handle_commands(self) -> None:
        """Commands from the owner's app: a file per command in
        <home>/commands, removed once done. `new-code` replaces the open
        pairing code (while unpaired); `unpair` drops every contact and
        offers a new code; `quit` is run()'s."""
        if not self.commands_dir or not self.commands_dir.is_dir():
            return
        for f in sorted(self.commands_dir.iterdir()):
            if f.name == "quit":
                continue
            if f.name.startswith("stop-"):
                # For the command running now (run_command watches and removes
                # it); one left behind by a command that had finished expires.
                try:
                    if time.time() - f.stat().st_mtime > 600:
                        f.unlink()
                except OSError:
                    pass
                continue
            try:
                f.unlink()
            except OSError:
                continue
            if f.name == "new-code":
                if not self.agent.contacts():
                    self.invite = None
                    self.ensure_invite()
            elif f.name == "unpair":
                self.unpair()
            else:
                self.say(f"unknown command file {f.name}")

    def unpair(self) -> None:
        with self.st.lock():
            self.st.put("contacts", {})
        self.paired = None
        self.invite = None
        self.log({"op": "unpair"})
        self.say("unpaired: contacts removed; a new pairing code follows")
        self.ensure_invite()
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
        self.say(f"pairing code {self.invite['code']} (link {self.invite['link']}; valid 15 minutes, renewed when it expires). "
                 f"Tell your agent: {pairing_message(self.invite['code'], self.agent.alias, self.agent.card)}")
        self.write_status()

    async def on_new(self, entries: list[dict]) -> None:
        if self._busy is None:
            self._busy = asyncio.Lock()
        async with self._busy:
            for e in entries:
                if self.stopping.is_set():
                    return   # stopping: what is left waits for the next start
                if e.get("direction") == "out":
                    continue
                if e["type"] == "paired":
                    # Owners compare this number on both sides, so it must be visible here too.
                    self.paired = {"alias": e.get("alias"), "verify": e.get("verify"), "at": int(time.time())}
                    self.log({"op": "paired", "alias": e.get("alias"), "sender": e.get("peer"), "verify": e.get("verify")})
                    self.say(f"paired with {e.get('alias')}: verification code {e.get('verify')} "
                             f"(your agent shows the same number; compare them)")
                    self.ensure_invite()
                    try:
                        await self.agent.sync_cards()   # tell it what this worker is, now
                    except (OSError, relay.RelayError):
                        pass  # the next poll does it
                elif e["type"] == "card":
                    self.log({"op": "card", "alias": e.get("alias"), "sender": e.get("peer")})
                    self.say(e.get("text", "card received"))
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
                    if not valid_id(rid):
                        await self.refuse(e, rid, BAD_ID)
                        return
                    await self.do_put(e, w[1], w[2])
                return
            req = e.get("request")
            if req:
                rid = req["id"]
                if not valid_id(rid):
                    await self.refuse(e, rid, BAD_ID)
                    return
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
        started = time.time()
        # The id is a plain name (handle checked it): "stop-../../key" can't
        # reach outside commands/.
        stop_file = self.commands_dir / f"stop-{req['id']}" if self.commands_dir and valid_id(req["id"]) else None
        result = await asyncio.get_running_loop().run_in_executor(None, run_command, cmd, self.work, timeout, stop_file,
                                                                  self.stopping)
        if processes.WINDOWS and NOT_RECOGNIZED in result["stderr"]:
            # An agent that took this for a Linux worker: say so where it looks.
            stderr = result["stderr"]
            result = dict(result, stderr=stderr + ("\n" if stderr and not stderr.endswith("\n") else "") + WINDOWS_HINT)
        self.note(e, "exec", True)
        self.record(req["id"], done_at=int(time.time()), duration=round(time.time() - started, 2), **result)
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
        self.record(req["id"], done_at=int(time.time()), size=target.stat().st_size)

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
        self.record(rid, done_at=int(time.time()), size=len(data))
        await self.reply(e, {"myous": "ack", "id": rid, "ok": True, "path": str(target), "size": len(data),
                             "sha256": files.sha256(data)})

    # --- review, paths, replies -----------------------------------------------------

    def review(self, op: str, e: dict, rid: str, **fields) -> str | None:
        """Why the request is refused, or None to go ahead (protocol.md 7.2)."""
        request = {"op": op, "id": rid, "sender": e.get("peer"), "alias": e.get("alias"), **fields}
        if self.review_cmd:
            self.record(rid, **request, at=int(time.time()), decision="pending")   # the hook may ask the owner
            why = self.run_review_hook(json.dumps(request))
        else:
            why = "worker is paused" if self.paused() else None
        self.log(dict(request, decision="allow" if why is None else f"refuse: {why}"))
        self.record(rid, **request, at=int(time.time()), decision="allow" if why is None else "refuse", reason=why)
        return why

    def run_review_hook(self, request: str) -> str | None:
        """Run the review command with the request on stdin: None to allow,
        else why not. It may wait for the owner; stopping the worker ends
        the wait (and the hook)."""
        try:
            # Not in the work directory: the hook must not pick up anything a request wrote there.
            proc = subprocess.Popen(self.review_cmd, shell=True, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, text=True, cwd=self.home,
                                    **({} if processes.WINDOWS else {"start_new_session": True}))
        except OSError as err:
            return f"the review hook could not run ({err})"
        deadline = time.time() + REVIEW_TIMEOUT
        stdin: str | None = request
        while True:
            try:
                _, stderr = proc.communicate(stdin, timeout=0.25)
                break
            except subprocess.TimeoutExpired:
                stdin = None   # given once; communicate keeps sending it
            if self.stopping.is_set() or time.time() >= deadline:
                processes.kill_tree(proc)
                proc.communicate()
                return STOPPING if self.stopping.is_set() else "the review hook didn't answer"
        return None if proc.returncode == 0 else ((stderr.strip().splitlines() or ["refused by the review hook"])[0])

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
        self.record(rid, done_at=int(time.time()), decision="refuse", reason=why)
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
        ]
        if processes.WINDOWS:
            lines.append(WINDOWS_HELP)
        if self.agent.card:
            lines.append(f"Its owner describes it: {self.agent.card}")
        lines += [
            "Use `myous exec <me> -- CMD`, `myous cp FILE <me>:PATH` and `myous cp <me>:PATH FILE` "
            "(protocol.md, section 7). Wait for each reply before the next request.",
            f"Shell: {processes.shell_name()} on {platform.system()} {platform.machine()}; Python {platform.python_version()}"
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
            "phase": "paused" if self.paused() else "running",
            "contacts": len(self.agent.contacts()),
            "invite": ({"code": self.invite["code"], "link": self.invite["link"], "expires_at": self.invite["expires_at"],
                        "message": pairing_message(self.invite["code"], self.agent.alias, self.agent.card)}
                       if self.invite else None),
            "paused": self.paused(), "requests": self.requests, "last": self.last, "paired": self.paired, "work": str(self.work),
            "updated": int(time.time()),
        })
        self._status_written = time.time()

    def log(self, record: dict) -> None:
        if not self.log_file:
            return
        with open(self.log_file, "a") as f:
            f.write(json.dumps(dict(record, at=int(time.time())), sort_keys=True) + "\n")

    def record(self, rid, **fields) -> None:
        """One JSON file per request in <home>/requests, updated as it
        progresses (decision, then outcome and output), for the owner's app
        to list. The newest REQUESTS_KEEP are kept."""
        if not self.requests_dir or not valid_id(rid):
            return   # an id that isn't a plain name was refused; worker.log has it
        try:
            self.requests_dir.mkdir(parents=True, exist_ok=True)
            path = self.requests_dir / f"{rid}.json"
            try:
                current = json.loads(path.read_text())
            except (OSError, ValueError):
                current = {"id": rid}
            current.update({k: v for k, v in fields.items() if v is not None})
            tmp = path.with_suffix(".tmp")
            tmp.write_text(json.dumps(current, sort_keys=True))
            tmp.replace(path)
            old = sorted(self.requests_dir.glob("*.json"), key=lambda p: p.stat().st_mtime)
            for p in old[:-REQUESTS_KEEP]:
                p.unlink(missing_ok=True)
        except OSError as err:
            self.say(f"could not record request {rid}: {err}")

    def say(self, text: str) -> None:
        print(text, file=self.out, flush=True)


def run_command(cmd: str, cwd: Path, timeout: int, stop_file: Path | None = None,
                stopping: threading.Event | None = None) -> dict:
    """Run a shell command (PowerShell on Windows) in its own process group, so a timeout kills
    whatever it started too. Output goes to temporary files (a daemon the
    command leaves behind can't hold us up by keeping a pipe open) and is
    cut to fit a message. While it runs, the owner's app can create
    `stop_file` to end it ("stopped by you" to the agent), and `stopping`
    (the worker being stopped) ends it too."""
    with tempfile.TemporaryFile() as out_f, tempfile.TemporaryFile() as err_f:
        proc = processes.shell_popen(cmd, cwd=cwd, stdin=subprocess.DEVNULL, stdout=out_f, stderr=err_f)
        deadline = time.time() + timeout
        note, stopped = None, False
        while True:
            try:
                proc.wait(timeout=0.25)
                break
            except subprocess.TimeoutExpired:
                pass
            if stopping is not None and stopping.is_set():
                stopped = True
                note = f"[myous: {STOPPING}]"
            elif stop_file is not None and stop_file.exists():
                stopped = True
                note = "[myous: stopped by the owner]"
            elif time.time() >= deadline:
                note = f"[myous: killed after {timeout} seconds]"
            else:
                continue
            processes.kill_tree(proc)
            proc.wait()
            break
        if stop_file is not None:
            try:
                stop_file.unlink()
            except OSError:
                pass
        out_f.seek(0)
        err_f.seek(0)
        out, err = out_f.read(), err_f.read()
    exit_code = proc.returncode if note is None else -1
    if note:
        err += b"\n" + note.encode()
    stdout, cut_out = _cut(out, STDOUT_LIMIT)
    stderr, cut_err = _cut(err, STDERR_LIMIT)
    result = {"exit": exit_code, "stdout": stdout, "stderr": stderr, "truncated": cut_out or cut_err}
    if stopped:
        result["stopped"] = True
    return result


def _cut(data: bytes, limit: int) -> tuple[str, bool]:
    if len(data) <= limit:
        return data.decode("utf-8", "replace"), False
    return data[:limit].decode("utf-8", "replace") + f"\n[myous: output cut at {limit} bytes]", True


def pairing_message(code: str, alias: str, about: str = "", windows: bool | None = None) -> str:
    """One sentence the owner pastes to their agent along with the code, so
    the agent knows what it is pairing with and what to do next (an agent
    that only gets a code may guess, for instance that it should become a
    worker itself). The owner's description of the worker, when set, goes
    in too: it is what the agent should remember the worker by. On Windows
    (default: this system) it says the worker takes PowerShell commands."""
    desc = f" It describes itself: \"{about}\"." if about else ""
    windows = processes.WINDOWS if windows is None else windows
    return (f"Pair with my worker \"{alias}\" (an environment I set up for you, not something to run yourself): "
            f"accept the pairing code {code} with relationship other and sharing \"my own worker; run commands "
            f"there for me\".{desc} Then send it the message help, and read the Workers section of "
            f"https://myoushq.com/skill.md before using it."
            + (" It runs on Windows and takes PowerShell commands." if windows else ""))

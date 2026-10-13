"""Optional helpers for agents on a machine with a persistent disk, cron (or
Task Scheduler on Windows) and background processes. None of this is required: an agent can call the
library from whatever scheduler or event source it has.

- a background listener that receives messages live (`myous listen`);
- `myous ensure`, a health check meant to be run regularly: restarts the
  listener if it died, and polls directly if it's down;
- a crontab entry (on Windows, a scheduled task) that runs `ensure` every
  minute, if the agent wants it;
- a hook command run when new messages or pairing results arrive, for
  agents that have some way to be woken.
"""
from __future__ import annotations

import html
import os
import re
import subprocess
import sys
import time
from pathlib import Path

from myous import processes
from myous.agent import Agent
from myous.storage import FileStorage

HEARTBEAT_STALE = 120
SAFETY_POLL_EVERY = 15 * 60
MAX_LOG_BYTES = 1024 * 1024
CRON_MARK = "# myous"
# Windows has no crontab: `myous cron` makes a scheduled task instead.
# Tests flip this.
WINDOWS = processes.WINDOWS
TASK_NAME = "myous ensure"
DEFAULT_HOME = "~/.myous"


def run_hook(st: FileStorage, entries: list[dict]) -> None:
    """Run the agent's hook, if it set one, without waiting for it."""
    command = st.get("settings", {}).get("on_message")
    if not command or not entries:
        return
    log = open(st.path("hook.log"), "a")
    processes.spawn_background(
        command, shell=True, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        env=dict(os.environ, MYOUS_HOME=str(st.home), MYOUS_NEW=str(len(entries))),
    )


async def run_listener(agent: Agent, st: FileStorage) -> None:
    def heartbeat() -> None:
        st.put("listener", {"pid": os.getpid(), "heartbeat": int(time.time())})

    def on_new(entries: list[dict]) -> None:
        for e in entries:
            print(f"{e['type']}: {e.get('alias', '')}", flush=True)
        run_hook(st, entries)

    heartbeat()
    await agent.listen(on_new=on_new, on_tick=heartbeat)


def listener_healthy(st: FileStorage) -> bool:
    info = st.get("listener", {})
    if time.time() - info.get("heartbeat", 0) > HEARTBEAT_STALE:
        return False
    pid = info.get("pid")
    if pid is None:
        return True  # just started; it hasn't written its pid yet
    return processes.alive(pid)


def start_listener(st: FileStorage) -> None:
    info = st.get("listener", {})
    pid = info.get("pid")
    if pid:
        # Stale or hung; but a process started after the listener's last
        # heartbeat is another program that got its pid (Windows).
        created = processes.created_at(pid)
        if created is None or created <= info.get("heartbeat", 0) + 5:
            processes.terminate(pid)
    log = open(st.path("listener.log"), "a")
    processes.spawn_background(
        [sys.executable, "-m", "myous", "listen"],
        stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        env=dict(os.environ, MYOUS_HOME=str(st.home)),
    )
    # Provisional heartbeat so a second `ensure` doesn't start another one.
    st.put("listener", {"pid": None, "heartbeat": int(time.time())})


def cron_line(st: FileStorage) -> str:
    return (f"* * * * * MYOUS_HOME={st.home} {sys.executable} -m myous ensure --quiet "
            f">> {st.path('cron.log')} 2>&1 {CRON_MARK}")


def _crontab() -> list[str] | None:
    try:
        current = subprocess.run(["crontab", "-l"], capture_output=True, text=True)
    except FileNotFoundError:
        return None
    return current.stdout.splitlines() if current.returncode == 0 else []


def install_cron(st: FileStorage) -> str:
    if WINDOWS:
        return _install_task(st)
    lines = _crontab()
    if lines is None:
        return "crontab is not available here"
    wanted = cron_line(st)
    if wanted in lines:
        return "cron entry already present"
    lines = [line for line in lines if CRON_MARK not in line] + [wanted]
    subprocess.run(["crontab", "-"], input="\n".join(lines) + "\n", text=True, check=True)
    return "cron entry installed"


def remove_cron(st: FileStorage | None = None) -> str:
    if WINDOWS:
        return _remove_task(st)
    lines = _crontab()
    if lines is None:
        return "crontab is not available here"
    kept = [line for line in lines if CRON_MARK not in line]
    if kept == lines:
        return "no cron entry to remove"
    subprocess.run(["crontab", "-"], input="\n".join(kept) + "\n", text=True, check=True)
    return "cron entry removed"


def show_cron(st: FileStorage | None = None) -> str:
    if WINDOWS:
        return _show_task(st)
    lines = _crontab()
    if lines is None:
        return "crontab is not available here"
    ours = [line for line in lines if CRON_MARK in line]
    return "\n".join(ours) or "no cron entry"


# What the scheduled task runs with pythonw.exe (no console window):
# `myous ensure --quiet`, with what it prints and any error in cron.log.
ENSURE_PYW = """\
# Run every minute by the scheduled task "{name}" (`myous cron install`).
import os, sys, traceback
HOME = {home!r}
os.environ["MYOUS_HOME"] = HOME
sys.stdout = sys.stderr = open(os.path.join(HOME, "cron.log"), "a", encoding="utf-8", errors="replace")
sys.argv = ["myous", "ensure", "--quiet"]
try:
    from myous.cli import main
    main()
except SystemExit as e:
    if e.code not in (None, 0):
        print(e.code)
except Exception:
    traceback.print_exc()
sys.stdout.flush()
"""


def _task_home(st: FileStorage | None) -> Path:
    return st.home if st else Path(os.environ.get("MYOUS_HOME") or DEFAULT_HOME).expanduser()


def task_name(st: FileStorage | None = None) -> str:
    """The scheduled task's name: `myous ensure` for the default home,
    `myous ensure (<folder>)` for another, so two agents on one PC get a
    task each."""
    home = _task_home(st)
    same = os.path.normcase(os.path.abspath(home)) == os.path.normcase(os.path.abspath(Path(DEFAULT_HOME).expanduser()))
    return TASK_NAME if same else f"{TASK_NAME} ({home.name})"


def _pythonw() -> tuple[str, bool]:
    """pythonw.exe next to this Python (no console window), and whether it
    was there."""
    w = Path(sys.executable).with_name("pythonw.exe")
    return (str(w), True) if w.is_file() else (sys.executable, False)


def task_command(st: FileStorage) -> str:
    """The scheduled task's /TR: pythonw.exe running ensure.pyw."""
    return f'"{_pythonw()[0]}" "{st.path("ensure.pyw")}"'


def _schtasks(*args: str) -> subprocess.CompletedProcess | str:
    """Run schtasks; a string says why it couldn't run."""
    try:
        return subprocess.run(["schtasks", *args], stdin=subprocess.DEVNULL, capture_output=True)
    except OSError as e:
        return str(e)


def _text(data: bytes) -> str:
    if data.startswith(b"\xff\xfe") or data[1:2] == b"\x00":
        return data.decode("utf-16", errors="replace").lstrip("\ufeff")
    return data.decode(errors="replace")


def _unavailable(r: subprocess.CompletedProcess | str) -> str:
    if isinstance(r, str):
        why = r
    else:
        lines = [line.strip() for line in (_text(r.stderr) + "\n" + _text(r.stdout)).splitlines() if line.strip()]
        why = lines[0] if lines else f"schtasks exited with {r.returncode}"
    return f"Task Scheduler is not available here: {why}"


def _task_line(r: subprocess.CompletedProcess) -> str | None:
    """From `schtasks /Query /XML`: the command the task runs, or None if
    there's no such task (or it can't be read)."""
    if r.returncode != 0:
        return None
    xml = _text(r.stdout)
    command = re.search(r"<Command>(.*?)</Command>", xml, re.S)
    if not command:
        return None
    exe = html.unescape(command.group(1)).strip()
    if " " in exe and not exe.startswith('"'):
        exe = f'"{exe}"'
    arguments = re.search(r"<Arguments>(.*?)</Arguments>", xml, re.S)
    return f"{exe} {html.unescape(arguments.group(1)).strip()}" if arguments else exe


def _same_command(a: str, b: str) -> bool:
    """Equal apart from quoting, spacing and case (how schtasks stores /TR
    varies)."""
    def norm(s: str) -> str:
        return " ".join(s.replace('"', " ").split()).lower()
    return norm(a) == norm(b)


def _query_task(name: str) -> subprocess.CompletedProcess | str:
    return _schtasks("/Query", "/TN", name, "/XML")


def _install_task(st: FileStorage) -> str:
    name = task_name(st)
    query = _query_task(name)
    if isinstance(query, str):
        return _unavailable(query)
    st.path("ensure.pyw").write_text(ENSURE_PYW.format(name=name, home=str(st.home)), encoding="utf-8")
    found = _pythonw()[1]
    note = "" if found else (f" (there is no pythonw.exe next to {sys.executable}, so the task runs it instead;"
                             " a console window may flash every minute)")
    wanted = task_command(st)
    existing = _task_line(query)
    if existing is not None and _same_command(existing, wanted):
        return "scheduled task already present" + note
    r = _schtasks("/Create", "/F", "/SC", "MINUTE", "/MO", "1", "/TN", name, "/TR", wanted)
    if isinstance(r, str) or r.returncode != 0:
        return _unavailable(r)
    return "scheduled task installed" + note


def _remove_task(st: FileStorage | None) -> str:
    name = task_name(st)
    query = _query_task(name)
    if isinstance(query, str):
        return _unavailable(query)
    if _task_line(query) is None:
        return "no scheduled task to remove"
    r = _schtasks("/Delete", "/F", "/TN", name)
    if isinstance(r, str) or r.returncode != 0:
        return _unavailable(r)
    return "scheduled task removed"


def _show_task(st: FileStorage | None) -> str:
    name = task_name(st)
    query = _query_task(name)
    if isinstance(query, str):
        return _unavailable(query)
    line = _task_line(query)
    return f"{name}: {line}" if line else "no scheduled task"


def trim_logs(st: FileStorage) -> None:
    for name in ("cron.log", "listener.log", "hook.log"):
        p = st.path(name)
        try:
            if p.stat().st_size > MAX_LOG_BYTES:
                # Bytes, not text: the log may not be in the locale's encoding.
                p.write_bytes(p.read_bytes()[-MAX_LOG_BYTES // 2:])
        except FileNotFoundError:
            pass

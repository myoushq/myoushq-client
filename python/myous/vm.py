"""Optional helpers for agents on a machine with a persistent disk, cron and
background processes. None of this is required: an agent can call the
library from whatever scheduler or event source it has.

- a background listener that receives messages live (`myous listen`);
- `myous ensure`, a health check meant to be run regularly: restarts the
  listener if it died, and polls directly if it's down;
- a crontab entry that runs `ensure` every minute, if the agent wants it;
- a hook command run when new messages or pairing results arrive, for
  agents that have some way to be woken.
"""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import time

from myous.agent import Agent
from myous.storage import FileStorage

HEARTBEAT_STALE = 120
SAFETY_POLL_EVERY = 15 * 60
MAX_LOG_BYTES = 1024 * 1024
CRON_MARK = "# myous"


def run_hook(st: FileStorage, entries: list[dict]) -> None:
    """Run the agent's hook, if it set one, without waiting for it."""
    command = st.get("settings", {}).get("on_message")
    if not command or not entries:
        return
    log = open(st.path("hook.log"), "a")
    subprocess.Popen(
        command, shell=True, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        start_new_session=True,
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
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def start_listener(st: FileStorage) -> None:
    pid = st.get("listener", {}).get("pid")
    if pid:
        try:
            os.kill(pid, signal.SIGTERM)  # stale or hung
        except OSError:
            pass
    log = open(st.path("listener.log"), "a")
    subprocess.Popen(
        [sys.executable, "-m", "myous", "listen"],
        stdin=subprocess.DEVNULL, stdout=log, stderr=log,
        start_new_session=True, env=dict(os.environ, MYOUS_HOME=str(st.home)),
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
    lines = _crontab()
    if lines is None:
        return "crontab is not available here"
    wanted = cron_line(st)
    if wanted in lines:
        return "cron entry already present"
    lines = [line for line in lines if CRON_MARK not in line] + [wanted]
    subprocess.run(["crontab", "-"], input="\n".join(lines) + "\n", text=True, check=True)
    return "cron entry installed"


def remove_cron() -> str:
    lines = _crontab()
    if lines is None:
        return "crontab is not available here"
    kept = [line for line in lines if CRON_MARK not in line]
    if kept == lines:
        return "no cron entry to remove"
    subprocess.run(["crontab", "-"], input="\n".join(kept) + "\n", text=True, check=True)
    return "cron entry removed"


def show_cron() -> str:
    lines = _crontab()
    if lines is None:
        return "crontab is not available here"
    ours = [line for line in lines if CRON_MARK in line]
    return "\n".join(ours) or "no cron entry"


def trim_logs(st: FileStorage) -> None:
    for name in ("cron.log", "listener.log", "hook.log"):
        p = st.path(name)
        try:
            if p.stat().st_size > MAX_LOG_BYTES:
                p.write_text(p.read_text()[-MAX_LOG_BYTES // 2:])
        except FileNotFoundError:
            pass

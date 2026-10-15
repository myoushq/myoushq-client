"""Processes the same way on every system: the shell a worker runs
commands in, stopping a command with everything it started, and whether
a pid is still alive. POSIX uses /bin/sh and process groups; Windows uses
PowerShell and `taskkill /T`."""
from __future__ import annotations

import base64
import os
import signal
import subprocess
import time

WINDOWS = os.name == "nt"

# Before the owner's command: no progress bars on stderr, UTF-8 output.
# After it: the exit status of what failed, so `exit` codes and failing
# programs come back as they would from a POSIX shell.
_PS_BEFORE = ("$ProgressPreference = 'SilentlyContinue'\n"
              "[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)\n"
              "$OutputEncoding = [Console]::OutputEncoding\n")
_PS_AFTER = ("\n$myousOk = $?; $myousCode = $LASTEXITCODE\n"
             "if (-not $myousOk) { if ($myousCode) { exit $myousCode } else { exit 1 } }\n"
             "exit 0\n")


def shell_name() -> str:
    """What the worker's help text calls its shell."""
    return "PowerShell" if WINDOWS else "/bin/sh"


def shell_popen(cmd: str, **kwargs) -> subprocess.Popen:
    """Start a shell command in its own process group (so kill_tree can end
    what it starts too)."""
    if not WINDOWS:
        return subprocess.Popen(cmd, shell=True, start_new_session=True, **kwargs)
    script = _PS_BEFORE + cmd + _PS_AFTER
    encoded = base64.b64encode(script.encode("utf-16-le")).decode()
    argv = ["powershell.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
            "-EncodedCommand", encoded]
    flags = subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW
    return subprocess.Popen(argv, creationflags=flags, **kwargs)


def background_kwargs() -> dict:
    """Popen arguments for a child that should outlive us and our console."""
    if WINDOWS:
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW}
    return {"start_new_session": True}


def spawn_background(args, **kwargs) -> subprocess.Popen:
    """Start a child that should outlive us and our console. On Windows it
    also leaves our job object where the job allows that: Task Scheduler
    runs each task in a job, and a listener left in it could keep the task
    "running" (so the next minute's run is skipped) or be ended with it."""
    if WINDOWS:
        try:
            flags = background_kwargs()["creationflags"] | subprocess.CREATE_BREAKAWAY_FROM_JOB
            return subprocess.Popen(args, creationflags=flags, **kwargs)
        except OSError:
            pass   # the job doesn't allow it: start the child inside it
    return subprocess.Popen(args, **background_kwargs(), **kwargs)


def kill_tree(proc: subprocess.Popen) -> None:
    """End a process started by shell_popen and everything it started."""
    try:
        if WINDOWS:
            subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], stdin=subprocess.DEVNULL,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           creationflags=subprocess.CREATE_NO_WINDOW)
            proc.kill()   # in case taskkill wasn't there
        else:
            os.killpg(proc.pid, signal.SIGKILL)
    except OSError:
        pass


def alive(pid: int) -> bool:
    """Whether a process with this pid exists. (On Windows, os.kill(pid, 0)
    would end it, so this asks the system instead.)"""
    if not WINDOWS:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        except OSError:
            return False
        return True
    import ctypes
    from ctypes import wintypes
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = k32.OpenProcess(0x1000, False, pid)   # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return ctypes.get_last_error() == 5   # access denied: it exists
    try:
        code = wintypes.DWORD()
        return bool(k32.GetExitCodeProcess(handle, ctypes.byref(code))) and code.value == 259   # STILL_ACTIVE
    finally:
        k32.CloseHandle(handle)


def created_at(pid: int) -> float | None:
    """When the process with this pid started (Unix time); None if it is
    gone or can't be asked. Pids are handed out again (on Windows soon,
    after a restart often at once; elsewhere once the numbers wrap), so a
    pid saved earlier may now be another program's: its start time tells.
    Linux reads /proc, other POSIX systems ask ps (whole seconds)."""
    if not WINDOWS:
        return _created_at_posix(pid)
    import ctypes
    from ctypes import wintypes
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.OpenProcess.restype = wintypes.HANDLE
    k32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    k32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    k32.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = k32.OpenProcess(0x1000, False, pid)   # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return None
    k32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    try:
        code = wintypes.DWORD()
        # An exited process whose pid is still held by an open handle (a
        # parent that hasn't waited) is gone: not STILL_ACTIVE (259).
        if not k32.GetExitCodeProcess(handle, ctypes.byref(code)) or code.value != 259:
            return None
        times = [wintypes.FILETIME() for _ in range(4)]
        if not k32.GetProcessTimes(handle, *[ctypes.byref(t) for t in times]):
            return None
        ticks = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime   # 100 ns since 1601
        return ticks / 1e7 - 11644473600
    finally:
        k32.CloseHandle(handle)


def _created_at_posix(pid: int) -> float | None:
    if os.path.exists("/proc/self/stat"):
        try:
            with open(f"/proc/{pid}/stat") as f:
                fields = f.read().rpartition(")")[2].split()   # the name, in parentheses, may hold spaces
            with open("/proc/stat") as f:
                boot = next(int(line.split()[1]) for line in f if line.startswith("btime "))
            if fields[0] == "Z":
                return None   # exited, not yet waited for
            return boot + int(fields[19]) / os.sysconf("SC_CLK_TCK")   # field 22: ticks after boot
        except (OSError, ValueError, IndexError, StopIteration):
            return None
    try:
        out = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], stdin=subprocess.DEVNULL, capture_output=True,
                             text=True, timeout=10, env=dict(os.environ, LC_ALL="C")).stdout.strip()
        return time.mktime(time.strptime(out, "%a %b %d %H:%M:%S %Y")) if out else None
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return None


def terminate(pid: int) -> None:
    """Ask a process to stop (SIGTERM; on Windows, end it)."""
    try:
        if WINDOWS:
            subprocess.run(["taskkill", "/F", "/PID", str(pid)], stdin=subprocess.DEVNULL,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           creationflags=subprocess.CREATE_NO_WINDOW)
        else:
            os.kill(pid, signal.SIGTERM)
    except OSError:
        pass

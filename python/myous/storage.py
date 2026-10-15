"""Where an agent keeps its myous data.

`Storage` is the interface; `FileStorage` keeps everything in one directory,
which suits agents with a persistent disk. Agents without one (serverless
runtimes, for example) implement `Storage` on whatever they have: a secrets
store for the key, a database or object store for the rest.

What must be durable:
  key        the private key. Losing it loses the identity.
  contacts   paired peers. Losing it loses every pairing.
What should be durable:
  state      handled message IDs, read position, registration flag
  history    message log
  settings   hub URL, alias, anything the agent adds
What can be lost:
  hub        cached hub config, refetched
  pending/*  pairings in progress, which expire after 15 minutes anyway
  claiming/* the id of a claim in flight, until its pairing is saved
"""
from __future__ import annotations

import abc
import contextlib
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, ContextManager, Iterator

if os.name == "nt":
    import msvcrt
else:
    import fcntl


SKIP_DIRS = {"venv", "bin", "node_modules", "locks", "target", ".git"}

class Storage(abc.ABC):
    @abc.abstractmethod
    def load_key(self) -> str | None:
        """The private key (nsec), or None if there isn't one."""

    @abc.abstractmethod
    def save_key(self, nsec: str) -> None:
        """Store the private key. Must refuse to overwrite an existing one."""

    @abc.abstractmethod
    def get(self, name: str, default: Any = None) -> Any:
        """A small JSON document by name ("contacts", "state", "pending/4821", ...)."""

    @abc.abstractmethod
    def put(self, name: str, value: Any) -> None:
        """Replace a document. Should be atomic."""

    @abc.abstractmethod
    def delete(self, name: str) -> None:
        """Remove a document; no error if it doesn't exist."""

    @abc.abstractmethod
    def names(self, prefix: str) -> list[str]:
        """Names of documents starting with prefix (e.g. "pending/")."""

    @abc.abstractmethod
    def append_history(self, entry: dict) -> None:
        """Append one record to the message history."""

    @abc.abstractmethod
    def read_history(self) -> list[dict]:
        """The whole message history, oldest first."""

    def lock(self, name: str = "state", wait: bool = True) -> ContextManager[bool]:
        """Mutual exclusion between concurrent runs of the same agent.

        Yields True when held; with wait=False, yields False instead of
        blocking. The default does nothing, which is fine for an agent that
        never runs two myous operations at once."""
        return contextlib.nullcontext(True)


DEFAULT_HOME = "~/.myous"


def installed_home() -> Path | None:
    """The myous directory this client is installed in, if it is one of
    its own: the venv is <directory>/venv, as skill.md installs it, and the
    directory holds an identity (a key or settings). None otherwise."""
    prefix = Path(sys.prefix)
    if prefix.name != "venv":
        return None
    home = prefix.parent
    return home if (home / "key").exists() or (home / "settings.json").exists() else None


def home_mismatch() -> str | None:
    """Why this command shouldn't run, if MYOUS_HOME isn't set and the
    client is installed in a myous directory other than ~/.myous: it would
    use ~/.myous, which may be another agent's (an agent whose shell
    forgets variables between commands runs into this). None if all is
    well."""
    if os.environ.get("MYOUS_HOME"):
        return None
    home, default = installed_home(), Path(DEFAULT_HOME).expanduser()
    if home is None or home.resolve() == default.resolve():
        return None
    setting = (f'$env:MYOUS_HOME = "{home}"; at the start of the command' if os.name == "nt"
               else f"MYOUS_HOME={home} in front of the command")
    return (f"error: MYOUS_HOME isn't set, so this would use {default}, but this myous is installed in {home}, "
            f"a myous directory of its own. Put {setting}.")


class FileStorage(Storage):
    """Everything in one directory (default ~/.myous, or $MYOUS_HOME):
    key, contacts.json, state.json, settings.json, hub.json, pending/*.json,
    messages.jsonl."""

    def __init__(self, home: str | os.PathLike | None = None):
        self.home = Path(home or os.environ.get("MYOUS_HOME") or DEFAULT_HOME).expanduser()
        self.home.mkdir(mode=0o700, parents=True, exist_ok=True)

    def path(self, name: str) -> Path:
        return self.home / name

    def load_key(self) -> str | None:
        try:
            return self.path("key").read_text().strip()
        except FileNotFoundError:
            return None

    def save_key(self, nsec: str) -> None:
        # O_EXCL: never replace an existing key, even in a race.
        fd = os.open(self.path("key"), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(nsec + "\n")
            f.flush()
            os.fsync(f.fileno())

    def get(self, name: str, default: Any = None) -> Any:
        try:
            return json.loads(self.path(name + ".json").read_text())
        except FileNotFoundError:
            return default

    def put(self, name: str, value: Any) -> None:
        self.write_private(self.path(name + ".json"), json.dumps(value, indent=2, sort_keys=True) + "\n")

    def delete(self, name: str) -> None:
        self.path(name + ".json").unlink(missing_ok=True)

    def names(self, prefix: str) -> list[str]:
        directory, _, stem = prefix.rpartition("/")
        base = self.path(directory) if directory else self.home
        if not base.is_dir():
            return []
        found = sorted(p.stem for p in base.glob(stem + "*.json"))
        return [f"{directory}/{n}" if directory else n for n in found]

    def append_history(self, entry: dict) -> None:
        fd = os.open(self.path("messages.jsonl"), os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        with os.fdopen(fd, "a") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")
            f.flush()
            os.fsync(f.fileno())

    def read_history(self) -> list[dict]:
        try:
            lines = self.path("messages.jsonl").read_text().splitlines()
        except FileNotFoundError:
            return []
        return [json.loads(line) for line in lines if line.strip()]

    def last_used(self) -> int | None:
        """Unix time of the newest file under the home: when this agent last
        did anything, without writing on every command. Skips installs
        (venv, bin, node_modules, target), locks and .git."""
        newest = 0
        for root, dirs, files in os.walk(self.home):
            dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
            for f in files:
                try:
                    newest = max(newest, int(os.stat(os.path.join(root, f)).st_mtime))
                except OSError:
                    pass
        return newest or None

    @contextlib.contextmanager
    def lock(self, name: str = "state", wait: bool = True) -> Iterator[bool]:
        lock_dir = self.path("locks")
        lock_dir.mkdir(mode=0o700, exist_ok=True)
        fd = os.open(lock_dir / name.replace("/", "_"), os.O_WRONLY | os.O_CREAT, 0o600)
        try:
            if not _lock(fd, wait):
                yield False
                return
            try:
                yield True
            finally:
                _unlock(fd)
        finally:
            os.close(fd)

    @staticmethod
    def write_private(p: Path, text: str) -> None:
        """Atomic write, readable only by this user."""
        p.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        tmp = p.with_name(p.name + ".tmp")
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, p)


def _lock(fd: int, wait: bool) -> bool:
    """An exclusive lock on an open file (flock; on Windows, a lock on its
    first byte). False if it is held elsewhere and wait is False."""
    if os.name != "nt":
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | (0 if wait else fcntl.LOCK_NB))
        except BlockingIOError:
            return False
        return True
    os.lseek(fd, 0, os.SEEK_SET)
    while True:
        try:
            msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
            return True
        except OSError:
            if not wait:
                return False
            time.sleep(0.05)


def _unlock(fd: int) -> None:
    if os.name != "nt":
        fcntl.flock(fd, fcntl.LOCK_UN)
        return
    os.lseek(fd, 0, os.SEEK_SET)
    msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)

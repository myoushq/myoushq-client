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
"""
from __future__ import annotations

import abc
import contextlib
import fcntl
import json
import os
from pathlib import Path
from typing import Any, ContextManager, Iterator


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


class FileStorage(Storage):
    """Everything in one directory (default ~/.myous, or $MYOUS_HOME):
    key, contacts.json, state.json, settings.json, hub.json, pending/*.json,
    messages.jsonl."""

    def __init__(self, home: str | os.PathLike | None = None):
        self.home = Path(home or os.environ.get("MYOUS_HOME", "~/.myous")).expanduser()
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

    @contextlib.contextmanager
    def lock(self, name: str = "state", wait: bool = True) -> Iterator[bool]:
        lock_dir = self.path("locks")
        lock_dir.mkdir(mode=0o700, exist_ok=True)
        fd = os.open(lock_dir / name.replace("/", "_"), os.O_WRONLY | os.O_CREAT, 0o600)
        try:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | (0 if wait else fcntl.LOCK_NB))
            except BlockingIOError:
                yield False
                return
            try:
                yield True
            finally:
                fcntl.flock(fd, fcntl.LOCK_UN)
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

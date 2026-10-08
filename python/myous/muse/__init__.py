"""Helpers for Meta's Muse, shipped with the package so a PyPI install has
them (docs/muse.md, served at https://myoushq.com/muse.md):

- `watcher`: the one-shot watcher that waits for new items and exits, so
  whoever started it (the hook, or a background job) wakes Muse.
  `myous watcher --for 55` / `myous watcher --takeover`.
- `check`: the scheduled backstop (`myous check`): poll once, say what
  needs doing.
- `hook.sh`: the runtime-managed hook that runs the watcher every minute;
  `myous hook-script` prints its path so it can be registered.

Until v0.5.0 these lived in `examples/muse/` of the repository; the shims
there still run them.
"""
from __future__ import annotations

import importlib.resources
from pathlib import Path


def hook_script() -> Path:
    """Absolute path of the packaged hook.sh."""
    return Path(str(importlib.resources.files(__name__) / "hook.sh")).resolve()

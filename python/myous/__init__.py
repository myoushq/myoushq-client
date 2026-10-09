"""myous: encrypted messaging between paired AI agents.

Library use: `Agent` with a `Storage` (`FileStorage`, or your own).
Command line: `myous --help`. Protocol: PROTOCOL.md.
"""

__version__ = "0.5.1"

from myous.agent import Agent, IdentityError  # noqa: E402
from myous.storage import FileStorage, Storage  # noqa: E402

__all__ = ["Agent", "IdentityError", "FileStorage", "Storage"]

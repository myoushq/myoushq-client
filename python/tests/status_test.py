"""The status command's fields for the desktop app, the init guard against
adopting another agent's home, and added_by on contacts."""
import argparse
import io
import json
import os
import tempfile
import time
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from myous import __version__, cli, contacts
from myous.agent import Agent
from myous.storage import FileStorage


class StatusTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.st = FileStorage(self.tmp.name)
        self.agent = Agent(self.st, "http://hub.invalid")

    def tearDown(self):
        self.tmp.cleanup()

    def status(self):
        out = io.StringIO()
        with redirect_stdout(out):
            cli.cmd_status(self.agent, self.st, argparse.Namespace(json=True))
        return json.loads(out.getvalue())

    def test_fields_without_identity(self):
        info = self.status()
        self.assertEqual(info["client"], "python")
        self.assertEqual(info["version"], __version__)
        self.assertIsNone(info["identity"])
        self.assertEqual(info["contacts"], 0)
        self.assertEqual(info["contact_list"], [])
        self.assertEqual(info["pending_pairings"], [])
        self.assertEqual(list(info)[:3], ["data_dir", "client", "version"])
        self.assertEqual(list(info)[-1], "last_used")

    def test_contact_list_and_added_by(self):
        self.agent.create_identity()
        peer = Agent(FileStorage(tempfile.mkdtemp()), "http://hub.invalid")
        peer.create_identity()
        hex_key = peer.keys.public_key().to_hex()
        contacts.add(self.st, hex_key, "Max's Muse")
        contacts.update(self.st, "Max's Muse", **contacts.context_fields("friend", "anything", "owner"))
        info = self.status()
        self.assertEqual(info["contacts"], 1)
        c = info["contact_list"][0]
        self.assertEqual(c["alias"], "Max's Muse")
        self.assertEqual(c["npub"], peer.keys.public_key().to_bech32())
        self.assertEqual(c["status"], "approved")
        self.assertEqual(c["relationship"], "friend")
        self.assertEqual(c["added_by"], "owner")
        self.assertIsInstance(c["paired_at"], int)
        self.assertNotIn("sharing", contacts.context_fields(added_by=" owner "))
        self.assertEqual(contacts.context_fields(added_by=" owner "), {"added_by": "owner"})
        self.assertEqual(contacts.context_fields(), {})

    def test_last_used_is_the_newest_file_outside_installs(self):
        self.assertIsNone(FileStorage(tempfile.mkdtemp()).last_used())
        self.st.put("settings", {"alias": "A"})
        old = time.time() - 3600
        os.utime(self.st.path("settings.json"), (old, old))
        self.assertEqual(self.st.last_used(), int(old))
        venv = Path(self.tmp.name, "venv", "bin")
        venv.mkdir(parents=True)
        (venv / "python").write_text("")  # a fresh install must not count
        self.assertEqual(self.st.last_used(), int(old))
        hist = Path(self.tmp.name, "history")
        hist.mkdir()
        (hist / "x.jsonl").write_text("{}")
        self.assertGreater(self.st.last_used(), int(old))
        self.assertEqual(self.status()["last_used"], self.st.last_used())


class InitGuardTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.st = FileStorage(self.tmp.name)
        self.agent = Agent(self.st, "http://hub.invalid")
        self.agent.create_identity()
        self.st.put("settings", {"alias": "Sam's Muse"})

    def tearDown(self):
        self.tmp.cleanup()

    def init(self, alias, rename=False):
        async def fake_register(alias):
            self.registered = alias
        with mock.patch.object(self.agent, "register", fake_register), mock.patch.object(self.agent, "is_registered", lambda: True):
            with redirect_stdout(io.StringIO()):
                cli.cmd_init(self.agent, self.st, argparse.Namespace(alias=alias, rename=rename))

    def test_another_alias_is_refused(self):
        with self.assertRaises(SystemExit) as e:
            self.init("Codex")
        self.assertEqual(str(e.exception), "this directory belongs to Sam's Muse; use another MYOUS_HOME, or pass --rename if this is the same agent")
        self.assertFalse(hasattr(self, "registered"))

    def test_same_alias_and_rename_pass(self):
        self.init("Sam's Muse")
        self.assertEqual(self.registered, "Sam's Muse")
        self.init("Sam's Muse 2", rename=True)
        self.assertEqual(self.registered, "Sam's Muse 2")

    def test_no_stored_alias_passes(self):
        self.st.put("settings", {})
        self.init("Codex")
        self.assertEqual(self.registered, "Codex")


if __name__ == "__main__":
    unittest.main()

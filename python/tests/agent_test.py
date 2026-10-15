"""The Agent's bookkeeping around listen() and worker replies, without a
relay: delivery happens once, and only the right contact can answer."""
import asyncio
import contextlib
import io
import json
import os
import sys
import tempfile
import time
import unittest
from unittest import mock

try:
    from inbox_test import MemoryStorage  # unittest discover puts tests/ on the path
except ModuleNotFoundError:
    from tests.inbox_test import MemoryStorage
from nostr_sdk import Keys

from myous import cli, contacts, inbox, processes, relay
from myous import hub as hub_module
from myous.hub import Hub
from myous.agent import Agent, _Deliveries, timeout_text


class Cards(unittest.TestCase):
    """Which contacts are told this agent's card, and when."""

    def setUp(self):
        self.st = MemoryStorage()
        self.st.put("settings", {"alias": "Max's Muse"})
        self.agent = Agent(self.st)
        contacts.add(self.st, "a" * 64, "old friend")                       # from before cards: nothing recorded
        contacts.add(self.st, "b" * 64, "new friend")
        contacts.peer_knows(self.st, "b" * 64, "Max's Muse", "")           # paired now: knows the alias
        contacts.add(self.st, "c" * 64, "blocked one")
        contacts.update(self.st, "blocked one", status="blocked")

    def due(self):
        return sorted(c["alias"] for c in self.agent.cards_due())

    def test_no_card_nothing_to_say(self):
        self.assertEqual(self.due(), [])

    def test_a_card_goes_to_everyone_once(self):
        self.agent.set_card("Max's own assistant")
        self.assertEqual(self.due(), ["new friend", "old friend"])
        contacts.peer_knows(self.st, "b" * 64, "Max's Muse", "Max's own assistant")
        self.assertEqual(self.due(), ["old friend"])

    def test_a_rename_is_announced_to_those_who_knew_the_old_name(self):
        self.st.put("settings", {"alias": "Max's Assistant"})
        self.assertEqual(self.due(), ["new friend"])   # the old friend never recorded a name; nothing to correct

    def test_limits(self):
        with self.assertRaises(ValueError):
            self.agent.set_card("x" * 501)
        self.assertEqual(self.agent.set_card("  spaced  "), "spaced")
        self.assertEqual(self.agent.set_card(None), "")


class Deliveries(unittest.TestCase):
    def setUp(self):
        self.st = MemoryStorage()
        self.agent = Agent(self.st)

    def test_each_entry_is_delivered_once_even_when_both_paths_see_it(self):
        seen = []

        async def slow_on_new(entries):
            seen.append([e["seq"] for e in entries])
            await asyncio.sleep(0.05)  # still busy when the other path looks

        d = _Deliveries(self.agent, slow_on_new)
        with self.st.lock():
            inbox.record(self.st, {"type": "message", "direction": "in", "peer": "p", "alias": "a", "text": "one"})
            inbox.record(self.st, {"type": "message", "direction": "out", "peer": "p", "alias": "a", "text": "mine"})
        async def both():
            await asyncio.gather(d.deliver(), d.deliver())  # the stream and housekeeping, at once

        asyncio.run(both())
        self.assertEqual(seen, [[1]])
        with self.st.lock():
            inbox.record(self.st, {"type": "message", "direction": "in", "peer": "p", "alias": "a", "text": "two"})
        asyncio.run(d.deliver())
        self.assertEqual(seen, [[1], [3]])

    def test_replies_only_count_from_the_contact_asked(self):
        rid = "c" * 32
        with self.st.lock():
            inbox.record(self.st, {"type": "ack", "direction": "in", "peer": "npub1other", "alias": "o", "id": rid, "ok": True})
            inbox.record(self.st, {"type": "file", "direction": "in", "peer": "npub1other", "alias": "o", "w": ["file", rid]})
        self.assertIsNone(self.agent._find_reply(rid, 1, "npub1worker"))
        with self.st.lock():
            inbox.record(self.st, {"type": "ack", "direction": "in", "peer": "npub1worker", "alias": "w", "id": rid, "ok": False, "error": "no"})
        self.assertEqual(self.agent._find_reply(rid, 1, "npub1worker")["error"], "no")


if __name__ == "__main__":
    unittest.main()


class Connections(unittest.TestCase):
    """Which connection a send goes out on: the listener's while it is up
    and connected, else a fresh one."""

    class FakeConn:
        def __init__(self, up):
            self.up = up

        async def connected(self):
            return self.up

    def setUp(self):
        self.st = MemoryStorage()
        self.agent = Agent(self.st)
        self.opened = []

        def connect(cfg=None):
            conn = Connections.Fresh()
            self.opened.append(conn)
            return conn

        self.agent._connect = connect

    class Fresh:
        closed = False

        async def __aenter__(self):
            return self

        async def __aexit__(self, *exc):
            self.closed = True

    def use(self, given=None):
        async def run():
            async with self.agent._use(given) as conn:
                return conn
        return asyncio.run(run())

    def test_the_given_connection_is_used_as_is(self):
        given = object()
        self.assertIs(self.use(given), given)
        self.assertEqual(self.opened, [])

    def test_the_listening_connection_is_reused_while_connected(self):
        self.agent._live = live = self.FakeConn(up=True)
        self.assertIs(self.use(), live)
        self.assertEqual(self.opened, [])

    def test_a_reconnecting_listener_is_bypassed(self):
        self.agent._live = self.FakeConn(up=False)
        conn = self.use()
        self.assertEqual(self.opened, [conn])
        self.assertTrue(conn.closed)

    def test_no_listener_means_a_fresh_connection_closed_afterwards(self):
        conn = self.use()
        self.assertEqual(self.opened, [conn])
        self.assertTrue(conn.closed)


class RequestTtl(unittest.TestCase):
    """What exec, put and get send as their ttl (protocol.md 7.1), and what
    a request that got no reply says and leaves behind."""

    def setUp(self):
        self.st = MemoryStorage()
        contacts.add(self.st, "d" * 64, "box")
        self.agent = Agent(self.st)
        self.sent, self.tags, self.waits = [], [], []

        @contextlib.asynccontextmanager
        async def connect(cfg=None):
            yield object()

        async def send(name, text, conn=None):
            self.sent.append(json.loads(text))

        async def send_file(name, path, extra_tags=None, conn=None):
            self.tags.append(extra_tags)

        async def await_reply(rid, timeout, npub, conn=None, expires=None):
            self.waits.append((timeout, expires))
            return {"type": "ack", "ok": True, "id": rid}

        self.agent._connect = connect
        self.agent.send = send
        self.agent.send_file = send_file
        self.agent._await_reply = await_reply
        self.agent.fetch = lambda reply, to: "fetched"

    def test_exec_and_get_carry_the_ttl_in_the_json(self):
        before = time.time()
        asyncio.run(self.agent.exec("box", "ls"))
        self.assertEqual((self.sent[-1]["timeout"], self.sent[-1]["ttl"]), (120, 150))   # as long as it waits
        wait, expires = self.waits[-1]
        self.assertEqual(wait, 150)
        self.assertTrue(before + 150 <= expires <= time.time() + 150)   # from when it went out
        asyncio.run(self.agent.exec("box", "ls", timeout=10.5))
        self.assertEqual((self.sent[-1]["timeout"], self.sent[-1]["ttl"]), (10, 40))
        asyncio.run(self.agent.exec("box", "ls", ttl=3600))
        self.assertEqual(self.sent[-1]["ttl"], 3600)
        self.assertEqual(self.waits[-1][0], 150)   # a longer ttl doesn't make it wait longer
        asyncio.run(self.agent.exec("box", "ls", ttl=10 ** 6))
        self.assertEqual(self.sent[-1]["ttl"], 86400)

        self.assertEqual(asyncio.run(self.agent.get("box", "out.txt")), "fetched")
        self.assertEqual((self.sent[-1]["myous"], self.sent[-1]["path"], self.sent[-1]["ttl"]), ("get", "out.txt", 300))
        asyncio.run(self.agent.get("box", "out.txt", timeout=20, ttl=7))
        self.assertEqual(self.sent[-1]["ttl"], 7)
        self.assertEqual(self.waits[-1][0], 20)

    def test_put_carries_it_as_the_fifth_value_of_the_w_tag(self):
        asyncio.run(self.agent.put("box", __file__, "in.py"))
        w = self.tags[-1][0]
        self.assertEqual((w[0], w[1], w[3], w[4]), ("w", "put", "in.py", "300"))
        self.assertEqual(len(w[2]), 32)
        asyncio.run(self.agent.put("box", __file__, "in.py", timeout=60, ttl=900))
        self.assertEqual(self.tags[-1][0][4], "900")

    def test_a_ttl_is_at_least_a_second(self):
        for ttl in (0, -1, 0.5, True, float("nan"), "60"):
            with self.assertRaises(ValueError, msg=repr(ttl)):
                asyncio.run(self.agent.exec("box", "ls", ttl=ttl))
            with self.assertRaises(ValueError, msg=repr(ttl)):
                asyncio.run(self.agent.put("box", __file__, "in.py", ttl=ttl))
        self.assertEqual(self.sent, [])
        self.assertEqual(self.tags, [])

    def test_what_a_timeout_says(self):
        gone = timeout_text(150, time.time() - 1)
        self.assertEqual(gone, "no reply from the worker within 150 seconds; it won't start this request any more "
                               "(it is off or busy), but if it already has, its reply will show in `myous inbox`")
        # A ttl that ends with the wait (the default) is over: no race on the seconds between them.
        self.assertEqual(timeout_text(40, time.time() + 2), gone.replace("150", "40"))
        later = time.time() + 3600
        self.assertEqual(timeout_text(150, later),
                         f"no reply from the worker within 150 seconds; it may still run this request in the next "
                         f"60 minutes (its ttl; until {time.strftime('%H:%M', time.localtime(later))} this computer's "
                         f"time), and its reply will show in `myous inbox` when it comes")
        self.assertIn("in the next 23 hours", timeout_text(150, time.time() + 23 * 3600 - 30))
        self.assertIn("in the next 45 seconds", timeout_text(150, time.time() + 44.5))

    def test_a_request_given_up_on_is_remembered(self):
        async def no_reply(rid, timeout, npub, conn=None, expires=None):
            raise TimeoutError("no reply from the worker")

        self.agent._await_reply = no_reply
        before = int(time.time())
        for request in (lambda: self.agent.exec("box", "make test", ttl=3600),
                        lambda: self.agent.put("box", __file__, "/work/in.py", timeout=60),
                        lambda: self.agent.get("box", "/work/out.txt", "out.txt")):
            with self.assertRaises(TimeoutError):
                asyncio.run(request())
        gave_up = self.st.get("gave_up")
        exec_id, get_id, put_id = self.sent[0]["id"], self.sent[1]["id"], self.tags[0][0][2]
        self.assertEqual(sorted(gave_up), sorted([exec_id, get_id, put_id]))
        npub = contacts.find(self.st, "box")[1]["npub"]
        for rid, what, ttl in ((exec_id, {"op": "exec", "cmd": "make test"}, 3600),
                               (put_id, {"op": "put", "local": __file__, "remote": "/work/in.py"}, 60),
                               (get_id, {"op": "get", "remote": "/work/out.txt", "local": "out.txt"}, 300)):
            r = gave_up[rid]
            self.assertEqual({k: r[k] for k in what}, what)
            self.assertEqual((r["peer"], r["alias"]), (npub, "box"))
            self.assertTrue(before <= r["sent_at"] <= time.time())
            self.assertEqual((r["expires"] - r["sent_at"], r["ttl"]), (ttl, ttl))
            self.assertNotIn("pid", r)   # given up: no longer waited for
        self.assertEqual((gave_up[exec_id]["wait"], gave_up[put_id]["wait"]), (150, 60))

    def test_while_it_waits_the_request_names_this_process(self):
        seen = []

        async def await_reply(rid, timeout, npub, conn=None, expires=None):
            seen.append(self.st.get("gave_up")[rid])
            return {"type": "result", "id": rid, "exit": 0}

        self.agent._await_reply = await_reply
        asyncio.run(self.agent.exec("box", "ls"))
        self.assertEqual((seen[0]["op"], seen[0]["cmd"], seen[0]["pid"]), ("exec", "ls", os.getpid()))
        self.assertLess(abs(seen[0]["pid_started"] - processes.created_at(os.getpid())), 2)

    def test_a_reply_in_time_leaves_nothing_behind(self):
        asyncio.run(self.agent.exec("box", "ls"))
        self.assertEqual(self.st.get("gave_up"), {})

    def test_a_wait_ended_otherwise_gives_the_request_up(self):
        # A caller's own timeout around exec cancels the wait; this process
        # lives on, so the reply must not stay the command's.
        async def cancelled(rid, timeout, npub, conn=None, expires=None):
            raise asyncio.CancelledError

        self.agent._await_reply = cancelled
        with self.assertRaises(asyncio.CancelledError):
            asyncio.run(self.agent.exec("box", "ls"))
        [r] = self.st.get("gave_up").values()
        self.assertNotIn("pid", r)

    def test_the_timeout_error_says_it(self):
        agent = Agent(MemoryStorage())

        async def listen(**kwargs):
            await asyncio.sleep(3600)

        agent.listen = listen
        with self.assertRaises(TimeoutError) as cm:
            asyncio.run(agent._await_reply("c" * 32, 0.1, "npub1worker", expires=time.time() + 3600))
        self.assertIn("it may still run this request in the next 60 minutes", str(cm.exception))
        with self.assertRaises(TimeoutError) as cm:
            asyncio.run(agent._await_reply("c" * 32, 0.1, "npub1worker", expires=time.time()))
        self.assertIn("it won't start this request any more", str(cm.exception))


class RelayDown(unittest.TestCase):
    def test_listen_keeps_since_when_no_relay_is_connected(self):
        agent = Agent(MemoryStorage())
        agent.hub.config = lambda refresh=False: {"relays": ["ws://127.0.0.1:9"]}

        def unreachable(cfg=None):
            raise OSError("connection refused")

        agent._connect = unreachable
        self.assertIsNone(agent.relay_down_since)
        before = time.time()
        with self.assertRaises(OSError):
            asyncio.run(agent.listen())
        since = agent.relay_down_since
        self.assertTrue(before <= since <= time.time())
        with self.assertRaises(OSError):
            asyncio.run(agent.listen())
        self.assertEqual(agent.relay_down_since, since)   # since the first time


class RelayUnreachable(unittest.TestCase):
    """A poll that reaches no relay fails at once instead of passing for an
    empty inbox, and the last poll stays when it was last done."""

    def setUp(self):
        self.st = MemoryStorage()
        self.st.key = Keys.generate().secret_key().to_bech32()
        self.st.put("state", {"last_poll": 123})
        self.agent = Agent(self.st)
        self.agent.hub.config = lambda refresh=False: {"relays": ["ws://127.0.0.1:1"]}   # refused

    def test_poll_raises_and_keeps_last_poll(self):
        started = time.time()
        with self.assertRaises(relay.RelayError) as cm:
            asyncio.run(self.agent.poll())
        self.assertLess(time.time() - started, 10)   # one attempt, not the retrying connect's full wait
        self.assertIn("couldn't reach the relay at ws://127.0.0.1:1", str(cm.exception))
        self.assertEqual(self.st.get("state")["last_poll"], 123)

    def test_inbox_warns_and_shows_what_is_stored(self):
        out, err = io.StringIO(), io.StringIO()
        args = cli.build_parser().parse_args(["inbox"])
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            args.func(self.agent, self.st, args)
        self.assertEqual(out.getvalue(), "no new messages\n")
        self.assertIn("warning: couldn't fetch new items (couldn't reach the relay at ws://127.0.0.1:1", err.getvalue())
        self.assertEqual(self.st.get("state")["last_poll"], 123)


class HubConfigBackoff(unittest.TestCase):
    """A stale config the hub can't refresh is used as it is, and not
    asked for again for a while: each try may wait for its timeout."""

    def test_a_failed_refresh_is_not_retried_at_once(self):
        st = MemoryStorage()
        hub = Hub(st, "https://hub.test")
        st.put("hub", {"url": "https://hub.test", "relays": ["wss://r"], "fetched_at": 0})
        calls = []

        def unreachable(*args, **kwargs):
            calls.append(kwargs.get("timeout"))
            raise OSError("timed out")

        hub.request = unreachable
        self.assertEqual(hub.config()["relays"], ["wss://r"])
        self.assertEqual(hub.config()["relays"], ["wss://r"])
        self.assertEqual(calls, [hub_module.CONFIG_TIMEOUT])   # asked once
        st.put("hub", dict(st.get("hub"), retry_at=0))           # the pause is over
        hub.config()
        self.assertEqual(len(calls), 2)


class StdinHasData(unittest.TestCase):
    """cli.stdin_has_data: input piped or redirected in, never a hang or a
    false alarm on a terminal, /dev/null or an empty pipe."""

    def check(self, f):
        with f, mock.patch.object(sys, "stdin", f):
            return cli.stdin_has_data()

    def pipe(self, data, close=True):
        r, w = os.pipe()
        if data:
            os.write(w, data)
        if close:
            os.close(w)
        else:
            self.addCleanup(os.close, w)
        return open(r)

    def test_pipes(self):
        self.assertTrue(self.check(self.pipe(b"print(1)\n")))
        self.assertFalse(self.check(self.pipe(None)))                  # closed, nothing written
        started = time.time()
        self.assertFalse(self.check(self.pipe(None, close=False)))     # open, nothing written: an agent's shell
        self.assertLess(time.time() - started, 2)

    def test_files_and_devices(self):
        with tempfile.NamedTemporaryFile("w", delete=False) as f:
            f.write("print(1)\n")
        self.addCleanup(os.unlink, f.name)
        self.assertTrue(self.check(open(f.name)))
        open(f.name, "w").close()
        self.assertFalse(self.check(open(f.name)))                     # an empty file
        self.assertFalse(self.check(open(os.devnull)))


class CliTtl(unittest.TestCase):
    """--ttl on `myous exec` and `myous cp` reaches the request."""

    class FakeAgent:
        def __init__(self, st):
            self.st, self.calls = st, []

        async def exec(self, name, cmd, timeout=120, ttl=None):
            self.calls.append(("exec", name, cmd, timeout, ttl))
            return {"exit": 0, "stdout": "", "stderr": ""}

        async def put(self, name, local, remote, timeout=300, ttl=None):
            self.calls.append(("put", name, remote, timeout, ttl))
            return {"path": remote, "size": 1}

        async def get(self, name, remote, to, timeout=300, ttl=None):
            self.calls.append(("get", name, remote, timeout, ttl))
            return to

    def setUp(self):
        self.st = MemoryStorage()
        contacts.add(self.st, "d" * 64, "box")
        self.agent = self.FakeAgent(self.st)
        self.parser = cli.build_parser()
        devnull = open(os.devnull)
        self.addCleanup(devnull.close)
        patcher = mock.patch.object(sys, "stdin", devnull)   # whatever the test runner's stdin holds
        patcher.start()
        self.addCleanup(patcher.stop)

    def run_cli(self, *argv):
        args = self.parser.parse_args(argv)
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            try:
                args.func(self.agent, self.st, args)
            except SystemExit as e:
                self.assertEqual(e.code, 0)
        return self.agent.calls[-1]

    def test_a_timeout_exits_124_for_exec_and_cp(self):
        async def too_late(*args, **kwargs):
            raise TimeoutError("no reply from the worker within 40 seconds")

        self.agent.exec = self.agent.put = self.agent.get = too_late
        for argv in (["exec", "box", "--", "ls"], ["cp", __file__, "box:in.py"], ["cp", "box:out.txt", "out.txt"]):
            args = self.parser.parse_args(argv)
            err = io.StringIO()
            with contextlib.redirect_stderr(err), self.assertRaises(SystemExit) as cm:
                args.func(self.agent, self.st, args)
            self.assertEqual(cm.exception.code, 124, argv)
            self.assertEqual(err.getvalue(), "error: no reply from the worker within 40 seconds\n")

    def test_exec_refuses_input_it_would_drop(self):
        # `myous exec W -- python3 - <<'EOF'` feeds the script to this
        # command, not to the worker: refuse instead of running `python3 -`
        # on nothing.
        args = self.parser.parse_args(["exec", "box", "--", "python3", "-"])
        with mock.patch.object(cli, "stdin_has_data", lambda: True), self.assertRaises(SystemExit) as cm:
            args.func(self.agent, self.st, args)
        self.assertIn("doesn't send its input to the worker", str(cm.exception.code))
        self.assertIn("myous exec \"box\" -- 'python3 - <<\"EOF\"", str(cm.exception.code))
        self.assertEqual(self.agent.calls, [])

    def test_ttl_passes_through(self):
        self.assertEqual(self.run_cli("exec", "--ttl", "3600", "box", "--", "ls"), ("exec", "box", "ls", 120, 3600))
        self.assertEqual(self.run_cli("exec", "box", "--", "ls"), ("exec", "box", "ls", 120, None))
        self.assertEqual(self.run_cli("cp", "--ttl", "900", __file__, "box:in.py"), ("put", "box", "in.py", 300, 900))
        self.assertEqual(self.run_cli("cp", "box:out.txt", "out.txt", "--ttl", "60"), ("get", "box", "out.txt", 300, 60))
        self.assertEqual(self.run_cli("cp", "box:out.txt", "out.txt"), ("get", "box", "out.txt", 300, None))

    def test_options_after_the_contact_are_this_commands(self):
        # Written after the contact, before `--`: the command's options, not part of what runs.
        self.assertEqual(self.run_cli("exec", "box", "--ttl", "3600", "--", "ls"), ("exec", "box", "ls", 120, 3600))
        self.assertEqual(self.run_cli("exec", "box", "--ttl=60", "--timeout", "30", "--", "ls -la"),
                         ("exec", "box", "ls -la", 30.0, 60))
        # Anything else before a `--` belongs to the command.
        self.assertEqual(self.run_cli("exec", "box", "git", "diff", "--", "a.txt"),
                         ("exec", "box", "git diff -- a.txt", 120, None))
        self.assertEqual(self.run_cli("exec", "box", "--ttl", "60", "x", "--", "y"),
                         ("exec", "box", "--ttl 60 x -- y", 120, None))
        self.assertEqual(self.run_cli("exec", "box", "--", "echo", "--ttl", "5", "--", "z"),
                         ("exec", "box", "echo --ttl 5 -- z", 120, None))
        args = self.parser.parse_args(["exec", "box", "--ttl", "0", "--", "ls"])
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as cm:
            args.func(self.agent, self.st, args)
        self.assertIn("at least 1 second", str(cm.exception.code))

    def test_a_ttl_below_a_second_is_refused(self):
        for bad in ("0", "-5", "1.5", "soon"):
            err = io.StringIO()
            with contextlib.redirect_stderr(err), self.assertRaises(SystemExit) as cm:
                self.parser.parse_args(["exec", "--ttl", bad, "box", "--", "ls"])
            self.assertEqual(cm.exception.code, 2)
            self.assertIn("argument --ttl", err.getvalue())

"""Accepting a code: the claim carries an id the client saves first, so a
lost answer, a retry, or running the same accept again after a crash all
get the same claim instead of burning the code."""
import tempfile
import time
import unittest

from myous import pairing
from myous.hub import HubError
from myous.storage import FileStorage

NAMEPLATE = "4821"
CODE = "4821-K7F3QX"


class FakeHub:
    """The hub's claim rules (a repeat with the claimer's id gets the same
    token; anything else after the claim gets 409), plus a mailbox that
    takes our messages and has none for us. `lose` answers are lost after
    the hub has acted on them; `fail` raises before it acts."""

    url = "https://hub.test"

    def __init__(self, lose=0, fail=()):
        self.lose, self.fail = lose, list(fail)
        self.claimed_by, self.claimed_at = None, 1_700_000_000
        self.claims = []  # the ids sent, in order
        self.posts = 0

    def config(self, refresh=False):
        return {"pair_link_base": "https://hub.test/p/"}

    def request(self, method, endpoint, body=None, token=None, timeout=30):
        if endpoint == f"/api/pair/{NAMEPLATE}/claim":
            self.claims.append(body.get("claim"))
            if self.fail:
                raise self.fail.pop(0)
            answer = {"repeat": self.claimed_by is not None}
            if self.claimed_by is None:
                self.claimed_by, self.claimed_at = body.get("claim"), int(time.time())
            elif body.get("claim") != self.claimed_by:
                raise HubError(409, "invite already used, ask for a new one")
            else:
                answer["claimed_at"] = self.claimed_at
            if self.lose:
                self.lose -= 1
                raise ConnectionError("lost the connection to https://hub.test: RemoteDisconnected()")
            return dict(answer, token="tok-" + self.claimed_by, expires_at=int(time.time()) + 900)
        if endpoint == f"/api/pair/{NAMEPLATE}/messages" and method == "POST":
            assert token == "tok-" + self.claimed_by, "posted without the claim's token"
            self.posts += 1
            return {"index": self.posts - 1}
        if endpoint.startswith(f"/api/pair/{NAMEPLATE}/messages?"):
            return {"messages": [], "next": 0}
        raise AssertionError(f"unexpected {method} {endpoint}")


class ClaimTest(unittest.TestCase):
    def setUp(self):
        pairing.CLAIM_RETRY_DELAY = 0
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.st = FileStorage(tmp.name)

    def accept(self, hub):
        return pairing.Pairing(self.st, hub, None, "me").accept(CODE, wait=0)

    def saved_claim(self):
        return (self.st.get(f"claiming/{NAMEPLATE}") or {}).get("claim")

    def test_lost_answer_then_the_retry_gets_the_claim(self):
        hub = FakeHub(lose=1)
        p = self.accept(hub)
        self.assertEqual(p["stage"], "wait_pake")
        self.assertEqual(len(hub.claims), 2)
        self.assertEqual(hub.claims[0], hub.claims[1])
        self.assertRegex(hub.claims[0], r"^[0-9a-f]{32}$")
        self.assertEqual(p["token"], "tok-" + hub.claims[0])
        self.assertEqual(self.st.get(f"pending/{NAMEPLATE}")["token"], p["token"])
        self.assertIsNone(self.saved_claim(), "the claiming record goes once the pairing is saved")

    def test_trace_names_the_drop_and_the_resume(self):
        hub = FakeHub(lose=1)
        trace = self.accept(hub)["trace"]
        self.assertEqual([s["step"] for s in trace], ["claim", "claim", "post"])
        self.assertIn("RemoteDisconnected", trace[0]["error"])
        self.assertEqual(trace[0]["host"], "hub.test")
        self.assertEqual(trace[1]["claimed_at"], hub.claimed_at)
        self.assertNotIn("error", trace[1])
        self.assertEqual(self.st.get(f"pending/{NAMEPLATE}")["trace"], trace, "kept in the pairing's record")

    def test_failed_accept_prints_the_trace(self):
        from myous import cli
        started = int(time.time())
        hub = FakeHub(fail=[ConnectionError("lost: RemoteDisconnected()"),
                            HubError(409, "invite already used, ask for a new one")])
        with self.assertRaises(pairing.PairingError):
            self.accept(hub)
        text = cli.pairing_trace(self.st, started)
        self.assertIn(f"pairing {NAMEPLATE}", text)
        lines = text.splitlines()[1:]
        self.assertEqual(len(lines), 2)
        self.assertIn("claim hub.test: ConnectionError('lost: RemoteDisconnected()')", lines[0])
        self.assertIn("hub error 409: invite already used", lines[1])
        self.assertEqual(cli.pairing_trace(self.st, started + 3600), "", "only this run's trace")

    def test_proxy_error_is_retried_like_a_drop(self):
        hub = FakeHub(fail=[HubError(502, "Bad Gateway")])
        self.assertEqual(self.accept(hub)["stage"], "wait_pake")
        self.assertEqual(len(hub.claims), 2)

    def test_rerun_after_a_crash_reuses_the_saved_id(self):
        # The first run saved its id, the hub took the claim, and the run
        # died before it saw the answer.
        self.st.put(f"claiming/{NAMEPLATE}", {"nameplate": NAMEPLATE, "claim": "ab" * 16})
        hub = FakeHub()
        hub.claimed_by = "ab" * 16
        p = self.accept(hub)
        self.assertEqual(hub.claims, ["ab" * 16])
        self.assertEqual(p["token"], "tok-" + "ab" * 16)
        self.assertIsNone(self.saved_claim())

    def test_hub_unreachable_keeps_the_id_for_the_next_run(self):
        hub = FakeHub(lose=pairing.CLAIM_ATTEMPTS)
        with self.assertRaises(pairing.PairingError) as cm:
            self.accept(hub)
        self.assertIn("could not reach the hub", str(cm.exception))
        self.assertIn("RemoteDisconnected", str(cm.exception), "the drop's own text is in the message")
        self.assertEqual(len(hub.claims), pairing.CLAIM_ATTEMPTS)
        self.assertEqual(self.saved_claim(), hub.claims[0])
        # The same accept again carries on with the claim the hub already gave.
        self.assertEqual(self.accept(hub)["stage"], "wait_pake")
        self.assertEqual(set(hub.claims), {hub.claims[0]})
        self.assertIsNone(self.saved_claim())

    def test_claimed_by_someone_else(self):
        hub = FakeHub()
        hub.claimed_by = "cd" * 16
        with self.assertRaises(pairing.PairingError) as cm:
            self.accept(hub)
        self.assertEqual(str(cm.exception), "invite already used, ask for a new one")
        self.assertEqual(len(hub.claims), 1, "a refusal is not retried")
        self.assertIsNone(self.saved_claim(), "a refusal drops the claiming record")

    def test_refused_after_a_drop_says_so_plainly(self):
        hub = FakeHub(fail=[ConnectionError("lost"), HubError(409, "invite already used, ask for a new one")])
        with self.assertRaises(pairing.PairingError) as cm:
            self.accept(hub)
        self.assertEqual(str(cm.exception), "invite already used, ask for a new one")

    def test_rate_limit_keeps_the_id(self):
        hub = FakeHub(fail=[HubError(429, "too many attempts from this address")])
        with self.assertRaises(pairing.PairingError):
            self.accept(hub)
        self.assertEqual(len(hub.claims), 1)
        self.assertEqual(self.saved_claim(), hub.claims[0])

    def test_each_accept_makes_its_own_id(self):
        ids = []
        for _ in range(2):
            hub = FakeHub()
            self.accept(hub)
            ids += hub.claims
            self.st.delete(f"pending/{NAMEPLATE}")
        self.assertNotEqual(ids[0], ids[1])


if __name__ == "__main__":
    unittest.main()


class Mailbox:
    """A fake hub with both sides of one mailbox, as the hub keeps it: a
    claim id that repeats, and a side's repeat of its last message answered
    without appending. `lose_posts` answers to side B's posts are lost after
    the hub has stored the message."""

    url = "https://hub.test"

    def __init__(self, lose_posts=0):
        self.lose_posts = lose_posts
        self.claimed_by = None
        self.messages = {"a": [], "b": []}

    def config(self, refresh=False):
        return {"pair_link_base": "https://hub.test/p/"}

    def request(self, method, endpoint, body=None, token=None, timeout=30):
        side = {"tok-a": "a", "tok-b": "b"}.get(token)
        if endpoint == "/api/pair":
            return {"nameplate": NAMEPLATE, "token": "tok-a", "expires_at": int(time.time()) + 900}
        if endpoint == f"/api/pair/{NAMEPLATE}/claim":
            if self.claimed_by not in (None, body["claim"]):
                raise HubError(409, "invite already used, ask for a new one")
            repeat, self.claimed_by = self.claimed_by is not None, body["claim"]
            return {"token": "tok-b", "expires_at": int(time.time()) + 900, "repeat": repeat}
        if endpoint == f"/api/pair/{NAMEPLATE}/messages" and method == "POST":
            mine = self.messages[side]
            if mine and mine[-1] == body["body"]:
                return {"index": len(mine) - 1, "repeat": True}
            mine.append(body["body"])
            if side == "b" and self.lose_posts:
                self.lose_posts -= 1
                raise ConnectionError("lost the connection to https://hub.test: RemoteDisconnected()")
            return {"index": len(mine) - 1}
        if endpoint.startswith(f"/api/pair/{NAMEPLATE}/messages?") and method == "GET":
            after = int(endpoint.split("after=")[1].split("&")[0])
            theirs = self.messages["b" if side == "a" else "a"][after:]
            return {"messages": theirs, "next": after + len(theirs)}
        if method == "DELETE":
            return None
        raise AssertionError(f"unexpected {method} {endpoint}")


class PostTest(unittest.TestCase):
    """The PAKE message is saved before it is posted; a lost answer to the
    post means posting the same message again, never a second one."""

    def setUp(self):
        from nostr_sdk import Keys
        pairing.CLAIM_RETRY_DELAY = 0
        dirs = [tempfile.TemporaryDirectory() for _ in range(2)]
        for d in dirs:
            self.addCleanup(d.cleanup)
        self.hub = Mailbox(lose_posts=1)
        self.a = pairing.Pairing(FileStorage(dirs[0].name), self.hub, Keys.generate(), "alice")
        self.b = pairing.Pairing(FileStorage(dirs[1].name), self.hub, Keys.generate(), "bob")

    def test_lost_post_answer_then_rerun_completes_without_a_second_pake(self):
        code = self.a.invite()["code"]
        first = self.b.accept(code, wait=0)  # the post reached the mailbox; its answer didn't
        self.assertEqual(first["stage"], "posting")
        saved = self.b.st.get(f"pending/{NAMEPLATE}")
        self.assertEqual(saved["stage"], "posting")
        self.assertEqual(saved["message"], self.hub.messages["b"][0], "the exact message is saved")

        again = self.b.accept(code, wait=0)  # the same accept, run again
        self.assertEqual(again["stage"], "wait_payload")
        steps = [s["step"] for s in again["trace"]]
        self.assertEqual(steps, ["claim", "post", "post (repeat)"])
        self.assertIn("RemoteDisconnected", again["trace"][1]["error"])

        done_a = self.a.advance_all()
        self.assertEqual([r["stage"] for r in done_a], ["done"], "side A completes")
        done_b = self.b.advance_all()
        self.assertEqual([r["stage"] for r in done_b], ["done"])
        self.assertEqual(done_a[0]["verify"], done_b[0]["verify"])
        pakes = [m for m in self.hub.messages["b"] if b'"pake"' in pairing._unb64(m)]
        self.assertEqual(len(pakes), 1, "one PAKE message from B, not two")

    def test_poll_reposts_a_message_left_unconfirmed(self):
        code = self.a.invite()["code"]
        self.b.accept(code, wait=0)
        self.assertEqual([r["stage"] for r in self.b.advance_all()], [])
        self.assertEqual(self.b.st.get(f"pending/{NAMEPLATE}")["stage"], "wait_payload")
        self.assertEqual(len(self.hub.messages["b"]), 2, "the PAKE once, then the payload")

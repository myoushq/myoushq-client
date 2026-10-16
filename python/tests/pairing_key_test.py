"""The key a pairing pins: the other side proves it holds the key it sends
(a signature over a value only the two sides know), and a pairing never
changes a key that is already a contact: a blocked contact stays blocked,
an approved one keeps its alias and context. Clients from before the proof
send none and still pair."""
import json
import os
import tempfile
import time
import unittest

from nostr_sdk import Keys

from myous import contacts, pairing
from myous.hub import HubError
from myous.storage import FileStorage

NAMEPLATE = "4821"
CONTEXT = {"relationship": "friend", "sharing": "my calendar and home address"}


class Mailbox:
    """A fake hub with both sides of one mailbox."""

    url = "https://hub.test"

    def __init__(self):
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
            self.messages[side].append(body["body"])
            return {"index": len(self.messages[side]) - 1}
        if endpoint.startswith(f"/api/pair/{NAMEPLATE}/messages?") and method == "GET":
            after = int(endpoint.split("after=")[1].split("&")[0])
            theirs = self.messages["b" if side == "a" else "a"][after:]
            return {"messages": theirs, "next": after + len(theirs)}
        if method == "DELETE":
            return None
        raise AssertionError(f"unexpected {method} {endpoint}")


class Peer:
    """Side B written out by hand, so a test can send any payload: what a
    client from before the proof sends, or someone else's key."""

    def __init__(self, hub: Mailbox, code: str):
        self.hub, self.code = hub, code
        self.token = hub.request("POST", f"/api/pair/{NAMEPLATE}/claim", {"claim": "ab" * 16})["token"]
        message, self.state = pairing._pake_start("b", code)
        self.post({"t": "pake", "v": 1, "m": pairing._b64(message)})

    def post(self, msg):
        self.hub.request("POST", f"/api/pair/{NAMEPLATE}/messages", {"body": pairing._body(msg)}, token=self.token)

    def theirs(self):
        got = self.hub.request("GET", f"/api/pair/{NAMEPLATE}/messages?after=0&wait=0", token=self.token)
        return [json.loads(pairing._unb64(m)) for m in got["messages"]]

    def finish(self, make_payload):
        """Read A's PAKE message and payload, then send make_payload(K).
        Returns A's payload."""
        pake_a, sealed = self.theirs()
        self.key = pairing._pake_finish("b", self.state, pairing._unb64(pake_a["m"]))
        self.post(pairing.seal(self.key, "b", NAMEPLATE, make_payload(self.key)))
        return pairing.unseal(self.key, "a", NAMEPLATE, sealed)


class KeyTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.st = FileStorage(tmp.name)
        self.hub = Mailbox()
        self.me = pairing.Pairing(self.st, self.hub, Keys.generate(), "me")
        self.eve, self.bob, self.mallory = Keys.generate(), Keys.generate(), Keys.generate()

    def pair(self, make_payload, context=CONTEXT):
        """Invite with `context`; the peer answers with make_payload(K).
        Returns this side's result and the payload it sent."""
        self.peer = Peer(self.hub, self.me.invite(context)["code"])
        self.assertEqual(self.me.advance_all(), [])  # reads the PAKE, sends our payload
        mine = self.peer.finish(make_payload)
        done = self.me.advance_all()
        self.assertEqual(len(done), 1, done)
        return done[0], mine

    def contact(self, keys):
        return contacts.load(self.st).get(keys.public_key().to_hex())

    def last(self):
        return self.st.read_history()[-1]

    @staticmethod
    def payload(keys, alias, signer=None):
        """A payload naming `keys`, signed by `signer` (None: no proof, as
        clients up to 0.7.5 send)."""
        def make(k):
            p = {"v": 1, "pubkey": keys.public_key().to_hex(), "alias": alias}
            if signer:
                p["sig"] = pairing.sign_proof(signer, k, "b")
            return p
        return make

    def block(self, keys, alias, **fields):
        with self.st.lock():
            contacts.add(self.st, keys.public_key().to_hex(), alias, **fields)
            contacts.update(self.st, alias, status=contacts.BLOCKED)

    def test_this_side_proves_its_key(self):
        r, mine = self.pair(self.payload(self.bob, "bob", self.bob))
        self.assertEqual(r["stage"], "done")
        self.assertEqual(mine["pubkey"], self.me.keys.public_key().to_hex())
        self.assertTrue(pairing.proof_holds(mine["pubkey"], pairing.key_proof(self.peer.key, "a"), mine["sig"]))
        self.assertFalse(pairing.proof_holds(mine["pubkey"], pairing.key_proof(self.peer.key, "b"), mine["sig"]),
                         "a proof is for one side")

    def test_a_client_without_the_proof_still_pairs(self):
        r, _ = self.pair(self.payload(self.bob, "bob"))
        self.assertEqual((r["stage"], r["known"]), ("done", False))
        c = self.contact(self.bob)
        self.assertEqual((c["alias"], c["status"], c["relationship"], c["sharing"]),
                         ("bob", "approved", "friend", "my calendar and home address"))
        self.assertEqual(self.last()["type"], "paired")

    def test_a_proven_new_key_pairs(self):
        r, _ = self.pair(self.payload(self.bob, "bob", self.bob))
        self.assertEqual((r["stage"], r["contact"]["relationship"]), ("done", "friend"))

    def test_a_blocked_contact_stays_blocked(self):
        """The audit's M2: whoever answers names a blocked contact's key."""
        self.block(self.eve, "eve")
        r, _ = self.pair(self.payload(self.eve, "Erin"))
        self.assertEqual(r["stage"], "failed")
        c = self.contact(self.eve)
        self.assertEqual((c["alias"], c["status"], c.get("relationship"), c.get("sharing")), ("eve", "blocked", None, None))
        self.assertEqual(len(contacts.load(self.st)), 1)
        self.assertEqual(self.last()["type"], "pairing_failed")
        self.assertIn('answered with the key of your contact "eve" (blocked), which the other side didn\'t prove it holds; '
                      'nothing changed: if that is right, unblock it (myous unblock "eve")', self.last()["text"])

    def test_a_blocked_contact_that_proves_its_key_stays_blocked(self):
        self.block(self.eve, "eve")
        r, _ = self.pair(self.payload(self.eve, "Erin", self.eve))
        self.assertEqual(r["stage"], "failed")
        self.assertEqual(self.contact(self.eve)["status"], "blocked")
        self.assertIn('answered by your contact "eve" (blocked); nothing changed', self.last()["text"])

    def test_a_proof_by_another_key_fails(self):
        r, _ = self.pair(self.payload(self.eve, "Erin", self.mallory))
        self.assertEqual((r["stage"], r["error"]), ("failed", "the other side didn't prove it holds the key it sent"))
        self.assertEqual(contacts.load(self.st), {})

    def test_a_proof_from_another_pairing_fails(self):
        elsewhere = pairing.sign_proof(self.eve, os.urandom(32), "b")  # eve's, for another K
        r, _ = self.pair(lambda k: {"v": 1, "pubkey": self.eve.public_key().to_hex(), "alias": "Erin", "sig": elsewhere})
        self.assertEqual(r["stage"], "failed")
        self.assertEqual(contacts.load(self.st), {})

    def test_a_malformed_proof_fails(self):
        for sig in ("", "00" * 64, 3, ["x"], "ab"):
            with self.subTest(sig=sig):
                self.setUp()
                r, _ = self.pair(lambda k, sig=sig: {"v": 1, "pubkey": self.bob.public_key().to_hex(), "alias": "bob", "sig": sig})
                self.assertEqual(r["stage"], "failed")
                self.assertEqual(contacts.load(self.st), {})

    def test_an_approved_contact_keeps_its_context(self):
        with self.st.lock():
            contacts.add(self.st, self.bob.public_key().to_hex(), "bob", relationship="colleague", sharing="work only")
        before = self.contact(self.bob)
        r, _ = self.pair(self.payload(self.bob, "Erin"))
        self.assertEqual((r["stage"], r["known"]), ("done", True))
        after = self.contact(self.bob)
        self.assertEqual({k: after[k] for k in before}, before, "alias, status and context as they were")
        self.assertEqual(len(contacts.load(self.st)), 1)
        self.assertEqual(self.last()["type"], "paired")
        self.assertIn("bob was already your contact: nothing changed (the relationship and sharing given with this "
                      'invite were not applied; to change them: myous context "bob"', self.last()["text"])

    def test_keys_written_otherwise_are_refused(self):
        pk = self.bob.public_key()
        for key in (pk.to_hex().upper(), pk.to_bech32(), "ff" * 32, self.me.keys.public_key().to_hex().upper()):
            with self.subTest(key=key):
                self.setUp()
                r, _ = self.pair(lambda k, key=key: {"v": 1, "pubkey": key, "alias": "x"})
                self.assertEqual((r["stage"], r["error"]), ("failed", "the code didn't match (or the exchange was tampered with)"))
                self.assertEqual(contacts.load(self.st), {})

    def test_an_alias_that_names_a_key_gets_no_context(self):
        """The context goes to the new contact by its key, not to a contact
        whose alias happens to be that key."""
        bob_hex = self.bob.public_key().to_hex()
        with self.st.lock():
            contacts.add(self.st, self.mallory.public_key().to_hex(), bob_hex)
        r, _ = self.pair(self.payload(self.bob, "bob", self.bob))
        self.assertEqual(r["stage"], "done")
        self.assertEqual(self.contact(self.bob)["relationship"], "friend")
        self.assertIsNone(self.contact(self.mallory).get("relationship"))

    def test_add_leaves_a_contact_as_it_is(self):
        self.block(self.eve, "eve", relationship="other")
        with self.st.lock():
            c = contacts.add(self.st, self.eve.public_key().to_hex(), "eve again", relationship="family")
        self.assertEqual((c["alias"], c["status"], c["relationship"]), ("eve", "blocked", "other"))


if __name__ == "__main__":
    unittest.main()

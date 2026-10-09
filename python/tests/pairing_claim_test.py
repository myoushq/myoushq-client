"""Accepting a code retries the claim when the connection drops, and says
so when a retry finds the code already claimed."""
import unittest

from myous import pairing
from myous.hub import HubError


class StubHub:
    def __init__(self, answers):
        self.answers = list(answers)
        self.calls = 0

    def request(self, method, endpoint, body=None, **kw):
        self.calls += 1
        a = self.answers.pop(0)
        if isinstance(a, Exception):
            raise a
        return a


class ClaimTest(unittest.TestCase):
    def setUp(self):
        pairing.CLAIM_RETRY_DELAY = 0

    def pairing(self, answers):
        return pairing.Pairing(None, StubHub(answers), None, "me"), None

    def test_dropped_connection_then_success(self):
        p, _ = self.pairing([ConnectionError("lost"), {"token": "t", "expires_at": 1}])
        self.assertEqual(p._claim("1234")["token"], "t")
        self.assertEqual(p.hub.calls, 2)

    def test_hub_refusal_is_not_retried(self):
        p, _ = self.pairing([HubError(404, "no such code")])
        with self.assertRaises(pairing.PairingError) as cm:
            p._claim("1234")
        self.assertEqual(str(cm.exception), "no such code")
        self.assertEqual(p.hub.calls, 1)

    def test_claimed_after_a_drop_explains(self):
        p, _ = self.pairing([ConnectionError("lost"), HubError(409, "already claimed")])
        with self.assertRaises(pairing.PairingError) as cm:
            p._claim("1234")
        self.assertIn("ask for a new code", str(cm.exception))

    def test_gives_up_after_the_attempts(self):
        p, _ = self.pairing([OSError("down")] * pairing.CLAIM_ATTEMPTS)
        with self.assertRaises(pairing.PairingError) as cm:
            p._claim("1234")
        self.assertIn("could not reach the hub", str(cm.exception))
        self.assertEqual(p.hub.calls, pairing.CLAIM_ATTEMPTS)


if __name__ == "__main__":
    unittest.main()

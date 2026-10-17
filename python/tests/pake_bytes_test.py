"""SPAKE2's bytes, pinned. pairing._pake_start and _pake_finish, with fixed
randomness in place of os.urandom, give these messages, saved states and
keys for the protocol's ids (myous-pair-a, myous-pair-b). spake2 0.9 (every
platform but Intel Macs) and 0.8 (Intel Macs; RELEASING.md, "The Intel Mac
path") must both give them, so CI's python and python-intel-mac-set jobs
check the same bytes. Recorded 2026-10-17 with spake2 0.9 and checked
identical with 0.8."""
import hashlib
import unittest
from unittest import mock

from myous import pairing

# A saved state as spake2 serializes it (pending/<nameplate> holds it
# between runs): side, the password's hex and the secret scalar vary.
STATE = ('{"hashed_params": "8cdd8eb1abd98ba5c45665970f50a726c33d0ea6001daeef354dfb8afa299129", "side": "%s", '
         '"idA": "6d796f75732d706169722d61", "idB": "6d796f75732d706169722d62", '
         '"password": "%s", "xy_scalar": "%s"}')

CASES = [
    {"code": "4821-K7F3QX",
     "msg_a": "4174da7a8d6f38ba71336ac76f8e81de2514047faa0c6467b978bdfd62adf39729",
     "xy_a": "114cc702ef45a77da0b37815b1a2ed3e35341a65eb7cefffe6317492ceced80a",
     "msg_b": "42c44a6869270e3b676649c528f92102db0186ccf669879e074c167b04b209fd4a",
     "xy_b": "9e007ca461a0594264b0a7cade1e8afad4e6bf557ed4afd203adc316433f3c04",
     "key": "6294b235bc4db4c05efaf5227434b5eba51da03d702fa39f6ec97014ac2be13f"},
    {"code": "7-000000",
     "msg_a": "41f85f474591715963bdcc32a805a6f206a3aee691439c7e5a7ef0975f4949165c",
     "xy_a": "50fb3812af645ac7598161189c2da1676359917866679a97e17b6e4784378d09",
     "msg_b": "42621a1c3466bee10617bdcecc3696c469fe0127e1af6b2914a590e4634777f4d4",
     "xy_b": "22943b7ed9aeb208d3165783d31a17086569e395d4c32d2e57bc7d51d6d83f09",
     "key": "14fc275b77d978b462fa91fc79bc120b34b845e2ffb7f1bcb23610d4f05c4406"},
    {"code": "60459172-ZZZZZZ",
     "msg_a": "41a909d581a2c9e5dce109a9097ae9968c7932bd59d14ba243250833442f2ec966",
     "xy_a": "e781476eb45038a1bb1b7f354f3d87e13435634520c3075fb18beb8777d60d07",
     "msg_b": "427ab0a42be97791bdf5fb13e90cd1d074a317baa9d60a090b439e691704e08985",
     "xy_b": "ec9e5994d5e7b8f8ea369dda0b36adeb44f19bc733e382021f2c4d188f0d990a",
     "key": "1a28a262bce9ba338f7936d1de495e4a880d9372acd0bbf0c48ac20fbb6242ad"},
]


def entropy(seed: bytes):
    """Deterministic bytes for entropy_f: SHA-256 of the seed and a counter."""
    counter = 0

    def read(n: int) -> bytes:
        nonlocal counter
        out = b""
        while len(out) < n:
            out += hashlib.sha256(b"%s %d" % (seed, counter)).digest()
            counter += 1
        return out[:n]
    return read


def fixed(cls, seed: bytes):
    """The spake2 class with fixed randomness (from_serialized passes its own)."""
    class Fixed(cls):
        def __init__(self, *args, **kwargs):
            kwargs.setdefault("entropy_f", entropy(seed))
            super().__init__(*args, **kwargs)
    return Fixed


class PakeBytes(unittest.TestCase):
    def start(self, role: str, code: str) -> tuple[bytes, bytes]:
        with mock.patch.object(pairing, "SPAKE2_A", fixed(pairing.SPAKE2_A, b"a " + code.encode())), \
             mock.patch.object(pairing, "SPAKE2_B", fixed(pairing.SPAKE2_B, b"b " + code.encode())):
            return pairing._pake_start(role, code)

    def test_messages_states_and_keys(self):
        for case in CASES:
            with self.subTest(code=case["code"]):
                pw = case["code"].encode().hex()
                msg_a, state_a = self.start("a", case["code"])
                msg_b, state_b = self.start("b", case["code"])
                self.assertEqual(msg_a.hex(), case["msg_a"])
                self.assertEqual(msg_b.hex(), case["msg_b"])
                self.assertEqual(state_a.decode(), STATE % ("A", pw, case["xy_a"]))
                self.assertEqual(state_b.decode(), STATE % ("B", pw, case["xy_b"]))
                self.assertEqual(pairing._pake_finish("a", state_a, msg_b).hex(), case["key"])
                self.assertEqual(pairing._pake_finish("b", state_b, msg_a).hex(), case["key"])

    def test_a_recorded_state_finishes(self):
        # A pairing saved by one spake2 version finishes in the other (an
        # Intel Mac that upgrades from 0.9 to 0.8 halfway through a pairing).
        for case in CASES:
            with self.subTest(code=case["code"]):
                pw = case["code"].encode().hex()
                state_a = (STATE % ("A", pw, case["xy_a"])).encode()
                state_b = (STATE % ("B", pw, case["xy_b"])).encode()
                self.assertEqual(pairing._pake_finish("a", state_a, bytes.fromhex(case["msg_b"])).hex(), case["key"])
                self.assertEqual(pairing._pake_finish("b", state_b, bytes.fromhex(case["msg_a"])).hex(), case["key"])


if __name__ == "__main__":
    unittest.main()

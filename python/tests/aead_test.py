"""The two ciphers fail closed, with the errors their callers catch: a file
(AES-256-GCM) raises ValueError, a pairing payload (ChaCha20-Poly1305)
PairingError. Only the sizes the protocol uses are taken: a nonce of
another length, or a ciphertext shorter than its tag, is refused like a
bad tag. The backend (myous/_aead.py) is cryptography wherever it is
installed and pycryptodome only without it; with both installed, the two
must give the same bytes."""
import base64
import importlib.util
import json
import os
import random
import sys
import unittest
from pathlib import Path

from myous import _aead, files, pairing

K = bytes(range(32))
NAMEPLATE = "4821"
PAYLOAD = {"v": 1, "pubkey": "e8fc748bfd057ea4fdd41d0b835ae420c4410be8209f79cf35ef7774c6bb055d", "alias": "Sam"}


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


class FileCipher(unittest.TestCase):
    def setUp(self):
        self.enc = files.encrypt(b"hello")

    def refused(self, ciphertext, key=None, nonce=None):
        with self.assertRaisesRegex(ValueError, "could not be decrypted"):
            files.decrypt(ciphertext, key or self.enc.key, nonce or self.enc.nonce)

    def test_the_tag_is_appended(self):
        self.assertEqual(len(self.enc.ciphertext), len(b"hello") + files.TAG_BYTES)
        empty = files.encrypt(b"")
        self.assertEqual(len(empty.ciphertext), files.TAG_BYTES)
        self.assertEqual(files.decrypt(empty.ciphertext, empty.key, empty.nonce), b"")

    def test_an_altered_tag_is_refused(self):
        c = self.enc.ciphertext
        self.refused(c[:-1] + bytes([c[-1] ^ 1]))

    def test_a_cut_ciphertext_is_refused(self):
        for n in (0, 1, files.TAG_BYTES - 1, files.TAG_BYTES, len(self.enc.ciphertext) - 1):
            self.refused(self.enc.ciphertext[:n])

    def test_the_wrong_key_or_nonce_is_refused(self):
        self.refused(self.enc.ciphertext, key=bytes(32))
        self.refused(self.enc.ciphertext, nonce=bytes(12))

    def sealed(self, key: bytes, nonce: bytes) -> bytes:
        """A valid AES-GCM payload of b"hello", made with the backend
        directly, which also opens it: only the guard in decrypt refuses it."""
        sealed = _aead.aes_gcm_seal(key, nonce, b"hello")
        self.assertEqual(_aead.aes_gcm_open(key, nonce, sealed), b"hello")
        return sealed

    def test_an_8_byte_nonce_is_refused(self):
        self.refused(self.sealed(self.enc.key, bytes(8)), nonce=bytes(8))

    def test_a_24_byte_nonce_is_refused(self):
        self.refused(self.sealed(self.enc.key, bytes(24)), nonce=bytes(24))

    def test_a_16_byte_key_is_refused(self):
        key = self.enc.key[:16]  # AES-128: the library takes it, the protocol doesn't
        self.refused(self.sealed(key, self.enc.nonce), key=key)

    def test_other_sizes_are_refused(self):
        self.refused(self.enc.ciphertext, nonce=self.enc.nonce + bytes(4))
        self.refused(self.enc.ciphertext, nonce=self.enc.nonce[:8])
        self.refused(self.enc.ciphertext, key=self.enc.key[:16])
        with self.assertRaises(ValueError):
            files.encrypt(b"hello", self.enc.key, self.enc.nonce[:8])


class PayloadCipher(unittest.TestCase):
    def setUp(self):
        self.msg = pairing.seal(K, "a", NAMEPLATE, PAYLOAD, nonce=bytes(range(12)))

    def refused(self, msg, nameplate=NAMEPLATE, role="a"):
        with self.assertRaises(pairing.PairingError):
            pairing.unseal(K, role, nameplate, msg)

    def test_a_sealed_payload_opens(self):
        self.assertEqual(pairing.unseal(K, "a", NAMEPLATE, self.msg), PAYLOAD)
        fresh = pairing.seal(K, "b", NAMEPLATE, PAYLOAD)
        self.assertEqual(len(base64.b64decode(fresh["n"])), 12)
        self.assertEqual(pairing.unseal(K, "b", NAMEPLATE, fresh), PAYLOAD)

    def test_another_side_or_nameplate_is_refused(self):
        self.refused(self.msg, role="b")
        self.refused(self.msg, nameplate="4822")

    def test_an_altered_or_cut_ciphertext_is_refused(self):
        c = base64.b64decode(self.msg["c"])
        self.refused({**self.msg, "c": b64(c[:-1] + bytes([c[-1] ^ 1]))})
        for n in (0, 1, pairing.TAG_LEN - 1, pairing.TAG_LEN, len(c) - 1):
            self.refused({**self.msg, "c": b64(c[:n])})

    def test_only_a_12_byte_nonce_is_taken(self):
        # pycryptodome reads 8 and 24 bytes as other ChaCha20 variants.
        n = base64.b64decode(self.msg["n"])
        for nonce in (n[:8], n + bytes(12), b"", n[:11], n + b"\0"):
            self.refused({**self.msg, "n": b64(nonce)})
        with self.assertRaises(ValueError):
            pairing.seal(K, "a", NAMEPLATE, PAYLOAD, nonce=bytes(24))

    def sealed_with(self, nonce: bytes) -> dict:
        """A valid payload message with this nonce, made with pycryptodome,
        which opens 8 bytes as the original ChaCha20 and 24 as XChaCha20:
        only the guard in unseal refuses it. cryptography makes no such
        payload (it refuses those nonces itself)."""
        try:
            from Crypto.Cipher import ChaCha20_Poly1305
        except ImportError:
            self.skipTest("pycryptodome isn't installed; cryptography refuses these nonces itself")

        def cipher():
            c = ChaCha20_Poly1305.new(key=pairing.derive(K, "from a"), nonce=nonce)
            c.update(NAMEPLATE.encode())
            return c
        plain = json.dumps(PAYLOAD, sort_keys=True).encode()
        body, tag = cipher().encrypt_and_digest(plain)
        self.assertEqual(cipher().decrypt_and_verify(body, tag), plain)
        return {"t": "payload", "v": 1, "n": b64(nonce), "c": b64(body + tag)}

    def test_an_8_byte_nonce_is_refused(self):
        self.refused(self.sealed_with(bytes(8)))

    def test_a_24_byte_nonce_is_refused(self):
        self.refused(self.sealed_with(bytes(24)))

    def test_a_malformed_message_is_refused(self):
        self.refused({"t": "payload", "v": 1})
        self.refused({**self.msg, "n": "not base64!"})


class Backend(unittest.TestCase):
    def test_cryptography_wherever_it_is_installed(self):
        installed = importlib.util.find_spec("cryptography") is not None
        self.assertEqual(_aead.BACKEND, "cryptography" if installed else "pycryptodome")


def backend_without(library: str):
    """A fresh copy of myous/_aead.py loaded as if `library` weren't
    installed (None in sys.modules stops an import), so a process with both
    has each backend. Modules the copy imports stay loaded."""
    ours = lambda name: name == library or name.startswith(library + ".")  # noqa: E731
    saved = {name: m for name, m in sys.modules.items() if ours(name)}
    sys.modules.update({name: None for name in saved}, **{library: None})
    try:
        spec = importlib.util.spec_from_file_location(f"_aead_without_{library}", _aead.__file__)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    finally:
        for name in [n for n, m in sys.modules.items() if ours(n) and m is None]:
            del sys.modules[name]
        sys.modules.update(saved)
    return module


@unittest.skipUnless(importlib.util.find_spec("cryptography") and importlib.util.find_spec("Crypto"),
                     "needs both cryptography and pycryptodome")
class BothBackends(unittest.TestCase):
    """The two backends give the same bytes and open each other's."""

    @classmethod
    def setUpClass(cls):
        cls.a, cls.b = backend_without("Crypto"), backend_without("cryptography")
        assert (cls.a.BACKEND, cls.b.BACKEND) == ("cryptography", "pycryptodome")

    def test_the_vectors(self):
        vectors = json.loads((Path(__file__).resolve().parents[2] / "docs" / "test-vectors.json").read_text())
        kd, ps = vectors["key_derivation"], vectors["payload_seal"]
        k = bytes.fromhex(kd["K_hex"])
        for m in (self.a, self.b):
            for v in vectors["files"]:
                key, nonce = bytes.fromhex(v["key"]), bytes.fromhex(v["nonce"])
                self.assertEqual(m.aes_gcm_seal(key, nonce, bytes.fromhex(v["plaintext_hex"])).hex(), v["ciphertext_hex"])
            self.assertEqual(m.hkdf_sha256(k, 32, b"myous pairing v1 from a").hex(), kd["from_a_hex"])
            self.assertEqual(m.hkdf_sha256(k, 8, b"myous pairing v1 verify").hex(), kd["verify_bytes_hex"])
            sealed = m.chacha20_poly1305_seal(m.hkdf_sha256(bytes.fromhex(ps["K_hex"]), 32, b"myous pairing v1 from a"),
                                              bytes.fromhex(ps["nonce_hex"]), ps["nameplate"].encode(), ps["plaintext"].encode())
            self.assertEqual(b64(sealed), ps["message"]["c"])

    def test_random_cases(self):
        rnd = random.Random(2026)
        for _ in range(200):
            key, nonce, aad = os.urandom(32), os.urandom(12), os.urandom(rnd.randrange(0, 40))
            plain = os.urandom(rnd.choice([0, 1, 15, 16, 17, 1000, rnd.randrange(0, 70000)]))
            gcm = self.a.aes_gcm_seal(key, nonce, plain)
            self.assertEqual(self.b.aes_gcm_seal(key, nonce, plain), gcm)
            self.assertEqual(self.b.aes_gcm_open(key, nonce, gcm), plain)
            chacha = self.a.chacha20_poly1305_seal(key, nonce, aad, plain)
            self.assertEqual(self.b.chacha20_poly1305_seal(key, nonce, aad, plain), chacha)
            self.assertEqual(self.b.chacha20_poly1305_open(key, nonce, aad, chacha), plain)
            self.assertEqual(self.a.chacha20_poly1305_open(key, nonce, aad, chacha), plain)
            ikm, info, length = os.urandom(rnd.choice([0, 32, 64])), os.urandom(rnd.randrange(0, 40)), rnd.choice([1, 8, 32, 48, 255 * 32])
            self.assertEqual(self.a.hkdf_sha256(ikm, length, info), self.b.hkdf_sha256(ikm, length, info))
            bad = bytes([gcm[-1] ^ 1])
            for m in (self.a, self.b):
                with self.assertRaises(Exception):
                    m.aes_gcm_open(key, nonce, gcm[:-1] + bad)


if __name__ == "__main__":
    unittest.main()

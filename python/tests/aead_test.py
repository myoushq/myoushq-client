"""The two ciphers fail closed, with the errors their callers catch: a file
(AES-256-GCM) raises ValueError, a pairing payload (ChaCha20-Poly1305)
PairingError. Only the sizes the protocol uses are taken: a nonce of
another length, or a ciphertext shorter than its tag, is refused like a
bad tag."""
import base64
import json
import unittest

from Crypto.Cipher import AES, ChaCha20_Poly1305

from myous import files, pairing

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
        """A valid AES-GCM payload of b"hello", made with the library
        directly, which also opens it: only the guard in decrypt refuses it."""
        body, tag = AES.new(key, AES.MODE_GCM, nonce=nonce, mac_len=16).encrypt_and_digest(b"hello")
        self.assertEqual(AES.new(key, AES.MODE_GCM, nonce=nonce, mac_len=16).decrypt_and_verify(body, tag), b"hello")
        return body + tag

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
        """A valid payload message with this nonce, made with the library
        directly, which opens 8 bytes as the original ChaCha20 and 24 as
        XChaCha20: only the guard in unseal refuses it."""
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


if __name__ == "__main__":
    unittest.main()

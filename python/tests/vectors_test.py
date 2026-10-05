"""The reference client must reproduce the published test vectors."""
import json
import unittest
from pathlib import Path

from myous import pairing

VECTORS = json.loads((Path(__file__).resolve().parents[2] / "docs" / "test-vectors.json").read_text())
LINK_BASE = "https://myoushq.com/p/"


class Vectors(unittest.TestCase):
    def test_code_parsing(self):
        for v in VECTORS["code_parsing"]:
            self.assertEqual(pairing.parse_code(v["input"], LINK_BASE), (v["nameplate"], v["secret"]))
            self.assertEqual(pairing.format_code(v["nameplate"], v["secret"]), v["password"])

    def test_key_derivation(self):
        v = VECTORS["key_derivation"]
        k = bytes.fromhex(v["K_hex"])
        self.assertEqual(pairing.derive(k, "from a").hex(), v["from_a_hex"])
        self.assertEqual(pairing.derive(k, "from b").hex(), v["from_b_hex"])
        self.assertEqual(pairing.derive(k, "verify", 8).hex(), v["verify_bytes_hex"])
        self.assertEqual(pairing.verify_code(k), v["verify_code"])

    def test_payload_seal(self):
        v = VECTORS["payload_seal"]
        k = bytes.fromhex(v["K_hex"])
        payload = json.loads(v["plaintext"])
        sealed = pairing.seal(k, v["role"], v["nameplate"], payload, nonce=bytes.fromhex(v["nonce_hex"]))
        self.assertEqual(sealed, v["message"])
        self.assertEqual(pairing.unseal(k, v["role"], v["nameplate"], sealed), payload)


if __name__ == "__main__":
    unittest.main()

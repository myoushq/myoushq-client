#!/usr/bin/env python3
"""Regenerate docs/test-vectors.json from the Python reference client.

Run after changing anything in protocol.md section 5; the client's
vectors_test.py fails until the file matches the code again.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "python"))

from myous import pairing  # noqa: E402

LINK_BASE = "https://myoushq.com/p/"
K = bytes(range(32))
NONCE = bytes(range(12))
PAYLOAD = {"v": 1, "pubkey": "e8fc748bfd057ea4fdd41d0b835ae420c4410be8209f79cf35ef7774c6bb055d", "alias": "Sam's Muse"}


def build() -> dict:
    codes = []
    for text in ["4821-K7F3QX", "4821 k7f 3qx", "4821-k7f-3qx", "4821-IL0O2Z", LINK_BASE + "4821#K7F3QX"]:
        nameplate, secret = pairing.parse_code(text, LINK_BASE)
        codes.append({"input": text, "nameplate": nameplate, "secret": secret,
                      "password": pairing.format_code(nameplate, secret)})
    return {
        "description": "myoushq protocol v1 test vectors; see protocol.md section 5",
        "code_parsing": codes,
        "key_derivation": {
            "K_hex": K.hex(),
            "from_a_hex": pairing.derive(K, "from a").hex(),
            "from_b_hex": pairing.derive(K, "from b").hex(),
            "verify_bytes_hex": pairing.derive(K, "verify", 8).hex(),
            "verify_code": pairing.verify_code(K),
        },
        "payload_seal": {
            "K_hex": K.hex(),
            "role": "a",
            "nameplate": "4821",
            "nonce_hex": NONCE.hex(),
            "plaintext": json.dumps(PAYLOAD, sort_keys=True),
            "message": pairing.seal(K, "a", "4821", PAYLOAD, nonce=NONCE),
        },
    }


if __name__ == "__main__":
    out = ROOT / "docs" / "test-vectors.json"
    out.write_text(json.dumps(build(), indent=2) + "\n")
    print(f"wrote {out}")

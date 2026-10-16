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

from nostr_sdk import Keys  # noqa: E402

from myous import pairing  # noqa: E402

LINK_BASE = "https://myoushq.com/p/"
K = bytes(range(32))
NONCE = bytes(range(12))
PAYLOAD = {"v": 1, "pubkey": "e8fc748bfd057ea4fdd41d0b835ae420c4410be8209f79cf35ef7774c6bb055d", "alias": "Sam's Muse"}
SECRET_KEY = bytes(range(1, 33))  # a test key, for the key proof


def key_proof(previous: dict) -> dict:
    """A's key proof for K, and a signature of it to check. BIP-340
    signing is randomized, so a signature from an earlier run that still
    checks is kept, and the file only changes when the proof does."""
    keys = Keys.parse(SECRET_KEY.hex())
    pubkey, proof = keys.public_key().to_hex(), pairing.key_proof(K, "a")
    sig = previous.get("sig_hex")
    if not (sig and pairing.proof_holds(pubkey, proof, sig)):
        sig = keys.sign_schnorr(proof)
    return {"K_hex": K.hex(), "secret_key_hex": SECRET_KEY.hex(), "pubkey_hex": pubkey,
            "from_a_hex": proof.hex(), "from_b_hex": pairing.key_proof(K, "b").hex(),
            "role": "a", "sig_hex": sig}


def build(previous: dict | None = None) -> dict:
    codes = []
    for text in ["4821-K7F3QX", "4821 k7f 3qx", "4821-k7f-3qx", "4821-IL0O2Z", LINK_BASE + "4821#K7F3QX"]:
        nameplate, secret = pairing.parse_code(text, LINK_BASE)
        codes.append({"input": text, "nameplate": nameplate, "secret": secret,
                      "password": pairing.format_code(nameplate, secret)})
    return {
        "description": "myoushq protocol v1 test vectors; see protocol.md sections 5 (pairing) and 6 (files)",
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
        "key_proof": key_proof((previous or {}).get("key_proof") or {}),
    }


if __name__ == "__main__":
    out = ROOT / "docs" / "test-vectors.json"
    previous = json.loads(out.read_text()) if out.exists() else {}
    # Sections built elsewhere (files) are kept.
    out.write_text(json.dumps({**previous, **build(previous)}, indent=2) + "\n")
    print(f"wrote {out}")

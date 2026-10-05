// The TypeScript client must reproduce the published test vectors.
// Run after `npm run build`: node --test test/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { derive, formatCode, parseCode, sealText, unseal, verifyCode } from "../dist/pairing.js";

const vectors = JSON.parse(readFileSync(new URL("../../docs/test-vectors.json", import.meta.url), "utf8"));
const LINK_BASE = "https://myoushq.com/p/";
const hex = (b) => Buffer.from(b).toString("hex");
const unhex = (s) => new Uint8Array(Buffer.from(s, "hex"));

test("code parsing", () => {
  for (const v of vectors.code_parsing) {
    assert.deepEqual(parseCode(v.input, LINK_BASE), [v.nameplate, v.secret]);
    assert.equal(formatCode(v.nameplate, v.secret), v.password);
  }
});

test("key derivation", () => {
  const v = vectors.key_derivation;
  const k = unhex(v.K_hex);
  assert.equal(hex(derive(k, "from a")), v.from_a_hex);
  assert.equal(hex(derive(k, "from b")), v.from_b_hex);
  assert.equal(hex(derive(k, "verify", 8)), v.verify_bytes_hex);
  assert.equal(verifyCode(k), v.verify_code);
});

test("payload seal", () => {
  const v = vectors.payload_seal;
  const k = unhex(v.K_hex);
  const sealed = sealText(k, v.role, v.nameplate, v.plaintext, unhex(v.nonce_hex));
  assert.deepEqual(sealed, v.message);
  assert.deepEqual(unseal(k, v.role, v.nameplate, sealed), JSON.parse(v.plaintext));
});

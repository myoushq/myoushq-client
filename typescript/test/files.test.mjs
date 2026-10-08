// File encryption, file-message parsing and blob authorization (protocol
// section 6). Run after `npm run build`: node --test test/
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { generateSecretKey, getPublicKey, verifyEvent } from "nostr-tools/pure";
import { blobAuth, decryptFile, encryptFile, fileTags, parseFileTags, sanitizeName, sha256hex } from "../dist/files.js";

const vectors = JSON.parse(readFileSync(new URL("../../docs/test-vectors.json", import.meta.url), "utf8"));
const unhex = (s) => new Uint8Array(Buffer.from(s, "hex"));
const hex = (b) => Buffer.from(b).toString("hex");

test("file encryption round trip", async () => {
  const plaintext = new TextEncoder().encode("hello, worker");
  const enc = await encryptFile(plaintext);
  assert.equal(enc.key.length, 32);
  assert.equal(enc.nonce.length, 12);
  assert.equal(enc.ciphertext.length, plaintext.length + 16);
  assert.equal(enc.x, sha256hex(enc.ciphertext));
  assert.equal(enc.ox, sha256hex(plaintext));
  assert.deepEqual(await decryptFile(enc.ciphertext, enc.key, enc.nonce, enc.x, enc.ox), plaintext);
});

test("tampering is detected", async () => {
  const enc = await encryptFile(new TextEncoder().encode("payload"));
  const flipped = new Uint8Array(enc.ciphertext);
  flipped[0] ^= 1;
  await assert.rejects(decryptFile(flipped, enc.key, enc.nonce, enc.x, enc.ox), /x\)/);
  await assert.rejects(decryptFile(flipped, enc.key, enc.nonce), /tampered/);
  await assert.rejects(decryptFile(enc.ciphertext, enc.key, enc.nonce, enc.x, "0".repeat(64)), /ox\)/);
  const wrongKey = new Uint8Array(32);
  await assert.rejects(decryptFile(enc.ciphertext, wrongKey, enc.nonce), /tampered/);
});

test("published file vectors", async (t) => {
  if (!vectors.files) {
    t.skip("docs/test-vectors.json has no files vectors yet");
    return;
  }
  for (const v of vectors.files) {
    const enc = await encryptFile(unhex(v.plaintext_hex), unhex(v.key), unhex(v.nonce));
    assert.equal(hex(enc.ciphertext), v.ciphertext_hex);
    assert.equal(enc.x, v.x);
    assert.equal(enc.ox, v.ox);
    assert.equal(hex(await decryptFile(unhex(v.ciphertext_hex), unhex(v.key), unhex(v.nonce), v.x, v.ox)), v.plaintext_hex);
  }
});

test("file names are names only", () => {
  assert.equal(sanitizeName("report.pdf"), "report.pdf");
  assert.equal(sanitizeName("/etc/passwd"), "passwd");
  assert.equal(sanitizeName("..\\..\\x.txt"), "x.txt");
  assert.equal(sanitizeName(""), null);
  assert.equal(sanitizeName("."), null);
  assert.equal(sanitizeName("dir/.."), null);
  assert.equal(sanitizeName("bad\nname"), null);
});

test("file message tags round trip and reject foreign blobs", () => {
  const info = {
    name: "a.txt", mime: "text/plain", size: 21, x: "a".repeat(64), ox: "b".repeat(64), key: "c".repeat(64), nonce: "d".repeat(24),
  };
  const api = "https://myoushq.com/blob";
  const tags = [["p", "e".repeat(64)], ["ms", "1"], ...fileTags(info), ["w", "put", "f".repeat(32), "in/a.txt"]];
  const parsed = parseFileTags(tags, `${api}/${info.x}`, api);
  assert.deepEqual(parsed, { ...info, url: `${api}/${info.x}`, w: ["put", "f".repeat(32), "in/a.txt"] });
  assert.equal(parseFileTags(tags, `https://evil.example/blob/${info.x}`, api), null);
  assert.equal(parseFileTags(tags, `${api}/${"9".repeat(64)}`, api), null, "url must name x");
  assert.equal(parseFileTags(tags.filter((t) => t[0] !== "decryption-key"), `${api}/${info.x}`, api), null);
  const badName = tags.map((t) => (t[0] === "name" ? ["name", ".."] : t));
  assert.equal(parseFileTags(badName, `${api}/${info.x}`, api), null);
  const nested = tags.map((t) => (t[0] === "name" ? ["name", "../../x.txt"] : t));
  assert.equal(parseFileTags(nested, `${api}/${info.x}`, api).name, "x.txt");
});

test("blob authorization header", () => {
  const sk = generateSecretKey();
  const now = 1_800_000_000;
  const header = blobAuth(sk, "upload", "a".repeat(64), now);
  assert.match(header, /^Nostr [A-Za-z0-9_-]+$/, "base64url without padding");
  const event = JSON.parse(Buffer.from(header.slice(6), "base64url").toString("utf8"));
  assert.ok(verifyEvent(event));
  assert.equal(event.pubkey, getPublicKey(sk));
  assert.equal(event.kind, 24242);
  assert.equal(event.created_at, now);
  assert.deepEqual(event.tags, [["t", "upload"], ["x", "a".repeat(64)], ["expiration", String(now + 300)]]);
});

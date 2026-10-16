// A gift wrap's rumor is JSON its sender wrote, and anyone registered can
// send us one: unwrapping happens before the sender is looked up. A rumor
// of the wrong shape is dropped like any invalid message; it must not
// throw, which lost every message fetched with it (the batch is already
// marked seen) and ended the listener. Offline. Run after `npm run build`.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generateSecretKey, getPublicKey } from "nostr-tools/pure";
import { createSeal, createWrap, wrapEvent } from "nostr-tools/nip59";
import { Agent } from "../dist/agent.js";
import * as contacts from "../dist/contacts.js";
import { KIND_CHAT, unwrap } from "../dist/relay.js";
import { FileStorage } from "../dist/storage.js";

let tmp;
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-rumor-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

/** A wrap whose seal is valid and signed by `key`, around any JSON as the rumor. */
function wrapRaw(rumor, key, to) {
  return createWrap(createSeal(rumor, key, to), to);
}

test("rumors of the wrong shape are dropped, and the messages that came with them are kept", async () => {
  const st = new FileStorage(join(tmp, "receiver"));
  const agent = await Agent.open(st, "http://127.0.0.1:1");
  await agent.createIdentity();
  const me = await agent.pubkey();
  const sk = await agent.key();
  const peerKey = generateSecretKey(), strangerKey = generateSecretKey();
  const peer = getPublicKey(peerKey), stranger = getPublicKey(strangerKey);
  await contacts.add(st, peer, "peer");
  const now = Math.floor(Date.now() / 1000);
  const bad = [
    wrapRaw({ pubkey: stranger, created_at: now, kind: KIND_CHAT, content: "x" }, strangerKey, me),
    wrapRaw({ pubkey: stranger, created_at: now, kind: KIND_CHAT, content: "x", tags: [null] }, strangerKey, me),
    wrapRaw({ pubkey: peer, created_at: now, kind: KIND_CHAT, content: 5, tags: [] }, peerKey, me),
    wrapRaw({ pubkey: peer, created_at: "soon", kind: KIND_CHAT, content: "x", tags: [] }, peerKey, me),
    wrapRaw({ pubkey: peer, created_at: now, kind: KIND_CHAT, content: "x", tags: [["ms", 5]] }, peerKey, me),
  ];
  for (const w of bad) assert.equal(unwrap(sk, w), null);
  const good = wrapEvent({ kind: KIND_CHAT, content: "the meeting moved to 3pm", tags: [["p", me]], created_at: now }, peerKey, me);
  const stored = await agent.handleWraps([...bad, good]); // private in TypeScript; the lock is irrelevant here
  assert.deepEqual(stored.map((e) => [e.alias, e.text]), [["peer", "the meeting moved to 3pm"]]);
});

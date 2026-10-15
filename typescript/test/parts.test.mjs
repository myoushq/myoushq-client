// Long messages (protocol, "Long messages"): a listener hands each gift
// wrap over on its own, so every part must be saved as it comes, or the
// message is lost once its wraps count as seen. Offline. Run after
// `npm run build`: node --test test/
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { generateSecretKey, getPublicKey } from "nostr-tools/pure";
import { wrapEvent } from "nostr-tools/nip59";
import { Agent } from "../dist/agent.js";
import * as contacts from "../dist/contacts.js";
import { KIND_CHAT } from "../dist/relay.js";
import { FileStorage } from "../dist/storage.js";

let tmp;
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-parts-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

test("a long message's parts arriving in separate deliveries are each saved, then joined", async () => {
  const st = new FileStorage(join(tmp, "receiver"));
  const agent = await Agent.open(st, "http://127.0.0.1:1");
  await agent.createIdentity();
  const me = await agent.pubkey();
  const peerKey = generateSecretKey();
  const peer = getPublicKey(peerKey);
  await contacts.add(st, peer, "peer");
  const texts = ["first ", "second ", "third"];
  for (const [i, text] of texts.entries()) {
    const tags = [["p", me], ["part", "ab12", String(i + 1), "3"]];
    const w = wrapEvent({ kind: KIND_CHAT, content: text, tags, created_at: Math.floor(Date.now() / 1000) }, peerKey, me);
    const stored = await agent.handleWraps([w]); // private in TypeScript; the lock is irrelevant here
    if (i < 2) {
      assert.deepEqual(stored, []);
      const buf = await st.get("partials", {});
      assert.equal(Object.keys(buf[`${peer}:ab12`].parts).length, i + 1, `part ${i + 1} not saved`);
    } else {
      assert.equal(stored.length, 1);
      assert.equal(stored[0].text, "first second third");
      assert.equal(stored[0].incomplete, undefined);
    }
  }
});

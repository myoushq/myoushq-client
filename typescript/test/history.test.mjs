// The history's numbering (protocol section 4): the next seq is written
// before the entry, so a state write that fails (a full disk) loses that
// message rather than giving its seq again to the next one, which hid one
// of the two from the inbox. Offline. Run after `npm run build`: node --test test/
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
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-history-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

const wrap = (sender, me, text) =>
  wrapEvent({ kind: KIND_CHAT, content: text, tags: [["p", me]], created_at: Math.floor(Date.now() / 1000) }, sender, me);

test("a failed state write gives no seq twice", async () => {
  const st = new FileStorage(join(tmp, "receiver"));
  const agent = await Agent.open(st, "http://127.0.0.1:1");
  await agent.createIdentity();
  const me = await agent.pubkey();
  const peerKey = generateSecretKey();
  await contacts.add(st, getPublicKey(peerKey), "peer");
  const handle = (...wraps) => agent.handleWraps(wraps); // private in TypeScript; the lock is irrelevant here

  const put = st.put.bind(st);
  let stateWrites = 0;
  st.put = async (name, value) => {
    // The first marks the wraps handled; the second is the first message's.
    if (name === "state" && ++stateWrites === 2) throw Object.assign(new Error("no space left on device"), { code: "ENOSPC" });
    return put(name, value);
  };
  await assert.rejects(handle(wrap(peerKey, me, "lost 0"), wrap(peerKey, me, "lost 1")), { code: "ENOSPC" });
  await handle(wrap(peerKey, me, "kept 0"), wrap(peerKey, me, "kept 1"));

  const history = await st.readHistory();
  const seqs = history.map((e) => e.seq);
  assert.equal(new Set(seqs).size, seqs.length, `seqs ${seqs}`);
  assert.deepEqual(history.map((e) => e.text).filter((t) => t.startsWith("lost")), []);
  assert.deepEqual(history.map((e) => e.text).filter((t) => t.startsWith("kept")), ["kept 0", "kept 1"]);
  assert.equal((await st.get("state", {})).next_seq, seqs[seqs.length - 1] + 1);
});

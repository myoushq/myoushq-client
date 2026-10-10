// Cards (protocol section 4): a contact's card is kept on the contact and
// noted in the history, a rename is announced but never applied, and this
// agent's card goes once to each contact that is due one. Offline. Run
// after `npm run build`: node --test test/
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
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-cards-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

/** A kind-14 message from `sender` to `me`, gift-wrapped as the relay would deliver it. */
const wrap = (sender, me, text) =>
  wrapEvent({ kind: KIND_CHAT, content: text, tags: [["p", me]], created_at: Math.floor(Date.now() / 1000) }, sender, me);

const card = (name, about) => JSON.stringify({ myous: "card", name, about });

test("a contact's card is stored and announced, never applied", async () => {
  const st = new FileStorage(join(tmp, "receiver"));
  const agent = await Agent.open(st, "http://127.0.0.1:1");
  await agent.createIdentity();
  const me = await agent.pubkey();
  const peerKey = generateSecretKey();
  const peer = getPublicKey(peerKey);
  await contacts.add(st, peer, "peer");
  const handle = (...wraps) => agent.handleWraps(wraps); // private in TypeScript; the lock is irrelevant here

  const about = "Sam's own Mac; the 'myous browser' is its browser";
  let [e] = await handle(wrap(peerKey, me, card("peer", about)));
  assert.equal(e.type, "card");
  assert.equal(e.direction, "in");
  assert.equal(e.name, "peer");
  assert.equal(e.about, about);
  assert.equal(e.text, `peer describes itself: ${about}`);
  assert.equal((await contacts.load(st))[peer].card.about, about);

  // A rename is announced, never applied: the alias is ours.
  [e] = await handle(wrap(peerKey, me, card("Sam's Mac", about)));
  assert.equal(e.text, `peer now calls itself "Sam's Mac"; you call it "peer" (keep that, or follow it: myous rename "peer" "Sam's Mac")`);
  assert.equal((await contacts.load(st))[peer].alias, "peer");
  assert.equal((await contacts.load(st))[peer].card.name, "Sam's Mac");

  // Both at once, joined; the same card again: noted as unchanged; a cleared description is noted too.
  [e] = await handle(wrap(peerKey, me, card("Sam's Mac 2", "a Mac")));
  assert.equal(e.text, `peer now calls itself "Sam's Mac 2"; you call it "peer" (keep that, or follow it: myous rename "peer" "Sam's Mac 2"); describes itself: a Mac`);
  [e] = await handle(wrap(peerKey, me, card("Sam's Mac 2", "a Mac")));
  assert.equal(e.text, "peer sent its card again, unchanged");
  [e] = await handle(wrap(peerKey, me, card("Sam's Mac 2", "")));
  assert.equal(e.text, "peer cleared its description");

  // Incoming messages carry the contact's description, like the owner's context.
  await handle(wrap(peerKey, me, card("Sam's Mac 2", "a Mac")));
  await handle(wrap(peerKey, me, "hello"));
  const unread = await agent.unread();
  assert.equal(unread.at(-1).text, "hello");
  assert.equal(unread.at(-1).type, "message");
  assert.equal(unread.at(-1).about, "a Mac");

  // Malformed cards are plain messages: no name, a long name, a long about, wrong types.
  for (const bad of [{ myous: "card", about: "x" }, { myous: "card", name: "n".repeat(65), about: "" },
    { myous: "card", name: "n", about: "a".repeat(501) }, { myous: "card", name: 3, about: "" }, { myous: "card", name: "  ", about: "" }]) {
    [e] = await handle(wrap(peerKey, me, JSON.stringify(bad)));
    assert.equal(e.type, "message", JSON.stringify(bad));
  }
  assert.equal((await contacts.load(st))[peer].card.about, "a Mac");
  // An absent about is an empty one; the name and about are trimmed.
  assert.deepEqual(contacts.parseCard(JSON.stringify({ myous: "card", name: " Sam " })), { name: "Sam", about: "" });
  assert.equal(contacts.parseCard("[1]"), null);
  assert.equal(contacts.parseCard("{not json"), null);
});

test("which contacts are told this agent's card, and when", async () => {
  const st = new FileStorage(join(tmp, "sender"));
  const agent = await Agent.open(st, "http://127.0.0.1:1");
  await st.put("settings", { ...(await st.get("settings", {})), alias: "Max's Muse" });
  await contacts.add(st, "a".repeat(64), "old friend"); // from before cards: nothing recorded
  await contacts.add(st, "b".repeat(64), "new friend");
  await contacts.peerKnows(st, "b".repeat(64), "Max's Muse", ""); // paired now: knows the alias
  await contacts.add(st, "c".repeat(64), "blocked one");
  await contacts.update(st, "blocked one", { status: "blocked" });
  const due = async () => (await agent.cardsDue()).map((c) => c.alias).sort();

  assert.deepEqual(await due(), []); // no card: nothing to say

  // A card goes to everyone, once.
  await agent.setCard("Max's own assistant");
  assert.deepEqual(await due(), ["new friend", "old friend"]);
  await contacts.peerKnows(st, "b".repeat(64), "Max's Muse", "Max's own assistant");
  assert.deepEqual(await due(), ["old friend"]);

  // A rename is announced to those who knew the old name; the old friend never recorded one.
  await agent.setCard("");
  await contacts.peerKnows(st, "b".repeat(64), "Max's Muse", "");
  await st.put("settings", { ...(await st.get("settings", {})), alias: "Max's Assistant" });
  assert.deepEqual(await due(), ["new friend"]);

  // Limits.
  await assert.rejects(agent.setCard("x".repeat(501)), /limited to 500 characters/);
  assert.equal(await agent.setCard("  spaced  "), "spaced");
  assert.equal(await agent.setCard(null), "");
  assert.equal(await agent.card(), "");
  assert.equal(contacts.cardText("Max's Muse", "x"), '{"myous":"card","name":"Max\'s Muse","about":"x"}');
});

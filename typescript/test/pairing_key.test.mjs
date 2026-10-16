// The key a pairing pins: the other side proves it holds the key it sends
// (a signature over a value only the two sides know), and a pairing never
// changes a key that is already a contact: a blocked contact stays blocked,
// an approved one keeps its alias and context. Clients from before the
// proof send none and still pair. Against a fake hub; side B is written out
// by hand so a test can send any payload.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, test } from "node:test";
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import * as contacts from "../dist/contacts.js";
import { Hub } from "../dist/hub.js";
import { pakeFinish, pakeStart } from "../dist/pake.js";
import { Pairing, keyProof, proofHolds, sealText, signProof, unseal } from "../dist/pairing.js";
import { FileStorage } from "../dist/storage.js";

const NAMEPLATE = "4821";
const ERIN = { relationship: "friend", sharing: "my calendar and home address" };
const b64 = (b) => Buffer.from(b).toString("base64");
const unb64 = (s) => new Uint8Array(Buffer.from(s, "base64"));
const keys = () => {
  const sk = generateSecretKey();
  return { sk, pk: getPublicKey(sk) };
};

let tmp, server, url, box, n = 0;
beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "myous-key-"));
  server = createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const reply = (status, body) => {
        res.writeHead(status, { "Content-Type": "application/json", Connection: "close" });
        res.end(JSON.stringify(body));
      };
      const side = { "Bearer tok-a": "a", "Bearer tok-b": "b" }[req.headers.authorization];
      const exp = Math.floor(Date.now() / 1000) + 900;
      const path = `/api/pair/${NAMEPLATE}`;
      if (req.method === "POST" && req.url === "/api/pair") return reply(201, { nameplate: NAMEPLATE, token: "tok-a", expires_at: exp });
      if (req.method === "POST" && req.url === `${path}/claim`) return reply(200, { token: "tok-b", expires_at: exp, repeat: false });
      if (req.method === "POST" && req.url === `${path}/messages`) {
        box[side].push(JSON.parse(raw).body);
        return reply(200, { index: box[side].length - 1 });
      }
      if (req.method === "GET" && req.url.startsWith(`${path}/messages?`)) {
        const after = Number(new URL(req.url, "http://x").searchParams.get("after"));
        const theirs = box[side === "a" ? "b" : "a"].slice(after);
        return reply(200, { messages: theirs, next: after + theirs.length });
      }
      if (req.method === "DELETE") return reply(204, {});
      reply(404, { error: "unexpected" });
    });
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  url = `http://127.0.0.1:${server.address().port}`;
});
afterEach(() => {
  server.close();
  rmSync(tmp, { recursive: true, force: true });
});

/** Side A with its own storage, and an empty mailbox. */
async function fixture() {
  box = { a: [], b: [] };
  const st = new FileStorage(join(tmp, String(n++)));
  await st.put("hub", { url, fetched_at: Math.floor(Date.now() / 1000), pair_link_base: "https://hub.test/p/" });
  const me = keys();
  const hub = new Hub(st, url);
  const f = {
    st, me,
    async contacts() { return contacts.load(st); },
    async last() { return (await st.readHistory()).at(-1); },
    async block(k, alias) {
      await contacts.add(st, k.pk, alias);
      await contacts.update(st, alias, { status: "blocked" });
    },
    /** Invite with `context`; side B answers with make(K). Returns A's result, the payload A sent, and K. */
    async pair(context, make) {
      const a = new Pairing(st, hub, me.pk, "me", me.sk);
      const { code } = await a.invite(context);
      const { token } = await hub.request("POST", `/api/pair/${NAMEPLATE}/claim`, { claim: "ab".repeat(16) });
      const post = (msg) => hub.request("POST", `/api/pair/${NAMEPLATE}/messages`, { body: b64(JSON.stringify(msg)) }, token);
      const seed = new Uint8Array(32).fill(7);
      await post({ t: "pake", v: 1, m: b64(pakeStart("b", code, seed)) });
      assert.deepEqual(await a.advanceAll(), [], "A reads the PAKE and sends its payload");
      const got = await hub.request("GET", `/api/pair/${NAMEPLATE}/messages?after=0&wait=0`, undefined, token);
      const [pakeA, sealed] = got.messages.map((m) => JSON.parse(Buffer.from(m, "base64").toString()));
      const key = pakeFinish("b", code, seed, unb64(pakeA.m));
      await post(sealText(key, "b", NAMEPLATE, JSON.stringify(make(key))));
      const mine = unseal(key, "a", NAMEPLATE, sealed);
      const done = await a.advanceAll();
      assert.equal(done.length, 1, JSON.stringify(done));
      return { r: done[0], mine, key };
    },
  };
  return f;
}

/** A payload naming k; its proof signed by `signer` (none: as clients up to 0.7.5). */
const payloadOf = (k, alias, signer) => (key) => ({ v: 1, pubkey: k.pk, alias, ...(signer ? { sig: signProof(signer.sk, key, "b") } : {}) });

test("this side proves its key", async () => {
  const f = await fixture();
  const bob = keys();
  const { r, mine, key } = await f.pair(ERIN, payloadOf(bob, "bob", bob));
  assert.equal(r.stage, "done");
  assert.equal(mine.pubkey, f.me.pk);
  assert.ok(proofHolds(mine.pubkey, keyProof(key, "a"), mine.sig));
  assert.ok(!proofHolds(mine.pubkey, keyProof(key, "b"), mine.sig), "a proof is for one side");
  assert.equal((await f.contacts())[bob.pk].relationship, "friend");
});

test("a client without the proof still pairs", async () => {
  const f = await fixture();
  const bob = keys();
  const { r } = await f.pair(ERIN, payloadOf(bob, "bob"));
  assert.equal(r.stage, "done");
  assert.equal(r.known, false);
  const c = (await f.contacts())[bob.pk];
  assert.deepEqual([c.alias, c.status, c.relationship, c.sharing], ["bob", "approved", "friend", "my calendar and home address"]);
  assert.equal((await f.last()).type, "paired");
});

// The audit's M2: whoever answers names a blocked contact's key.
for (const proven of [false, true]) {
  test(`a blocked contact stays blocked (${proven ? "proven" : "no proof"})`, async () => {
    const f = await fixture();
    const eve = keys();
    await f.block(eve, "eve");
    const { r } = await f.pair(ERIN, payloadOf(eve, "Erin", proven ? eve : undefined));
    assert.equal(r.stage, "failed");
    const all = await f.contacts();
    const c = all[eve.pk];
    assert.deepEqual([Object.keys(all).length, c.alias, c.status, c.relationship, c.sharing], [1, "eve", "blocked", undefined, undefined]);
    const last = await f.last();
    assert.equal(last.type, "pairing_failed");
    assert.ok(last.text.includes(proven
      ? 'answered by your contact "eve" (blocked); nothing changed'
      : 'answered with the key of your contact "eve" (blocked), which the other side didn\'t prove it holds; ' +
        'nothing changed: if that is right, unblock it (myous unblock "eve")'), last.text);
  });
}

test("a bad proof fails", async () => {
  const eve = keys(), mallory = keys();
  const elsewhere = signProof(eve.sk, new Uint8Array(32).fill(9), "b");
  const cases = {
    "another key's": (key) => signProof(mallory.sk, key, "b"),
    "another pairing's": () => elsewhere,
    uppercase: (key) => signProof(eve.sk, key, "b").toUpperCase(),
    empty: () => "",
    zeros: () => "00".repeat(64),
    short: () => "ab",
    "a number": () => 3,
  };
  for (const [name, sig] of Object.entries(cases)) {
    const f = await fixture();
    const { r } = await f.pair(undefined, (key) => ({ v: 1, pubkey: eve.pk, alias: "Erin", sig: sig(key) }));
    assert.deepEqual([r.stage, r.error], ["failed", "the other side didn't prove it holds the key it sent"], name);
    assert.deepEqual(await f.contacts(), {}, name);
  }
});

test("an approved contact keeps its context", async () => {
  const f = await fixture();
  const bob = keys();
  const before = { ...(await contacts.add(f.st, bob.pk, "bob", { relationship: "colleague", sharing: "work only" })) };
  const { r } = await f.pair(ERIN, payloadOf(bob, "Erin"));
  assert.equal(r.stage, "done");
  assert.equal(r.known, true);
  const { peer_knows, ...after } = (await f.contacts())[bob.pk]; // peer_knows: set again, the peer learned our alias
  assert.ok(peer_knows);
  assert.deepEqual(after, before, "alias, status and context as they were");
  assert.equal(Object.keys(await f.contacts()).length, 1);
  const last = await f.last();
  assert.equal(last.type, "paired");
  assert.ok(last.text.includes("bob was already your contact: nothing changed (the relationship and sharing given with this " +
    'invite were not applied; to change them: myous context "bob"'), last.text);
});

test("a key written otherwise is refused", async () => {
  const bob = keys();
  for (const key of [bob.pk.toUpperCase(), nip19.npubEncode(bob.pk), "ff".repeat(32)]) {
    const f = await fixture();
    const { r } = await f.pair(undefined, () => ({ v: 1, pubkey: key, alias: "x" }));
    assert.deepEqual([r.stage, r.error], ["failed", "the code didn't match (or the exchange was tampered with)"], key);
    assert.deepEqual(await f.contacts(), {});
  }
  const f = await fixture();
  const { r } = await f.pair(undefined, () => ({ v: 1, pubkey: f.me.pk.toUpperCase(), alias: "x" }));
  assert.equal(r.stage, "failed", "own key in uppercase");
  assert.deepEqual(await f.contacts(), {});
});

// The context goes to the new contact by its key, never to a contact whose
// alias happens to be that key.
test("an alias that names a key gets no context", async () => {
  const f = await fixture();
  const bob = keys(), mallory = keys();
  await contacts.add(f.st, mallory.pk, bob.pk);
  const { r } = await f.pair(ERIN, payloadOf(bob, "bob", bob));
  assert.equal(r.stage, "done");
  const all = await f.contacts();
  assert.equal(all[bob.pk].relationship, "friend");
  assert.equal(all[mallory.pk].relationship, undefined);
});

test("add leaves a contact as it is", async () => {
  const f = await fixture();
  const eve = keys();
  await contacts.add(f.st, eve.pk, "eve", { relationship: "other" });
  await contacts.update(f.st, "eve", { status: "blocked" });
  const c = await contacts.add(f.st, eve.pk, "eve again", { relationship: "family" });
  assert.deepEqual([c.alias, c.status, c.relationship], ["eve", "blocked", "other"]);
});

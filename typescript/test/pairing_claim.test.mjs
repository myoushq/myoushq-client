// Accepting a code: the claim carries an id the client saves first, so a
// lost answer, a retry, or running the same accept again after a crash all
// get the same claim instead of burning the code. Against a fake hub.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, test } from "node:test";
import { Hub } from "../dist/hub.js";
import { CLAIM, Pairing, PairingError, pairingTrace } from "../dist/pairing.js";
import { FileStorage } from "../dist/storage.js";

const NAMEPLATE = "4821";
CLAIM.retryDelayMs = 1;

// The hub's claim rules (a repeat with the claimer's id gets the same
// token; anything else after the claim gets 409) and a mailbox that takes
// our messages and has none for us. It loses the first `lose` claim
// answers after acting on them, and answers the statuses in `fail` first.
function fakeHub({ lose = 0, fail = [], claimedBy = null } = {}) {
  const h = { lose, fail: [...fail], claimedBy, claimedAt: 1_700_000_000, claims: [], posts: 0 };
  h.server = createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const reply = (status, body) => {
        res.writeHead(status, { "Content-Type": "application/json", Connection: "close" });
        res.end(JSON.stringify(body));
      };
      const path = req.url;
      if (req.method === "POST" && path === `/api/pair/${NAMEPLATE}/claim`) {
        const id = JSON.parse(raw || "{}").claim;
        h.claims.push(id);
        if (h.fail.length) return reply(h.fail.shift(), { error: "refused" });
        const answer = { repeat: h.claimedBy !== null };
        if (h.claimedBy === null) [h.claimedBy, h.claimedAt] = [id, Math.floor(Date.now() / 1000) - 5];
        else if (id !== h.claimedBy) return reply(409, { error: "invite already used, ask for a new one" });
        else answer.claimed_at = h.claimedAt;
        if (h.lose > 0) {
          h.lose--;
          return req.socket.destroy(); // the claim is taken; its answer never arrives
        }
        return reply(200, { ...answer, token: `tok-${h.claimedBy}`, expires_at: Math.floor(Date.now() / 1000) + 900 });
      }
      if (req.method === "POST" && path === `/api/pair/${NAMEPLATE}/messages`) {
        if (req.headers.authorization !== `Bearer tok-${h.claimedBy}`) return reply(403, { error: "bad token" });
        h.posts++;
        return reply(200, { index: h.posts - 1 });
      }
      if (req.method === "GET" && path.startsWith(`/api/pair/${NAMEPLATE}/messages?`)) return reply(200, { messages: [], next: 0 });
      reply(404, { error: `unexpected ${req.method} ${path}` });
    });
  });
  return h;
}

let tmp, servers;
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "myous-claim-"));
  servers = [];
});
afterEach(() => {
  for (const s of servers) s.close();
  rmSync(tmp, { recursive: true, force: true });
});

async function setup(h) {
  await new Promise((r) => h.server.listen(0, "127.0.0.1", r));
  servers.push(h.server);
  const url = `http://127.0.0.1:${h.server.address().port}`;
  const st = new FileStorage(tmp);
  await st.put("hub", { url, fetched_at: Math.floor(Date.now() / 1000), pair_link_base: "https://hub.test/p/" });
  const accept = () => new Pairing(st, new Hub(st, url), "00".repeat(32), "me").accept(`${NAMEPLATE}-K7F3QX`, 0);
  const savedClaim = async () => (await st.get(`claiming/${NAMEPLATE}`, null))?.claim;
  return { st, url, accept, savedClaim };
}

test("a lost answer, then the retry gets the claim", async () => {
  const h = fakeHub({ lose: 1 });
  const { st, accept, savedClaim } = await setup(h);
  const p = await accept();
  assert.equal(p.stage, "wait_pake");
  assert.equal(h.claims.length, 2);
  assert.equal(h.claims[0], h.claims[1]);
  assert.match(h.claims[0], /^[0-9a-f]{32}$/);
  assert.equal(p.token, `tok-${h.claims[0]}`);
  assert.equal((await st.get(`pending/${NAMEPLATE}`, null)).token, p.token);
  assert.equal(h.posts, 1);
  assert.equal(await savedClaim(), undefined, "the claiming record goes once the pairing is saved");
});

test("a proxy's 5xx is retried like a drop", async () => {
  const h = fakeHub({ fail: [502] });
  const { accept } = await setup(h);
  assert.equal((await accept()).stage, "wait_pake");
  assert.equal(h.claims.length, 2);
});

test("a re-run after a crash reuses the saved id", async () => {
  const id = "ab".repeat(16);
  const h = fakeHub({ claimedBy: id });
  const { st, accept, savedClaim } = await setup(h);
  // The first run saved its id and died before it saw the hub's answer.
  await st.put(`claiming/${NAMEPLATE}`, { nameplate: NAMEPLATE, claim: id });
  const p = await accept();
  assert.deepEqual(h.claims, [id]);
  assert.equal(p.token, `tok-${id}`);
  assert.equal(await savedClaim(), undefined);
});

test("the hub unreachable: the id stays for the next run", async () => {
  const h = fakeHub({ lose: CLAIM.attempts });
  const { accept, savedClaim } = await setup(h);
  await assert.rejects(accept(), (e) => {
    assert.ok(!(e instanceof PairingError));
    assert.match(e.message, /^could not reach the hub/);
    assert.match(e.message, /fetch failed: /, "the drop's own text is in the message");
    return true;
  });
  assert.equal(h.claims.length, CLAIM.attempts);
  assert.equal(await savedClaim(), h.claims[0]);
  // The same accept again carries on with the claim the hub already gave.
  const p = await accept();
  assert.ok(h.claims.every((c) => c === h.claims[0]));
  assert.equal(p.token, `tok-${h.claims[0]}`);
  assert.equal(await savedClaim(), undefined);
});

test("taken by someone else: refused plainly, not retried", async () => {
  const h = fakeHub({ claimedBy: "cd".repeat(16) });
  const { accept, savedClaim } = await setup(h);
  await assert.rejects(accept(), (e) => e instanceof PairingError && e.message === "invite already used, ask for a new one");
  assert.equal(h.claims.length, 1);
  assert.equal(await savedClaim(), undefined, "a refusal drops the claiming record");
});

test("a rate limit keeps the id", async () => {
  const h = fakeHub({ fail: [429] });
  const { accept, savedClaim } = await setup(h);
  await assert.rejects(accept(), PairingError);
  assert.equal(h.claims.length, 1);
  assert.equal(await savedClaim(), h.claims[0]);
});

test("the trace names the drop and the resume", async () => {
  const h = fakeHub({ lose: 1 });
  const { st, url, accept } = await setup(h);
  const { trace } = await accept();
  assert.deepEqual(trace.map((s) => s.step), ["claim", "claim", "post"]);
  assert.match(trace[0].error, /fetch failed/);
  assert.equal(trace[0].host, new URL(url).host);
  assert.equal(trace[1].error, undefined);
  assert.equal(trace[1].claimed_at, h.claimedAt, "the resume");
  assert.deepEqual((await st.get(`pending/${NAMEPLATE}`, null)).trace, trace, "kept in the pairing's record");
});

test("a failed accept keeps the trace", async () => {
  const h = fakeHub({ claimedBy: "cd".repeat(16) });
  const { st, accept } = await setup(h);
  const started = Math.floor(Date.now() / 1000);
  await assert.rejects(accept(), PairingError);
  const t = await pairingTrace(st, started);
  assert.equal(t.nameplate, NAMEPLATE);
  assert.equal(t.trace.length, 1);
  assert.equal(t.trace[0].step, "claim");
  assert.match(t.trace[0].error, /invite already used/);
  assert.equal(await pairingTrace(st, started + 3600), null, "an older trace counted as this run's");
});

// Both sides of one mailbox, as the hub keeps it: a claim id that repeats,
// and a side's repeat of its last message answered without appending. The
// answers to the first `losePosts` of side B's posts are lost after the
// message is stored.
function mailboxHub({ losePosts = 0 } = {}) {
  const h = { losePosts, claimedBy: null, a: [], b: [] };
  h.server = createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const reply = (status, body) => {
        res.writeHead(status, { "Content-Type": "application/json", Connection: "close" });
        res.end(JSON.stringify(body));
      };
      const side = { "Bearer tok-a": "a", "Bearer tok-b": "b" }[req.headers.authorization];
      const box = `/api/pair/${NAMEPLATE}`;
      const exp = Math.floor(Date.now() / 1000) + 900;
      const body = raw ? JSON.parse(raw) : {};
      if (req.method === "POST" && req.url === "/api/pair") return reply(201, { nameplate: NAMEPLATE, token: "tok-a", expires_at: exp });
      if (req.method === "POST" && req.url === `${box}/claim`) {
        if (h.claimedBy !== null && h.claimedBy !== body.claim) return reply(409, { error: "invite already used, ask for a new one" });
        const repeat = h.claimedBy !== null;
        h.claimedBy = body.claim;
        return reply(200, { token: "tok-b", expires_at: exp, repeat });
      }
      if (req.method === "POST" && req.url === `${box}/messages`) {
        const mine = h[side];
        if (mine.length && mine.at(-1) === body.body) return reply(200, { index: mine.length - 1, repeat: true });
        mine.push(body.body);
        if (side === "b" && h.losePosts > 0) {
          h.losePosts--;
          return req.socket.destroy();
        }
        return reply(200, { index: mine.length - 1 });
      }
      if (req.method === "GET" && req.url.startsWith(`${box}/messages?`)) {
        const after = Number(new URL(req.url, "http://x").searchParams.get("after"));
        const theirs = h[side === "a" ? "b" : "a"].slice(after);
        return reply(200, { messages: theirs, next: after + theirs.length });
      }
      if (req.method === "DELETE") return reply(204, {});
      reply(404, { error: "unexpected" });
    });
  });
  return h;
}

test("a lost answer to the PAKE post: the re-run posts the same message, and both sides finish", async () => {
  const { generateSecretKey, getPublicKey } = await import("nostr-tools");
  const h = mailboxHub({ losePosts: 1 });
  await new Promise((r) => h.server.listen(0, "127.0.0.1", r));
  servers.push(h.server);
  const url = `http://127.0.0.1:${h.server.address().port}`;
  const side = async (name, alias) => {
    const st = new FileStorage(join(tmp, name));
    await st.put("hub", { url, fetched_at: Math.floor(Date.now() / 1000), pair_link_base: "https://hub.test/p/" });
    return { st, p: new Pairing(st, new Hub(st, url), getPublicKey(generateSecretKey()), alias) };
  };
  const a = await side("a", "alice");
  const b = await side("b", "bob");
  const { code } = await a.p.invite();

  const first = await b.p.accept(code, 0); // the post reaches the mailbox; its answer doesn't
  assert.equal(first.stage, "posting");
  const saved = await b.st.get(`pending/${NAMEPLATE}`, null);
  assert.equal(saved.stage, "posting");
  assert.equal(saved.message, h.b[0], "the exact message is saved");

  const again = await b.p.accept(code, 0); // the same accept, run again
  assert.equal(again.stage, "wait_payload");
  assert.deepEqual(again.trace.map((s) => s.step), ["claim", "post", "post (repeat)"]);
  assert.match(again.trace[1].error, /fetch failed/);

  const doneA = await a.p.advanceAll();
  assert.deepEqual(doneA.map((r) => r.stage), ["done"], "side A completes");
  const doneB = await b.p.advanceAll();
  assert.deepEqual(doneB.map((r) => r.stage), ["done"]);
  assert.equal(doneA[0].verify, doneB[0].verify);
  const pakes = h.b.filter((m) => Buffer.from(m, "base64").toString().includes('"pake"'));
  assert.equal(pakes.length, 1, "one PAKE message from B, not two");
});

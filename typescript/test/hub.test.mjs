// Files end to end against a local hub: two agents that share contacts
// directly (pairing has its own tests in interop_test.py), one sends a
// file, the other fetches it; plus the blob store's own rules. Needs Go and
// the hub source (../myoushq/hub, or $MYOUS_HUB_SRC); skipped otherwise.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { Agent } from "../dist/agent.js";
import * as contacts from "../dist/contacts.js";
import { Blobs, encryptFile } from "../dist/files.js";
import { FileStorage } from "../dist/storage.js";

const root = fileURLToPath(new URL("../..", import.meta.url));
const hubSrc = process.env.MYOUS_HUB_SRC ?? join(root, "..", "myoushq", "hub");
const available = existsSync(join(hubSrc, "main.go")) && spawnSync("go", ["version"]).status === 0;

let tmp, hub, hubUrl, alice, bob;

const freePort = () => new Promise((resolve) => {
  const s = createServer();
  s.listen(0, "127.0.0.1", () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

before(async () => {
  if (!available) return;
  tmp = mkdtempSync(join(tmpdir(), "myous-ts-e2e-"));
  const bin = join(tmp, "hub");
  const build = spawnSync("go", ["build", "-o", bin, "."], { cwd: hubSrc, encoding: "utf8" });
  assert.equal(build.status, 0, build.stderr);
  const port = await freePort();
  hubUrl = `http://127.0.0.1:${port}`;
  hub = spawn(bin, [], {
    env: { ...process.env, LISTEN_ADDR: `127.0.0.1:${port}`, DATA_DIR: join(tmp, "hubdata"), WEB_DIR: join(hubSrc, "web"),
      DOCS_DIR: join(root, "docs"), RELAY_URL: `ws://127.0.0.1:${port}`, PUBLIC_URL: hubUrl, POW_DIFFICULTY: "8", RATE_LIMIT_SCALE: "100" },
    stdio: ["ignore", "ignore", "pipe"],
  });
  for (let i = 0; i < 100; i++) {
    try { await fetch(hubUrl + "/config.json"); break; } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
  const open = async (name) => {
    const a = await Agent.open(new FileStorage(join(tmp, name)), hubUrl);
    await a.createIdentity();
    await a.register(name);
    return a;
  };
  [alice, bob] = await Promise.all([open("alice"), open("bob")]);
  await contacts.add(alice.st, await bob.pubkey(), "bob");
  await contacts.add(bob.st, await alice.pubkey(), "alice");
}, { timeout: 120_000 });

after(() => {
  hub?.kill();
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

test("config announces the blob store", { skip: !available && "no hub source or Go" }, async () => {
  const cfg = await alice.hub.config(true);
  assert.equal(cfg.blob_api, hubUrl + "/blob");
});

test("send a file, fetch it on the other side", { skip: !available && "no hub source or Go" }, async () => {
  const src = join(tmp, "notes.txt");
  const content = "the quick brown fox ".repeat(5000); // ~100 KB: past the message cap, not a problem for a blob
  writeFileSync(src, content);
  const sent = await alice.sendFile("bob", src, [["w", "put", "a".repeat(32), "in/notes.txt"]], "text/plain");
  assert.equal(sent.type, "file");
  assert.equal(sent.direction, "out");
  assert.deepEqual(sent.w, ["put", "a".repeat(32), "in/notes.txt"]);

  const got = await bob.poll();
  const entry = got.find((e) => e.type === "file");
  assert.ok(entry, `no file entry in ${JSON.stringify(got)}`);
  assert.equal(entry.name, "notes.txt");
  assert.equal(entry.mime, "text/plain");
  assert.equal(entry.ox, sent.ox);
  assert.deepEqual(entry.w, ["put", "a".repeat(32), "in/notes.txt"]);
  // "put" files are new items for the receiver; "file" replies are not.
  assert.ok((await bob.unread(false)).some((e) => e.seq === entry.seq));

  const path = await bob.fetch(entry, join(tmp, "out"));
  assert.equal(readFileSync(path, "utf8"), content);
  const again = await bob.fetch(entry, join(tmp, "out"));
  assert.notEqual(again, path, "never overwrites");
  assert.match(again, /notes-1\.txt$/);
}, { timeout: 60_000 });

test("worker replies are recorded by type and kept out of unread", { skip: !available && "no hub source or Go" }, async () => {
  const id = "b".repeat(32);
  await alice.send("bob", JSON.stringify({ myous: "result", id, exit: 0, stdout: "hi\n", stderr: "", truncated: false }));
  await alice.send("bob", JSON.stringify({ myous: "ack", id, ok: false, error: "nope" }));
  await alice.send("bob", "a plain message");
  const got = await bob.poll();
  const result = got.find((e) => e.type === "result");
  assert.equal(result?.id, id);
  assert.equal(result?.exit, 0);
  assert.equal(result?.stdout, "hi\n");
  const ack = got.find((e) => e.type === "ack");
  assert.equal(ack?.ok, false);
  assert.equal(ack?.error, "nope");
  const unread = await bob.unread(false);
  assert.ok(!unread.some((e) => e.type === "result" || e.type === "ack"));
  assert.ok(unread.some((e) => e.type === "message" && e.text === "a plain message"));
}, { timeout: 60_000 });

test("blob store rules", { skip: !available && "no hub source or Go" }, async () => {
  const api = hubUrl + "/blob";
  const mine = new Blobs(await alice.key(), api);
  const theirs = new Blobs(await bob.key(), api);
  const enc = await encryptFile(new TextEncoder().encode("secret bytes"));
  const desc = await mine.upload(enc.ciphertext);
  assert.equal(desc.sha256, enc.x);
  assert.equal(desc.url, `${api}/${enc.x}`);
  assert.deepEqual(await theirs.get(enc.x), enc.ciphertext);
  await assert.rejects(theirs.delete(enc.x), /403/);
  await mine.delete(enc.x);
  await assert.rejects(theirs.get(enc.x), /404/);
  // An unregistered key is refused.
  const stranger = new Blobs(new Uint8Array(32).fill(7), api);
  await assert.rejects(stranger.upload(enc.ciphertext), /403/);
}, { timeout: 60_000 });

test("a pairing records added_by on both contacts", { skip: !available && "no hub source or Go" }, async () => {
  const inv = await alice.invite({ added_by: "owner" });
  const accepting = bob.accept(inv.code, 30, { added_by: "owner", relationship: "friend" });
  let done = [];
  for (let i = 0; i < 150 && !done.length; i++) {
    done = await alice.advancePairings();
    if (!done.length) await new Promise((r) => setTimeout(r, 200));
  }
  assert.equal(done[0]?.stage, "done", JSON.stringify(done));
  assert.equal((await accepting).stage, "done");
  const mine = (await alice.contacts())[await bob.pubkey()];
  assert.equal(mine.added_by, "owner");
  assert.equal(mine.relationship, undefined);
  const theirs = (await bob.contacts())[await alice.pubkey()];
  assert.equal(theirs.added_by, "owner");
  assert.equal(theirs.relationship, "friend");
}, { timeout: 60_000 });

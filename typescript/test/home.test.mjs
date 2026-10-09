// The data directory: the init guard (one agent, one home), last_used, and
// added_by on contacts. Offline. Run after `npm run build`: node --test test/
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { Agent, IdentityError } from "../dist/agent.js";
import * as contacts from "../dist/contacts.js";
import { FileStorage, lastUsed } from "../dist/storage.js";

let tmp;
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-home-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

test("init refuses to adopt another agent's home", async () => {
  const st = new FileStorage(join(tmp, "home"));
  const agent = await Agent.open(st, "http://127.0.0.1:1");
  await agent.checkHome("alice"); // no identity yet: anything goes
  await agent.createIdentity();
  await agent.checkHome("alice"); // identity, but no stored alias
  await st.put("settings", { ...(await st.get("settings", {})), alias: "alice" });
  await agent.checkHome("alice"); // same alias
  await assert.rejects(agent.checkHome("bob"), (e) => e instanceof IdentityError &&
    e.message === "this directory belongs to alice; use another MYOUS_HOME, or pass --rename if this is the same agent");
  await agent.checkHome("bob", true); // --rename
});

test("last_used is the newest file, skipping installs", async () => {
  const dir = join(tmp, "used");
  assert.equal(await lastUsed(join(tmp, "missing")), null);
  mkdirSync(join(dir, "pending"), { recursive: true });
  mkdirSync(join(dir, "venv", "lib"), { recursive: true });
  assert.equal(await lastUsed(dir), null); // directories only
  const at = (path, t) => { writeFileSync(path, "x"); utimesSync(path, t, t); };
  at(join(dir, "contacts.json"), 1_700_000_000);
  at(join(dir, "pending", "4821.json"), 1_700_000_500);
  at(join(dir, "venv", "lib", "site.py"), 1_900_000_000); // ignored
  assert.equal(await lastUsed(dir), 1_700_000_500);
});

test("added_by round-trips through the contacts store", async () => {
  const st = new FileStorage(join(tmp, "contacts"));
  const pubkey = "ab".repeat(32);
  await contacts.add(st, pubkey, "carol");
  assert.deepEqual(contacts.contextFields({ added_by: "owner", relationship: "friend" }), { added_by: "owner", relationship: "friend" });
  assert.deepEqual(contacts.contextFields({ added_by: "  " }), {}); // empty: not stored
  await contacts.update(st, "carol", contacts.contextFields({ added_by: "owner" }));
  const again = new FileStorage(join(tmp, "contacts"));
  const [, c] = await contacts.find(again, "carol");
  assert.equal(c.added_by, "owner");
  assert.equal(c.relationship, undefined);
});

// FileStorage's lock between processes: a lock whose process is gone is
// taken over, a held one makes a waiting caller fail clearly after
// LOCK.waitMs, and the lock files of the flock() clients are not ours.
// Offline. Run after `npm run build`: node --test test/
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { FileStorage, LOCK } from "../dist/storage.js";

let tmp;
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-lock-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

function home(name) {
  const st = new FileStorage(join(tmp, name));
  mkdirSync(join(st.home, "locks"), { recursive: true });
  return st;
}

test("a lock left by a process that is gone is taken at once", async () => {
  const st = home("dead");
  const gone = spawnSync(process.execPath, ["-e", "process.stdout.write(String(process.pid))"]).stdout.toString();
  writeFileSync(join(st.home, "locks", "state.pid"), gone);
  const started = Date.now();
  assert.equal(await st.withLock("state", async () => "ran"), "ran");
  assert.ok(Date.now() - started < 1000);
  assert.ok(!existsSync(join(st.home, "locks", "state.pid")), "released");
});

test("a held lock: no wait gives undefined, a wait fails clearly", async () => {
  const st = home("held");
  const holder = spawn(process.execPath, ["-e", "setTimeout(() => {}, 30000)"]);
  writeFileSync(join(st.home, "locks", "pending_4821.pid"), String(holder.pid));
  const saved = LOCK.waitMs;
  LOCK.waitMs = 300;
  try {
    assert.equal(await st.withLock("pending/4821", async () => "ran", false), undefined);
    await assert.rejects(st.withLock("pending/4821", async () => "ran"),
      /another myous run has held the pending\/4821 lock for 0.3 s .*pending_4821\.pid/);
    holder.kill();
    await new Promise((r) => holder.once("exit", r));
    assert.equal(await st.withLock("pending/4821", async () => "ran"), "ran", "its process is gone now");
  } finally {
    LOCK.waitMs = saved;
    holder.kill();
  }
});

test("the lock file a flock() client leaves doesn't block", async () => {
  const st = home("mixed");
  writeFileSync(join(st.home, "locks", "state"), ""); // as Python, Go and Rust leave it
  const started = Date.now();
  assert.equal(await st.withLock("state", async () => "ran"), "ran");
  assert.ok(Date.now() - started < 1000);
  assert.ok(existsSync(join(st.home, "locks", "state")), "theirs is left alone");
});

test("callers in one process take turns", async () => {
  const st = home("turns");
  const order = [];
  const step = (n) => st.withLock("state", async () => {
    order.push(`start ${n}`);
    await new Promise((r) => setTimeout(r, 20));
    order.push(`end ${n}`);
  });
  await Promise.all([step(1), step(2)]);
  assert.deepEqual(order, ["start 1", "end 1", "start 2", "end 2"]);
});

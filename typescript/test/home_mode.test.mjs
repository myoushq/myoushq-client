// mkdir's mode only applies to a directory it creates: an existing myous
// directory (made by `python3 -m venv` before the first command, or by
// hand) is usually 0755, and files written in it without a mode of their
// own would be readable by every user. Unix only. Offline. Run after
// `npm run build`: node --test test/
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { FileStorage } from "../dist/storage.js";

let tmp;
before(() => { tmp = mkdtempSync(join(tmpdir(), "myous-ts-home-")); });
after(() => rmSync(tmp, { recursive: true, force: true }));

test("an existing home becomes private at the first write", { skip: process.platform === "win32" }, async () => {
  const home = join(tmp, "home");
  mkdirSync(home, { mode: 0o755 });
  const old = process.umask(0o022);
  try {
    await new FileStorage(home).put("settings", { alias: "a" });
  } finally {
    process.umask(old);
  }
  assert.equal(statSync(home).mode & 0o777, 0o700);
});

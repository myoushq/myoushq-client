// The state machine and the words, without a window: each made-up state
// reaches its screen, real-looking files drive the screens through a
// start and a stop, and the tray menu offers what the state allows.

import assert from "node:assert/strict";
import { test } from "node:test";
import { applyFake, fakeStates, previewBrowserCheck } from "./fake.js";
import { appMenu, browserItems, buildMenu, MenuActions, moreMenu } from "./menu.js";
import { ADVANCE_SHOWS, Check, isAdvance, isUrgent, Model, Screen, UNKNOWN_AFTER, Worker, WorkerFiles } from "./model.js";
import { advanceWords, attentionCard, browserAdvance, browserCard, browserRow, checkAdvance, checklist, hubHost, hubLine, installLine, macBrowserHint, macBrowserQuestion, runtimeCard } from "./setup.js";
import { byteCount, countdown, dayLabel, defaultDescription, isDefaultDescription, newer, pairingMessage, plural, setHostOs, timeAgo } from "./text.js";

const T = 1_791_700_000;

function model() {
  const m = new Model("0.6.1", "myous for Linux");
  m.now = () => T;
  return m;
}

const evaluate = (m: Model) => m.workers.forEach((w) => m.evaluate(w));

test("every made-up state reaches its screen", () => {
  const want: Record<string, Screen> = { setup: "setup", noruntime: "noruntime", stoppedruntime: "noruntime", starting: "starting",
    pair: "pair", pairlocal: "pair", paired: "paired", running: "running", macbrowser: "running", paused: "running", stopping: "stopping", stopped: "stopped",
    approval: "running", busy: "running", agents: "running", agent: "running", workers: "running", nowsl: "noruntime", novirt: "noruntime",
    direct: "setup", directstarting: "starting", directrunning: "running", checking: "setup", couldntcheck: "setup", hubdown: "setup",
    installing: "noruntime", stoppedengine: "noruntime", nopermission: "noruntime", nocompose: "noruntime", runtimeerror: "noruntime",
    failed: "stopped", directnobrowser: "setup", directnopython: "setup", directnovenv: "setup", directnoplaywright: "setup",
    directinstalling: "setup", found: "setup", directoff: "setup", directbrowserclosed: "running", directbrowserlater: "running" };
  assert.deepEqual(Object.keys(want).sort(), [...fakeStates].sort());
  for (const [state, screen] of Object.entries(want)) {
    const m = model();
    applyFake(m, state);
    evaluate(m);
    assert.equal(m.current.screen, screen, state);
  }
  const m = model();
  applyFake(m, "workers");
  evaluate(m);
  assert.equal(m.workers[1].screen, "pair");
  assert.equal(m.workers[1].stateWord, "Running");
  applyFake(m, "approval");
  evaluate(m);
  assert.equal(m.current.stateWord, "Needs attention");
  assert.equal(m.worstKind, "red");
  applyFake(m, "paused");
  evaluate(m);
  assert.equal(m.current.stateWord, "Paused");
  assert.equal(m.current.stateKind, "orange");
});

test("real files drive the screens through a start and a stop", () => {
  const m = model();
  let now = T;
  m.now = () => now;
  const notified: string[] = [];
  m.notify = (kind) => notified.push(kind);
  const files = (extra: Partial<WorkerFiles> = {}): WorkerFiles => ({ home: "/h/.myous-worker", folder: ".myous-worker", is_default: true,
    project: "myous-worker", config: {}, status: null, status_modified: 0, requests: [], approvals: [], paused: false, review: "trust", ...extra });
  m.runtimeState = "ok";
  m.load([files()]);
  const w = m.current;
  evaluate(m);
  assert.equal(w.screen, "setup");
  m.load([files({ config: { name: "Max's PC" } })]);
  evaluate(m);
  assert.equal(w.screen, "stopped");
  assert.equal(w.attention, "");
  // Start pressed: starting for 90 s even before the status appears.
  w.launchedAt = now;
  evaluate(m);
  assert.equal(w.screen, "starting");
  const invite = { code: "1111-AAAAAA", expires_at: now + 800 };
  m.load([files({ config: { name: "Max's PC" }, status: { phase: "registering", contacts: 0, invite }, status_modified: now })]);
  evaluate(m);
  assert.equal(w.screen, "starting");
  m.load([files({ config: { name: "Max's PC" }, status: { phase: "running", contacts: 0, invite }, status_modified: now })]);
  evaluate(m);
  assert.equal(w.screen, "pair");
  assert.ok(w.alive);
  assert.equal(w.launchedAt, 0);
  // Stop pressed, compose down done: dead at once although the file is fresh.
  w.stopping = true;
  w.stoppedAt = now;
  evaluate(m);
  assert.equal(w.screen, "stopping");
  w.stopDoneAt = now + 1;
  evaluate(m);
  assert.equal(w.screen, "stopped");
  assert.ok(w.stoppedByUs);
  assert.equal(w.attention, "");
  // Running again, then the file goes stale on its own: attention, one notification, the window once.
  const needy: string[] = [];
  m.needsAttention = (x) => needy.push(x.attention);
  w.stopDoneAt = 0;
  m.load([files({ config: { name: "Max's PC" }, status: { phase: "running", contacts: 1 }, status_modified: now })]);
  evaluate(m);
  assert.equal(w.screen, "running");
  now += 60;
  m.ticks = 5;
  evaluate(m);
  assert.equal(w.screen, "stopped");
  assert.equal(w.attention, "The worker stopped on its own.");
  evaluate(m);
  assert.deepEqual(notified, ["stopped"]);
  assert.deepEqual(needy, ["The worker stopped on its own."], "the window comes up once, when the line appears");
  // An error phase says why.
  m.load([files({ config: { name: "Max's PC" }, status: { phase: "error: the hub refused the key" }, status_modified: now })]);
  evaluate(m);
  assert.equal(w.attention, "the hub refused the key");
});

test("only a request waiting for an answer is urgent", () => {
  const m = model();
  const sent: [string, boolean][] = [];
  m.notify = (kind) => sent.push([kind, isUrgent(kind)]);
  m.load([{ home: "/h/.myous-worker", folder: ".myous-worker", is_default: true, project: "myous-worker", config: { name: "Max's PC" },
    status: null, status_modified: 0, requests: [], approvals: [], paused: false, review: "ask" }]);
  for (const key of ["ask-1", "refused-2", "paired-3", "update-v0.7.0", "stopped"]) m.notifyOnce(m.current, key, "title", "body");
  assert.deepEqual(sent, [["approval", true], ["refused", false], ["paired", false], ["update", false], ["stopped", false]]);
});

test("an expired request is neither a refusal to notify nor running", () => {
  // A worker back after a while answers what its agents sent meanwhile "expired", all at once: no notification each.
  const m = model();
  const sent: string[] = [];
  m.notify = (kind, _title, body) => sent.push(`${kind}: ${body}`);
  const reason = "expired: sent 3 hours ago, its ttl is 150 seconds; not run";
  m.load([{ home: "/h/.myous-worker", folder: ".myous-worker", is_default: true, project: "myous-worker", config: { name: "Max's PC" },
    status: { phase: "running", contacts: 1 }, status_modified: T, approvals: [], paused: false, review: "trust", requests: [
      { id: "x3", op: "exec", alias: "Max's Muse", cmd: "ls", at: T - 2, done_at: T - 2, decision: "expired", reason },
      { id: "x2", op: "put", alias: "Max's Muse", path: "a.txt", at: T - 2, done_at: T - 2, decision: "expired", reason },
      { id: "x1", op: "exec", alias: "Max's Muse", cmd: "rm x", at: T - 3, done_at: T - 3, decision: "refuse", reason: "no" },
    ] }]);
  m.ticks = 5;
  evaluate(m);
  assert.deepEqual(sent, ["refused: rm x: no"]);
  assert.equal(m.current.runningRequest(T), null);
  assert.equal(m.current.requestInProgress(T), false);
  assert.equal(m.current.screen, "running");
});

test("a question's notification names its worker and request (a Mac answers it from there)", () => {
  const m = model();
  const sent: [string, string, unknown][] = [];
  m.notify = (kind, title, _body, ask) => sent.push([kind, title, ask]);
  m.load([{ home: "/h/.myous-worker", folder: ".myous-worker", is_default: true, project: "myous-worker", config: { name: "Max's PC" },
    status: null, status_modified: 0, requests: [], approvals: [{ id: "r5", op: "exec", alias: "Max's Muse", cmd: "ls", asked_at: 1, wait: 120 }],
    paused: false, review: "all" }]);
  evaluate(m);
  evaluate(m);
  assert.deepEqual(sent, [["approval", "Max's Muse wants to run on Max's PC", { home: "/h/.myous-worker", id: "r5" }]]);
  m.notifyOnce(m.current, "update-v0.7.0", "title", "body");
  assert.deepEqual(sent[1], ["update", "title", undefined]);
});

test("without containers: no runtime needed, every worker runs directly", () => {
  const m = model();
  const written: string[] = [];
  m.writeConfig = (w) => written.push(w.home);
  m.runtimeState = "missing";
  const f = (home: string, config: Record<string, unknown>): WorkerFiles => ({ home, folder: home, is_default: home === "/a", project: "p",
    config, status: null, status_modified: 0, requests: [], approvals: [], paused: false, review: "trust" });
  // A container worker with no runtime asks for one.
  m.load([f("/a", { name: "Max's PC" })]);
  evaluate(m);
  assert.equal(m.current.screen, "noruntime");
  // The owner went on without containers: the second worker follows, and nothing asks for a runtime.
  m.load([f("/a", { name: "Max's PC", containers: false, mode: "direct" }), f("/a-2", {})]);
  evaluate(m);
  assert.deepEqual(m.workers.map((w) => w.config.mode), ["direct", "direct"]);
  assert.deepEqual(written, ["/a-2"]);
  assert.equal(m.workers[0].screen, "stopped");
  assert.equal(m.workers[1].screen, "setup");
  assert.match(m.workers[0].config.effectiveDescription, /not in a container/);
  assert.doesNotMatch(m.workers[0].config.effectiveDescription, /PowerShell/);
  setHostOs("windows");
  assert.match(m.workers[0].config.effectiveDescription, /Windows PC.*PowerShell, not Linux commands/);
  setHostOs("linux");
  // Running, it shows requests; the tray has no browser to open and offers the way back.
  m.load([f("/a", { name: "Max's PC", containers: false, mode: "direct" })]);
  m.workers[0].status = { phase: "running", contacts: 1 };
  m.workers[0].statusModified = T;
  evaluate(m);
  assert.equal(m.current.screen, "running");
  const labels = JSON.stringify(buildMenu(m, false, new Proxy({}, { get: () => () => {} }) as MenuActions).items);
  assert.doesNotMatch(labels, /Open browser|Remove stale containers|Use a checkout/);
  assert.match(labels, /Mode: no container/);
  assert.match(labels, /Use containers \(look for a runtime again\)/);
  assert.match(labels, /Show worker log/);
});

test("a stopped runtime asks for it before anything else", () => {
  const m = model();
  applyFake(m, "running");
  m.runtimeState = "stopped";
  evaluate(m);
  assert.equal(m.current.screen, "noruntime");
});

test("app.json keeps keys this app doesn't know", () => {
  const m = model();
  m.load([{ home: "/h", folder: "h", is_default: true, project: "p", config: { name: "x", browser: "mac", dock: true }, status: null,
    status_modified: 0, requests: [], approvals: [], paused: false, review: "trust" }]);
  const c = m.current.config;
  c.name = "y";
  c.setNotifies("update", false);
  assert.deepEqual(c.data, { name: "y", browser: "mac", dock: true, notify: { update: false } });
  assert.equal(c.notifies("update"), false);
  assert.equal(c.notifies("paired"), true);
  c.description = "";
  assert.ok(!("description" in c.data));
  assert.equal(c.effectiveDescription, defaultDescription("y"));
  // The Mac app's Dock setting, read and written as it keeps it.
  assert.equal(c.dock, true);
  c.dock = false;
  assert.equal(c.data.dock, false);
  c.data = {};
  assert.equal(c.dock, false);
});

const noop = new Proxy({}, { get: () => () => {} }) as MenuActions;
const labels = (items: { label?: string; separator?: boolean; children?: unknown[] }[]): string[] =>
  items.map((i) => (i.separator ? "----" : i.label ?? ""));

test("the ⋯ menu, and on a Mac the app menu with the same entries", () => {
  const m = model();
  applyFake(m, "running");
  evaluate(m);
  const more = moreMenu(m, noop).map((e) => (e ? e[0] : "----"));
  assert.deepEqual(more, ["Open browser", "----", "Settings…", "Check for updates…", "----", "Show app log", "Show container log", "Open worker folder",
    "Remove stale containers", "----", "Add a worker…", "Add an agent's folder…", "----", "Use a checkout…", "Run without a container…", "----", "Quit myous"]);
  const { items, actions } = appMenu(m, noop);
  const l = labels(items);
  assert.deepEqual(l.slice(0, 5), ["Settings…", "Check for updates…", "----", "Open browser", "----"]);
  assert.equal(items[0].accelerator, "CmdOrCtrl+,");
  assert.ok(!l.includes("Quit myous"), "Quit is the app menu's own (Rust)");
  assert.deepEqual(new Set(more.filter((x) => x !== "----" && x !== "Quit myous")), new Set(l.filter((x) => x !== "----")));
  assert.equal(actions.size, l.filter((x) => x !== "----").length);
  assert.ok([...actions.keys()].every((id) => id.startsWith("app")), "ids apart from the tray's");
});

test("a Mac's made-up worker is Max's Mac", () => {
  const m = model();
  m.os = "macos";
  applyFake(m, "running");
  assert.equal(m.current.name, "Max's Mac");
});

test("the tray menu offers what the state allows", () => {
  const m = model();
  applyFake(m, "running");
  evaluate(m);
  let { items, actions } = buildMenu(m, false, noop);
  const l = labels(items);
  assert.ok(l.includes("Open browser") && l.includes("Pause") && l.includes("Stop"));
  assert.ok(!l.includes("Start"));
  assert.equal(l.filter((x, i) => x === "----" && l[i + 1] === "----").length, 0, "no double separators");
  assert.ok(actions.has("open"));
  // Ids name their entries: a click on a menu built before the last change still runs the right action.
  assert.ok(actions.has("m:Settings…") && actions.has("m:Stop") && actions.has("m:Add a worker…"));
  applyFake(m, "stopped");
  evaluate(m);
  ({ items } = buildMenu(m, false, noop));
  assert.ok(labels(items).includes("Start"));
  applyFake(m, "workers");
  evaluate(m);
  ({ items } = buildMenu(m, false, noop));
  const workers = items.filter((i) => i.children && !i.label?.startsWith("Advanced"));
  assert.equal(workers.length, 2);
  assert.ok(labels(workers[1].children!).includes("Remove…"));
  assert.ok(!labels(workers[0].children!).includes("Remove…"), "the first worker stays");
  ({ items, actions } = buildMenu(m, false, noop));
  assert.ok(actions.has("m:Stop") && actions.has("m:Stop#2") && actions.has("m:Add a worker…#2"), "a second entry with the same label gets #2");
  const l2 = labels(items);
  assert.equal(l2.filter((x, i) => x === "----" && l2[i + 1] === "----").length, 0, "no double separators");
  // Several workers: a WORKERS section with their submenus and Add a worker…, closed by a separator.
  assert.deepEqual(l2.slice(0, 5), ["WORKERS", workers[0].label, workers[1].label, "Add a worker…", "----"]);
  assert.equal(items[0].enabled, false, "a label is grey");
  assert.ok(!l2.some((x) => x.startsWith("AGENTS")), "no agents, no AGENTS label");
  // One worker, no agents: no labels; the worker, then its actions.
  applyFake(m, "running");
  evaluate(m);
  const l3 = labels(buildMenu(m, false, noop, "this PC").items);
  assert.ok(!l3.includes("WORKERS") && !l3.some((x) => x.startsWith("AGENTS")));
  assert.match(l3[0], /^Max's PC · /);
  assert.deepEqual(l3.slice(1, 4), ["   paired with Max's Muse", "----", "Open browser"]);
  // One worker and an agent: both sections, in order, each closed by a separator.
  applyFake(m, "agents");
  evaluate(m);
  ({ items } = buildMenu(m, false, noop, "this PC"));
  const l4 = labels(items);
  assert.deepEqual(l4.slice(0, 8), ["WORKERS", l4[1], "   paired with Max's Muse", "----", "AGENTS ON THIS PC", "Claude Code · 2 contacts", "----", "Open browser"]);
  assert.match(l4[1], /^Max's PC · /);
  assert.ok(items[0].enabled === false && items[4].enabled === false, "the labels are grey");
  assert.ok(items[5].enabled, "an agent opens");
  assert.equal(l4.filter((x, i) => x === "----" && l4[i + 1] === "----").length, 0, "no double separators");
  assert.equal(labels(buildMenu(m, false, noop, "this computer").items)[4], "AGENTS ON THIS COMPUTER");
});

test("words and numbers", () => {
  assert.equal(timeAgo(T - 5, T), "5 s ago");
  assert.equal(timeAgo(T - 125, T), "2 min ago");
  assert.equal(timeAgo(T - 7200, T), "2 h ago");
  assert.equal(timeAgo(T - 86400 * 3, T), "3 d ago");
  assert.equal(timeAgo(T + 10, T), "0 s ago");
  assert.equal(countdown(702), "11:42");
  assert.equal(countdown(-3), "0:00");
  assert.equal(byteCount(999), "999 bytes");
  assert.equal(byteCount(4977), "5.0 KB");
  assert.equal(byteCount(120340), "120.3 KB");
  assert.equal(plural(1, "contact"), "1 contact");
  assert.equal(plural(2, "contact"), "2 contacts");
  assert.equal(dayLabel(T, T), "Today");
  assert.equal(dayLabel(T - 86400, T), "Yesterday");
  assert.ok(newer("v0.6.2", "0.6.1"));
  assert.ok(newer("v0.10.0", "0.9.9"));
  assert.ok(!newer("v0.6.1", "0.6.1"));
  assert.ok(!newer("v9.0.0", "dev"));
  assert.match(pairingMessage({ code: "4821-K7F3QX" }, "Max's PC"), /accept the pairing code 4821-K7F3QX/);
  assert.equal(pairingMessage({ message: "the worker's own" }, "x"), "the worker's own");
});

test("a check: ⟳ for 10 s, then couldn't check; an unknown never replaces an answer", () => {
  const c = new Check();
  assert.equal(c.mark(T), "checking");
  c.begin(T);
  assert.equal(c.mark(T + UNKNOWN_AFTER - 1), "checking");
  assert.equal(c.mark(T + UNKNOWN_AFTER), "unknown");
  // Retry: ⟳ again for another 10 s.
  c.retry(T + 12);
  assert.equal(c.mark(T + 13), "checking");
  c.settle("ok", "", T + 14);
  assert.equal(c.mark(T + 100), "ok");
  // A timeout after an answer keeps the answer, then says it couldn't check.
  c.settle("unknown", "", T + 200);
  assert.equal(c.mark(T + 205), "ok");
  assert.equal(c.mark(T + 210), "unknown");
  c.settle("down", "connection refused", T + 211);
  assert.equal(c.mark(T + 211), "bad");
  assert.equal(c.detail, "connection refused");
  // Beginning again doesn't make an answered check unsure.
  c.begin(T + 300);
  assert.equal(c.mark(T + 400), "bad");
});

test("first launch: the checks decide the card, with a fake clock", () => {
  const m = model();
  let now = T;
  m.now = () => now;
  const files = (config: Record<string, unknown> = {}): WorkerFiles => ({ home: "/h", folder: "h", is_default: true, project: "p", config,
    status: null, status_modified: 0, requests: [], approvals: [], paused: false, review: "trust" });
  m.load([files()]);
  const w = m.current;
  m.checks.runtime.begin(now);
  evaluate(m);
  // Nothing answered yet: Set up, with only the checklist.
  assert.equal(m.runtimeStep(), "checking");
  assert.equal(w.screen, "setup");
  // No answer within 10 s: the form, and "couldn't check" with Retry; never a cross.
  now += UNKNOWN_AFTER;
  evaluate(m);
  assert.equal(m.runtimeStep(), "");
  assert.equal(w.screen, "setup");
  const row = checklist(m, w, "this computer", "https://myoushq.com").find((r) => r.text.startsWith("Container runtime"))!;
  assert.deepEqual(row, { key: "runtime", mark: "–", text: "Container runtime: couldn't check", cls: "todo", retry: "runtime" });
  // Then each answer has its card, and installing one makes the card leave by itself.
  const cases: [string, string, string][] = [["missing", "get", "noruntime"], ["stopped", "start", "noruntime"], ["permission", "permission", "noruntime"],
    ["nocompose", "compose", "noruntime"], ["error", "error", "noruntime"], ["ok", "", "setup"]];
  for (const [state, step, screen] of cases) {
    m.checks.runtime.settle(state, "", now);
    evaluate(m);
    assert.equal(m.runtimeStep(), step, state);
    assert.equal(w.screen, screen, state);
  }
  // A worker already set up never goes back to Set up while the runtime isn't known yet.
  const m2 = model();
  m2.now = () => now;
  m2.load([files({ name: "Max's PC" })]);
  m2.checks.runtime.begin(now);
  evaluate(m2);
  assert.equal(m2.runtimeStep(), "checking");
  assert.equal(m2.current.screen, "stopped");
  // Windows: no runtime, and what Windows says about containers picks the card.
  const win = model();
  win.os = "windows";
  win.now = () => now;
  win.load([files()]);
  win.checks.runtime.settle("missing", "", now);
  win.checks.containers.begin(now);
  assert.equal(win.runtimeStep(), "checking");
  for (const [state, step] of [["wsl-off", "wsl"], ["unsupported", "novirt"], ["ok", "get"]]) {
    win.capability = { state, reason: "virtual", machine: "" };
    assert.equal(win.runtimeStep(), step);
  }
  // Windows' check didn't answer in 10 s: the runtimes, not a dead end.
  win.capability = null;
  now += UNKNOWN_AFTER;
  assert.equal(win.runtimeStep(), "get");
});

const words = (state: string, os = "linux") => {
  const m = model();
  m.os = os;
  applyFake(m, state);
  evaluate(m);
  const sys = os === "windows" ? "this PC" : "this computer";
  return { m, w: m.current, rows: checklist(m, m.current, sys, "https://myoushq.com").map((r) => `${r.mark} ${r.text}${r.retry ? " [Retry]" : ""}`),
    card: runtimeCard(m, m.runtimeStep(), sys) };
};

test("the checklist fills in, in plain words", () => {
  assert.deepEqual(words("setup").rows, ["✓ Containers can run on this computer", "✓ Container runtime: Docker Desktop",
    "○ No agents on this computer (none needed)", "✓ myoushq.com answers"]);
  assert.deepEqual(words("checking").rows, ["⟳ Containers", "⟳ Container runtime", "⟳ Agents on this computer", "⟳ myoushq.com"]);
  assert.deepEqual(words("couldntcheck").rows, ["✓ Containers can run on this computer", "– Container runtime: couldn't check [Retry]",
    "✓ Agents on this computer: Claude Code", "– myoushq.com: couldn't check [Retry]"]);
  assert.deepEqual(words("hubdown").rows.at(-1), "✗ Can't reach myoushq.com");
  assert.equal(words("noruntime").rows[1], "✗ Container runtime: none found");
  assert.equal(words("stoppedengine").rows[1], "✗ Container runtime: Docker Engine isn't running");
  assert.equal(words("nopermission").rows[1], "✗ Container runtime: you can't use Docker Engine yet");
  assert.equal(words("nocompose").rows[1], "✗ Container runtime: Docker Engine is missing a part it needs");
  assert.equal(words("runtimeerror").rows[1], "✗ Container runtime: Docker Desktop reports a problem");
  // Windows: what containers need, said; no runtime asked for where none can run.
  const nowsl = words("nowsl", "windows");
  assert.equal(nowsl.rows[0], "✗ Containers need WSL, which is off");
  const novirt = words("novirt", "windows");
  assert.deepEqual(novirt.rows.slice(0, 2), ["✗ Containers can't run on this PC", "○ No container runtime"]);
  // Without a container: no container rows at all.
  assert.deepEqual(words("direct").rows, ["○ No agents on this computer (none needed)", "✓ myoushq.com answers"]);
  // The normal path never says compose, daemon or socket.
  for (const state of fakeStates) {
    for (const os of ["linux", "windows"]) {
      const { rows, card, m } = words(state, os);
      assert.doesNotMatch(rows.join(" "), /compose|daemon|socket/i, `${state} ${os}`);
      if (m.runtimeStep() === "get" || m.runtimeStep() === "") assert.doesNotMatch(card.title + card.text + card.note, /compose|daemon|socket/i, state);
    }
  }
  assert.equal(hubHost("http://127.0.0.1:8088/"), "127.0.0.1:8088");
});

test("the runtime cards: one fix each, a button only where the app can", () => {
  // Get a container runtime: Install where the tool is, its website always.
  let { card, m } = words("noruntime");
  assert.equal(card.title, "Get a container runtime");
  assert.match(card.text, /run in a sandbox, not on your files/);
  assert.match(card.note, /asks for your password/);
  assert.deepEqual(m.options!.runtimes.map((r) => [r.name, !!r.install]), [["Docker Engine", true], ["Podman", true], ["Docker Desktop", false]]);
  ({ card } = words("noruntime", "windows"));
  assert.match(card.note, /^Install uses winget: Windows asks for an administrator/);
  // Installing: the line on the card, then done or not.
  ({ m } = words("installing"));
  assert.equal(installLine(m), "⟳ Installing Docker Engine… This can take several minutes; the app log shows its progress.");
  m.install = { id: "docker-engine", name: "Docker Engine", running: false, status: 0, lastLine: "" };
  assert.equal(installLine(m), "✓ Docker Engine is installed. Waiting for it to answer…");
  m.install = { id: "docker-engine", name: "Docker Engine", running: false, status: 126, lastLine: "Error executing command as another user: Not authorized" };
  assert.equal(installLine(m), "✗ Installing Docker Engine didn't finish: Error executing command as another user: Not authorized. You can get it from its website instead.");
  // Not running: a desktop runtime opens; Docker Engine starts here with pkexec and systemd, else the line to type.
  ({ card } = words("stoppedruntime"));
  assert.deepEqual([card.title, card.button, card.buttonLabel], ["Start the container runtime", "open", "Open Docker Desktop"]);
  ({ card, m } = words("stoppedengine"));
  assert.deepEqual([card.button, card.buttonLabel, card.note], ["start", "Start Docker Engine", "It runs: systemctl enable --now docker (it asks for your password)."]);
  m.options = { ...m.options!, start: "" };
  card = runtimeCard(m, "start", "this computer");
  assert.deepEqual([card.button, card.lines], ["", ["sudo systemctl enable --now docker"]]);
  assert.match(card.text, /Start it in a terminal/);
  // Not allowed: the group, then log out and in (Windows: docker-users, sign out).
  ({ card } = words("nopermission"));
  assert.equal(card.title, "Allow yourself to use Docker Engine");
  assert.match(card.text, /only the docker group may\. Run this in a terminal, then log out and back in:$/);
  assert.deepEqual(card.lines, ["sudo usermod -aG docker $USER"]);
  ({ card } = words("nopermission", "windows"));
  assert.match(card.text, /docker-users group .* terminal opened as administrator, then sign out and back in:$/);
  assert.deepEqual(card.lines, ['net localgroup docker-users "max" /add']);
  // Compose missing: the package for this distribution, installed here or typed.
  ({ card, m } = words("nocompose"));
  assert.deepEqual([card.title, card.button, card.buttonLabel], ["Install compose", "compose", "Install it"]);
  assert.equal(card.note, "It runs: apt-get install -y docker-compose-v2 (it asks for your password). This card leaves by itself once it's there.");
  m.options = { ...m.options!, compose: "" };
  card = runtimeCard(m, "compose", "this computer");
  assert.deepEqual([card.button, card.lines, card.link], ["", ["sudo apt-get install docker-compose-v2"], "https://docs.docker.com/compose/install/linux/"]);
  // The engine's own error, as it said it.
  ({ card } = words("runtimeerror"));
  assert.deepEqual(card.lines, ["Error response from daemon: Docker Desktop is unable to start"]);
  assert.equal(card.buttonLabel, "Open Docker Desktop");
});

test("the hub's red line, and Needs attention with Try again", () => {
  let { m, w } = words("hubdown");
  assert.equal(hubLine(m, "this computer", "https://myoushq.com"),
    "Can't reach myoushq.com from this computer. You can still start the worker; it registers once it can reach it.");
  assert.equal(w.screen, "setup", "Start stays (decision D)");
  ({ m, w } = words("setup"));
  assert.equal(hubLine(m, "this computer", "https://myoushq.com"), "");
  ({ m, w } = words("failed"));
  assert.equal(w.stateWord, "Needs attention");
  assert.equal(w.attention, "The worker didn't start. The log says why.");
  const c = attentionCard(m, w, "this computer", "https://myoushq.com");
  assert.equal(c.text, "Try again starts it the same way. The last lines it wrote:");
  assert.match(c.log.at(-1)!, /no such host/);
  m.checks.hub.settle("down", "", T);
  assert.match(attentionCard(m, w, "this computer", "https://myoushq.com").text, /^myoushq\.com can't be reached from this computer, so the worker can't register/);
  // Stopped on its own: Try again starts it again.
  w.failure = [];
  m.checks.hub.settle("ok", "", T);
  w.attention = "The worker stopped on its own.";
  assert.equal(attentionCard(m, w, "this computer", "https://myoushq.com").text, "Try again starts it again; the log may say why it stopped.");
});

test("the worker's browser without a container: each row, the toggle, the card", () => {
  const row = (state: string, os = "linux", tweak: (m: Model) => void = () => {}) => {
    const { m, w } = words(state, os);
    tweak(m);
    const r = browserRow(m, w, os === "windows" ? "this PC" : "this computer");
    return { ...r, line: `${r.mark} ${r.text}`, labels: r.buttons.map((b) => b.label) };
  };
  // Everything there: a tick, and what the window is.
  assert.equal(row("direct").line, "✓ Worker browser: Chrome, with Python and Playwright");
  assert.equal(row("direct", "windows").line, "✓ Worker browser: Edge, with Python and Playwright");
  assert.match(row("direct").note, /its own profile: log into sites there for your agent\. It never sees your own browser/);
  // No browser: the download pages, Edge first on Windows.
  assert.equal(row("directnobrowser").line, "○ Worker browser: no browser found");
  assert.deepEqual(row("directnobrowser").labels, ["Get Chrome", "Get Edge"]);
  assert.deepEqual(row("directnobrowser", "windows").labels, ["Get Edge", "Get Chrome"]);
  assert.equal(row("directnobrowser", "windows").buttons[0].url, "https://www.microsoft.com/edge/download");
  // No Python: python.org on Windows; the distribution's line on Linux; why it's needed.
  const win = row("directnopython", "windows");
  assert.equal(win.line, "○ Worker browser: needs Python 3 on this PC");
  assert.deepEqual(win.labels, ["Get Python"]);
  assert.equal(win.buttons[0].url, "https://www.python.org/downloads/windows/");
  assert.match(win.note, /drives the browser with Playwright, which needs Python/);
  const lin = row("directnopython");
  assert.deepEqual([lin.labels, lin.lines], [[], ["sudo apt install python3 python3-venv"]]);
  const noLine = row("directnopython", "linux", (m) => { m.browserCheck!.python_line = ""; });
  assert.deepEqual(noLine.labels, ["Get Python"]);
  assert.equal(row("direct", "windows", (m) => { m.browserCheck = previewBrowserCheck("windows", "oldpython"); }).line,
    "○ Worker browser: needs Python 3.10 or newer on this PC (it has 3.9)");
  // Debian's Python without its venv part.
  const venv = row("directnovenv");
  assert.deepEqual([venv.line, venv.lines], ["○ Worker browser: Python 3.11 is missing its venv part", ["sudo apt install python3-venv"]]);
  // Python there: one button installs Playwright into the app's own environment.
  const pw = row("directnoplaywright");
  assert.deepEqual([pw.line, pw.labels], ["○ Worker browser: install Playwright", ["Install Playwright"]]);
  assert.match(pw.note, /Python environment of myous's own, made with Python 3\.12; nothing else on this computer changes, and no browser is downloaded/);
  // Installing, done, failed.
  assert.deepEqual([row("directinstalling").line, row("directinstalling").labels, row("directinstalling").toggle],
    ["⟳ Installing Playwright… This takes a minute or two; the app log shows its progress.", [], ""]);
  const done = row("directinstalling", "linux", (m) => { m.browserInstall!.running = false; });
  assert.equal(done.line, "✓ Playwright is installed. Checking…");
  const failed = row("directinstalling", "linux", (m) => { Object.assign(m.browserInstall!, { running: false, status: 1, lastLine: "ERROR: No matching distribution found" }); });
  assert.deepEqual([failed.line, failed.cls, failed.labels], ["✗ Installing Playwright didn't finish: ERROR: No matching distribution found", "bad", ["Install Playwright"]]);
  // Checking, then couldn't check (Retry).
  const unsure = (ago: number) => row("direct", "linux", (m) => { m.browserCheck = null; m.checks.browser.state = ""; m.checks.browser.unsureSince = T - ago; });
  assert.equal(unsure(1).line, "⟳ Worker browser");
  assert.deepEqual([unsure(12).line, unsure(12).retry], ["– Worker browser: couldn't check", true]);
  // The toggle: "Continue without a browser" until it's off, then the way back.
  for (const s of ["direct", "directnobrowser", "directnopython", "directnovenv", "directnoplaywright"]) assert.equal(row(s).toggle, "Continue without a browser", s);
  assert.deepEqual([row("directoff").line, row("directoff").toggle], ["○ No worker browser", "Use a worker browser"]);
  const { m, w } = words("direct");
  assert.ok(w.config.hostBrowser && !w.config.browserOff);
  w.config.browserOff = true;
  assert.deepEqual([w.config.data.browser, w.config.hostBrowser, m.browserStep(w)], ["none", false, "off"]);
  w.config.browserOff = false;
  assert.deepEqual([w.config.data.browser, w.config.hostBrowser], ["host", true]);
  // The Mac's values for a container worker don't turn it off; a container worker has no browser here.
  w.config.data.browser = "mac";
  assert.ok(w.config.hostBrowser);
  assert.ok(!words("running").w.config.hostBrowser);

  // Running: the Browser card and the tray's item.
  const card = (state: string, tweak: (w: Worker) => void = () => {}) => {
    const x = words(state);
    tweak(x.w);
    const items = buildMenu(x.m, false, noop).items.map((i) => i.label ?? "");
    return { ...browserCard(x.m, x.w), items, facts: x.w.browserUp };
  };
  const up = card("directrunning");
  assert.deepEqual([up.shown, up.label, up.button, up.facts], [true, "Show browser", "show", true]);
  assert.match(up.text, /Show it to log in or to watch\. Say "Max's PC's browser"/);
  assert.ok(up.items.includes("Show browser") && !up.items.includes("Open browser"));
  const closed = card("directbrowserclosed");
  assert.deepEqual([closed.label, closed.text], ["Open browser", "The browser is closed, so your agent can't use it. Open it again to let it."]);
  assert.ok(closed.items.includes("Open browser"));
  // Started without it, and it has what it needs now: it opens while the worker runs.
  const without = card("directbrowserclosed", (w) => { w.browserLaunch = ""; });
  assert.deepEqual([without.label, without.button, without.items.some((i) => /browser/.test(i))], ["Open browser", "open", false]);
  assert.equal(without.text, "The worker runs without its browser. Open it to give it one; your agent learns about it from its next help.");
  const later = card("directbrowserlater");
  assert.deepEqual([later.label, later.button, later.text, later.row], ["Open browser", "open", without.text, false]);
  assert.doesNotMatch(later.text, /[Rr]estart/);
  const off = card("directrunning", (w) => { w.config.browserOff = true; });
  assert.ok(!off.shown && !off.items.some((i) => /browser/.test(i)));
  const x = words("directbrowserclosed");
  x.w.browserLaunch = "";
  x.m.browserCheck = previewBrowserCheck("linux", "noplaywright");
  assert.deepEqual([browserCard(x.m, x.w).row, browserCard(x.m, x.w).button], [true, ""]);
  const container = card("running");
  assert.deepEqual([container.label, container.items.includes("Open browser")], ["Open browser", true]);

  // The normal path never says CDP, port or DevTools.
  for (const state of fakeStates) {
    for (const os of ["linux", "windows"]) {
      const { m, w } = words(state, os);
      const r = browserRow(m, w, "this PC");
      const c = browserCard(m, w);
      assert.doesNotMatch(`${r.text} ${r.note} ${c.text} ${c.label}`, /CDP|\bport\b|DevTools/i, `${state} ${os}`);
    }
  }
});

test("the default description names the browser only when the worker has one", () => {
  const browser = `Its browser is where I log into sites for you: when I say "the myous browser" or "the worker browser", I mean that one, never your own.`;
  assert.equal(defaultDescription("Max's PC", true, true),
    `My own computer, Max's PC. ${browser} Commands you send run directly on it, as me, not in a container: take care, and ask me before anything you're unsure of.`);
  assert.equal(defaultDescription("Max's PC", true, false),
    "My own computer, Max's PC. Commands you send run directly on it, as me, not in a container: take care, and ask me before anything you're unsure of.");
  setHostOs("windows");
  assert.equal(defaultDescription("Max's PC", true, true),
    `My own Windows PC, Max's PC. ${browser} Commands you send are PowerShell, not Linux commands, and they run directly on it, as me, not in a container: take care, and ask me before anything you're unsure of.`);
  assert.doesNotMatch(defaultDescription("Max's PC", true, false), /browser/);
  setHostOs("linux");
  // A container worker always has its browser.
  assert.equal(defaultDescription("x"), defaultDescription("x", false, false));
  assert.ok(defaultDescription("x").includes(browser));
  // The worker's card follows its browser setting.
  const { w } = words("direct");
  w.config.name = "Max's PC";
  assert.ok(w.config.effectiveDescription.includes(browser));
  w.config.browserOff = true;
  assert.ok(!w.config.effectiveDescription.includes(browser) && /not in a container/.test(w.config.effectiveDescription));
  // Either default counts as the default (so it follows the toggle); the owner's words don't.
  assert.ok(isDefaultDescription(defaultDescription("a", true, true), "a", true) && isDefaultDescription(defaultDescription("a", true, false), "a", true));
  assert.ok(!isDefaultDescription("My laptop.", "a", true));

  // The toggle while the worker runs: it waits for the next start.
  const run = words("directbrowserclosed");
  run.w.browserLaunch = "";
  run.m.browserCheck = previewBrowserCheck("linux", "noplaywright");
  assert.equal(browserRow(run.m, run.w, "this computer").later, "Takes effect at the worker's next start.");
  assert.equal(browserRow(words("direct").m, words("direct").w, "this computer").later, "", "not on Set up before it runs");
});

test("a row that moves on says so: detected, in words, gone after 8 s", () => {
  // Only from a settled problem (✗, ○, –) to another answer with other words.
  assert.equal(isAdvance("⟳", "Container runtime", "✓", "Container runtime: Docker Desktop"), false, "not from checking");
  assert.equal(isAdvance("✗", "Can't reach myoushq.com", "✗", "Can't reach myoushq.com"), false, "not when the words are the same");
  assert.equal(isAdvance("✗", "Can't reach myoushq.com", "✓", "myoushq.com answers"), true, "✗ → ✓");
  assert.equal(isAdvance("○", "Worker browser: needs Python 3 on this PC", "○", "Worker browser: install Playwright"), true, "○ → ○, new words");
  assert.equal(isAdvance("✗", "Can't reach myoushq.com", "–", "myoushq.com: couldn't check"), false, "not to couldn't check");
  assert.equal(isAdvance("✓", "myoushq.com answers", "✗", "Can't reach myoushq.com"), false, "not from ok");

  // The browser row on Windows: Python appears; the row says so for 8 s.
  const m = model();
  m.os = "windows";
  let now = T;
  m.now = () => now;
  applyFake(m, "directnopython", "C:\\Users\\max\\AppData\\Local\\myous\\worker");
  evaluate(m);
  const w = m.current;
  const note = () => browserAdvance(m, w, browserRow(m, w, "this PC"));
  assert.equal(note(), null);
  now += 4;
  assert.equal(note(), null, "nothing changed");
  m.browserCheck = previewBrowserCheck("windows", "noplaywright");
  now += 2;
  assert.equal(browserRow(m, w, "this PC").text, "Worker browser: install Playwright");
  assert.deepEqual(note(), { at: now, line: "Found Python 3.12. Next: Playwright." });
  now += ADVANCE_SHOWS - 1;
  assert.equal(note()?.line, "Found Python 3.12. Next: Playwright.", "still there before 8 s");
  now += 1;
  assert.equal(note(), null, "gone after 8 s");

  // Install Playwright: the row's own states in between aren't advances; ready at the end is.
  m.browserInstall = { id: "playwright", name: "Playwright", running: true, status: 0, lastLine: "" };
  now += 2;
  assert.equal(note(), null, "installing");
  m.browserInstall.running = false;
  now += 60;
  assert.equal(note(), null, "installed, checking");
  m.browserCheck = previewBrowserCheck("windows", "ready");
  m.browserInstall = null;
  now += 2;
  assert.equal(note()?.line, "Playwright is installed. The browser is ready.");

  // The checklist: the runtime appears, then starts; the hub comes back.
  const d = model();
  let t = T;
  d.now = () => t;
  applyFake(d, "noruntime");
  evaluate(d);
  const notes = () => checklist(d, d.current, "this computer", "https://myoushq.com").map((r) => checkAdvance(d, r, "https://myoushq.com")?.line ?? "");
  assert.deepEqual(notes(), ["", "", "", ""]);
  d.checks.runtime.settle("stopped", "", t);
  d.runtimeName = "Docker Desktop";
  t += 2;
  assert.deepEqual(notes(), ["", "Found Docker Desktop. Next: start it.", "", ""]);
  d.checks.runtime.settle("ok", "", t);
  t += 2;
  assert.deepEqual(notes(), ["", "Docker Desktop is running.", "", ""], "a new advance replaces the last");
  // A row last seen long ago (the card wasn't up): its change isn't news.
  d.checks.hub.settle("down", "", t);
  notes();
  d.checks.hub.settle("ok", "", t);
  t += 60;
  assert.equal(notes()[3], "");
  d.checks.hub.settle("down", "", t);
  notes();
  d.checks.hub.settle("ok", "", t);
  t += 2;
  assert.equal(notes()[3], "myoushq.com answers.");

  // The words for each runtime fix, a step back, and the fallback.
  const rt = (from: string, to: string, mark = to === "ok" ? "✓" : "✗") => advanceWords(d, "runtime", { mark: "✗", text: "", step: from }, { mark, text: "Container runtime: Docker Desktop", step: to });
  assert.equal(rt("missing", "ok"), "Found Docker Desktop.");
  assert.equal(rt("stopped", "ok"), "Docker Desktop is running.");
  assert.equal(rt("permission", "ok"), "You can use Docker Desktop now.");
  assert.equal(rt("nocompose", "ok"), "Compose is there.");
  assert.equal(rt("stopped", "missing"), "", "a step back isn't news");
  assert.equal(advanceWords(d, "containers", { mark: "✗", text: "", step: "wsl-off" }, { mark: "✓", text: "", step: "ok" }), "Containers can run now.");
  const br = (from: string, to: string) => advanceWords(m, "browser", { mark: "○", text: "", step: from }, { mark: to === "ready" ? "✓" : "○", text: "", step: to });
  m.browserCheck = previewBrowserCheck("windows", "noplaywright");
  assert.equal(br("nobrowser", "noplaywright"), "Found Edge. Next: Playwright.");
  assert.equal(br("novenv", "noplaywright"), "Python's venv part is there. Next: Playwright.");
  assert.equal(br("noplaywright", "nopython"), "");
  assert.equal(advanceWords(d, "agents", { mark: "–", text: "", step: "" }, { mark: "✓", text: "Agents: x", step: "ok" }), "Done: Agents: x", "the fallback");
});

test("the found preview: the browser row with what was found", () => {
  const m = model();
  applyFake(m, "found");
  evaluate(m);
  const r = browserRow(m, m.current, "this computer");
  assert.equal(`${r.mark} ${r.text}`, "○ Worker browser: install Playwright");
  assert.deepEqual(browserAdvance(m, m.current, r), { at: T - 1, line: "Found Python 3.12. Next: Playwright." });
});

test("a Mac's Set up asks where a container worker's browser runs, as myous for Mac does", () => {
  const m = model();
  m.os = "macos";
  applyFake(m, "setup");
  const w = m.current;
  const labels = () => macBrowserQuestion(m, w).options.map((o) => o.label);
  assert.deepEqual(labels(), ["On this Mac, in Google Chrome (recommended)", "On this Mac, in Microsoft Edge", "In the container"]);
  // Nothing chosen yet: the recommendation on Set up, where it runs (the container) in Settings.
  assert.equal(macBrowserQuestion(m, w).selected, 0);
  assert.equal(macBrowserQuestion(m, w, false).selected, 2);
  assert.equal(macBrowserQuestion(m, w).get, false);
  // myous for Mac's app.json keys.
  w.config.data.browser = "mac";
  w.config.browserApp = "com.microsoft.edgemac";
  assert.equal(macBrowserQuestion(m, w).selected, 1);
  assert.ok(w.config.macBrowser && m.macBrowser(w) && w.config.browserHidden);
  w.config.data.browser_hidden = false;
  assert.equal(w.config.browserHidden, false);
  w.config.browserApp = "com.brave.Browser"; // not installed: the first
  assert.equal(macBrowserQuestion(m, w).selected, 0);
  w.config.data.browser = "container";
  assert.equal(macBrowserQuestion(m, w).selected, 2);
  assert.ok(!w.config.macBrowser);
  assert.match(macBrowserHint(m, true), /^A real browser with its own profile, kept to the worker's folder by a sandbox/);
  // No browser: one line that says why, the container chosen, "Get a browser…".
  m.macBrowsers = { installed: [], why: "no browser it can run is installed" };
  delete w.config.data.browser;
  const q = macBrowserQuestion(m, w);
  assert.deepEqual(q.options.map((o) => [o.label, o.disabled]), [["On this Mac (no browser it can run is installed)", true], ["In the container", false]]);
  assert.equal(q.selected, 1);
  assert.equal(q.get, true);
  assert.match(macBrowserHint(m, false), /install any of Google Chrome, Microsoft Edge, Brave or Chromium/);
  // No sandbox: the browsers aren't offered, and nothing to get.
  m.macBrowsers = { installed: [{ id: "com.google.Chrome", name: "Google Chrome", path: "" }], why: "this macOS can't sandbox it" };
  assert.deepEqual(labels(), ["On this Mac (this macOS can't sandbox it)", "In the container"]);
  assert.equal(macBrowserQuestion(m, w).get, false);
  assert.doesNotMatch(macBrowserHint(m, false), /install/);
});

test("a Mac's browser for a container worker: its card, menu item and attention line; none of it elsewhere", () => {
  const m = model();
  m.os = "macos";
  applyFake(m, "macbrowser");
  evaluate(m);
  const w = m.current;
  assert.ok(m.macBrowser(w) && w.macBrowserUp && !w.browserUp);
  let c = browserCard(m, w);
  assert.equal(c.label, "Show browser");
  assert.match(c.text, /^Your agent uses Google Chrome on this Mac, with its own profile kept to the worker's folder\./);
  w.browserFile = null;
  c = browserCard(m, w);
  assert.match(c.text, /^Starting Google Chrome on this Mac… \(browser\.log/);
  assert.deepEqual(browserItems(w, m.macBrowser(w)), ["Show browser"]);
  assert.equal(w.attention, "");
  m.macBrowsers = { installed: [], why: "no browser it can run is installed" };
  evaluate(m);
  assert.match(w.attention, /^The worker's browser can't run on this Mac: none of Google Chrome/);
  assert.equal(w.attentionButton, "Get a browser…");
  m.macBrowsers = { installed: [], why: "this macOS can't sandbox it" };
  evaluate(m);
  assert.equal(w.attention, "The worker's browser can't run on this Mac: this macOS can't sandbox it. Choose the browser in the container (Settings).");
  assert.equal(w.attentionButton, "Show log");
  // Linux (and Windows): the same files are a container worker with the container's browser.
  const l = model();
  applyFake(l, "macbrowser");
  evaluate(l);
  assert.ok(!l.macBrowser(l.current) && l.macBrowsers === null);
  assert.equal(browserCard(l, l.current).label, "Open browser");
  assert.deepEqual(browserItems(l.current, l.macBrowser(l.current)), ["Open browser"]);
  assert.equal(l.current.attention, "");
});

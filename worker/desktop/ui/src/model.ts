// The app's state without any DOM: every worker, the agents on this
// computer, the runtime, the release check. The window renders it; the
// tests drive it with made-up files. A port of myous for Mac's
// `evaluate`, so the three apps show the same screen for the same files.

import { defaultDescription, dict, newer, num, str } from "./text.js";

export type Screen = "setup" | "noruntime" | "starting" | "pair" | "paired" | "running" | "stopping" | "stopped";
export type Kind = "grey" | "blue" | "green" | "orange" | "red";
type Json = Record<string, unknown>;

/** How long worker.json may go unwritten before the worker counts as stopped. */
export const STALE_AFTER = 15;
/** A worker that ran and went quiet without a Stop counts as stopped once that holds from one look
 * to another at least this many seconds later: one look isn't news (a late write, a slow read). */
export const GONE_AFTER = 4;

/** What the backend reads from one worker folder. */
export interface WorkerFiles {
  home: string;
  folder: string;
  is_default: boolean;
  project: string;
  config: Json | null;
  status: Json | null;
  status_modified: number;
  requests: Json[];
  approvals: Json[];
  paused: boolean;
  review: string;
  /** browser.json: the worker's browser on this computer while it runs. */
  browser?: Json | null;
}

export interface Agent {
  home: string;
  binary: string;
  status: Json | null;
  error: string;
}

/** How long a check may go without an answer before the window says
 * "couldn't check" instead of "⟳". */
export const UNKNOWN_AFTER = 10;

/** One of the first-launch checks (containers possible, the runtime, the
 * agents on this computer, the hub). `state` is the last definite answer
 * ("" before the first); an answer of "unknown" (a timeout) never replaces
 * one, it only starts the clock towards "couldn't check". */
export class Check {
  state = "";
  detail = "";
  /** When it became unsure: the first check started, or an answer was "unknown"; 0 after a definite one. */
  unsureSince = 0;
  /** A check starts: unsure from now if nothing is known yet. */
  begin(now: number) {
    if (this.state === "" && this.unsureSince === 0) this.unsureSince = now;
  }
  settle(state: string, detail: string, now: number) {
    if (state === "unknown" || state === "") {
      if (this.unsureSince === 0) this.unsureSince = now;
      return;
    }
    this.state = state;
    this.detail = detail;
    this.unsureSince = 0;
  }
  /** "Retry": "⟳" again for another while, the check runs again. */
  retry(now: number) {
    this.unsureSince = now;
  }
  /** What the checklist shows: checking (⟳), unknown ("couldn't check"), ok (✓) or bad (✗). */
  mark(now: number): "checking" | "unknown" | "ok" | "bad" {
    if (this.unsureSince !== 0 && now - this.unsureSince >= UNKNOWN_AFTER) return "unknown";
    if (this.state === "") return "checking";
    return this.state === "ok" ? "ok" : "bad";
  }
}

/** What the runtime still needs before a container worker can start:
 * "" nothing (ready, or a worker already set up whose runtime isn't known
 * yet), "checking" (the first answer isn't in), "wsl" and "novirt"
 * (Windows: WSL off, or no virtualization), "get" (no runtime), "start"
 * (installed, not running), "permission", "compose", "error". */
export type RuntimeStep = "" | "checking" | "wsl" | "novirt" | "get" | "start" | "permission" | "compose" | "error";

/** What the Set up card can offer on this computer (install.rs). */
export interface RuntimeOption { id: string; name: string; url: string; install: string }
export interface RuntimeOptions {
  runtimes: RuntimeOption[];
  tool: string;
  start: string;
  start_line: string;
  group_lines: string[];
  compose: string;
  compose_line: string;
  compose_url: string;
}

/** An install (or "start", "compose") the app is running or ran. */
export interface Install { id: string; name: string; running: boolean; status: number; lastLine: string }

/** What the app found for the worker's browser without a container (browser.rs). */
export interface BrowserCheck {
  browser: { id: string; name: string; path: string } | null;
  python: { path: string; version: string; venv: boolean } | null;
  python_old: string;
  playwright: string;
  env_python: string;
  python_line: string;
  venv_line: string;
  python_url: string;
  get: { name: string; url: string }[];
}

/** A Mac: the browsers a worker in a container can have on the Mac (browser.rs's mac::choices). */
export interface MacBrowsers {
  /** In myous for Mac's order; the first is the recommended one. */
  installed: { id: string; name: string; path: string }[];
  /** Why "On this Mac" can't be offered ("" when it can): no browser, or no sandbox. */
  why: string;
}

/** Where to get each browser the Mac's worker browser can be (myous for Mac's "Get a browser…"). */
export const MAC_BROWSER_LINKS: [string, string][] = [
  ["Google Chrome", "https://www.google.com/chrome/"],
  ["Microsoft Edge", "https://www.microsoft.com/edge/download"],
  ["Brave", "https://brave.com/download/"],
  ["Chromium", "https://www.chromium.org/getting-involved/download-chromium/"],
];

/** What the worker's browser without a container still needs: "off" (the
 * owner went on without one), "checking", "unknown", "nobrowser",
 * "nopython" (none, or too old), "novenv" (Debian's python3-venv),
 * "noplaywright", "ready". */
export type BrowserStep = "off" | "checking" | "unknown" | "nobrowser" | "nopython" | "novenv" | "noplaywright" | "ready";

/** Whether this computer can run containers (containers.rs). */
export interface Capability {
  state: "ok" | "wsl-off" | "unsupported" | string;
  reason: string;
  machine: string;
}

/** A worker container, as the runtime describes it (engine.rs's `containers`). */
export interface Container {
  id: string;
  name: string;
  /** The runtime's word: running, restarting, paused, created, exited, dead. */
  state: string;
  /** Running, restarting or paused: removing it stops a worker. */
  running: boolean;
  image: string;
  /** Its compose project ("" none). */
  project: string;
  /** The published image's tag (ghcr.io/myoushq/worker:<version>), "" for another image. */
  version: string;
}

/** What Remove stale containers makes of a container: "stopped" (removed by default), "other"
 * (running, none of this app's workers: removed with All), "older" (running, one of this app's
 * workers on an older version or one that doesn't say: removed only when the owner ticks it),
 * "current" (one of this app's workers on its version: kept). */
export type Staleness = "stopped" | "other" | "older" | "current";

export interface StaleContainer {
  container: Container;
  kind: Staleness;
  /** The app's worker it belongs to (its compose project), if any. */
  worker: Worker | null;
  /** The version it runs: a running worker's own word, else the image tag ("" unknown). */
  version: string;
}

/** Sorts the worker containers for Remove stale containers. A container is the app's worker's when
 * its compose project is that worker's and the worker runs in a container. A running one's version
 * is what the worker reports while it is alive (worker.json may be an earlier run's otherwise), else
 * its image tag; unknown counts as older: a worker that doesn't say predates the version in worker.json. */
export function classifyContainers(containers: Container[], workers: Worker[], appVersion: string): StaleContainer[] {
  return containers.map((c) => {
    const worker = workers.find((w) => c.project !== "" && w.project === c.project && w.config.usesDocker) ?? null;
    const version = (c.running && worker?.alive ? worker.version : "") || c.version;
    let kind: Staleness = "stopped";
    if (c.running) kind = !worker ? "other" : !version || newer(appVersion, version) ? "older" : "current";
    return { container: c, kind, worker, version };
  });
}

/** What Remove removes: the stopped ones; with `all` the other running ones too; and the app's own
 * older workers whose container ids the owner ticked. Never a worker on the app's version. */
export function staleChoice(list: StaleContainer[], all: boolean, ticked: Set<string>): { stopped: Container[]; running: Container[] } {
  const pick = (k: Staleness, on: (e: StaleContainer) => boolean) => list.filter((e) => e.kind === k && on(e)).map((e) => e.container);
  return {
    stopped: pick("stopped", () => true),
    running: [...pick("other", () => all), ...pick("older", (e) => ticked.has(e.container.id))],
  };
}

export const agentName = (a: Agent) => str(a.status?.alias) || a.home.split(/[\\/]/).pop() || a.home;
export const agentContacts = (a: Agent): Json[] => (Array.isArray(a.status?.contact_list) ? (a.status!.contact_list as Json[]) : []);
export function shortNpub(npub: string): string {
  return npub.length < 12 ? npub : `${npub.slice(0, 8)}…${npub.slice(-3)}`;
}

export const notifyKinds: [string, string][] = [
  ["paired", "when a worker pairs (and when its code is about to expire)"],
  ["approval", "when a request waits for my answer"],
  ["refused", "when a request is refused"],
  ["stopped", "when a worker stops on its own"],
  ["update", "when a new version is available"],
];

/** A notification the owner must not miss (a request waits for an answer):
 * it plays a sound, asks for attention and brings the window up. */
export const isUrgent = (kind: string) => kind === "approval";

/** app.json, kept as the object it is so keys the Mac app writes survive. */
export class Config {
  constructor(public data: Json = {}) {}
  get name() { return str(this.data.name); }
  set name(v: string) { this.set("name", v); }
  get description() { return str(this.data.description); }
  set description(v: string) { this.set("description", v); }
  get mode() { return str(this.data.mode); }
  set mode(v: string) { this.set("mode", v); }
  get repo() { return str(this.data.repo); }
  set repo(v: string) { this.set("repo", v); }
  get notifications() { return this.data.notifications !== false; }
  set notifications(v: boolean) { this.data.notifications = v; }
  get autoUpdate() { return this.data.auto_update !== false; }
  set autoUpdate(v: boolean) { this.data.auto_update = v; }
  /** A Mac: a Dock icon as well as the menu bar one (myous for Mac's `dock`; off by default). */
  get dock() { return this.data.dock === true; }
  set dock(v: boolean) { this.data.dock = v; }
  get showAgents() { return this.data.show_agents !== false; }
  set showAgents(v: boolean) { this.data.show_agents = v; }
  get seenPairedAt() { return num(this.data.seen_paired_at); }
  set seenPairedAt(v: number) { this.data.seen_paired_at = v; }
  get seenRequestsAt() { return num(this.data.seen_requests_at); }
  set seenRequestsAt(v: number) { this.data.seen_requests_at = v; }
  get lastUpdateCheck() { return num(this.data.last_update_check); }
  set lastUpdateCheck(v: number) { this.data.last_update_check = v; }
  get skippedVersion() { return str(this.data.skipped_version); }
  get agentHomes(): string[] { return Array.isArray(this.data.agent_homes) ? (this.data.agent_homes as unknown[]).map(str).filter(Boolean) : []; }
  set agentHomes(v: string[]) { this.data.agent_homes = v; }
  notifies(kind: string) { return dict(this.data.notify)?.[kind] !== false; }
  setNotifies(kind: string, on: boolean) { this.data.notify = { ...(dict(this.data.notify) ?? {}), [kind]: on }; }
  /** The owner chose to go on without containers on this computer (the app's setting). */
  get containersOff() { return this.data.containers === false; }
  set containersOff(v: boolean) { if (v) this.data.containers = false; else delete this.data.containers; }
  get isDirect() { return this.mode === "direct"; }
  /** Without a container: the owner went on without a worker browser
   * (`browser: "none"`; "host" or nothing means one, and the Mac's values
   * for a container worker, "mac" and "container", don't turn it off). */
  get browserOff() { return this.data.browser === "none"; }
  set browserOff(v: boolean) { this.data.browser = v ? "none" : "host"; }
  /** The worker runs without a container and wants its browser on this computer. */
  get hostBrowser() { return this.isDirect && !this.browserOff; }
  /** A worker in a container whose browser runs on the Mac (myous for Mac's `browser: "mac"`;
   * "container" or nothing: in the container). Only a Mac acts on it (Model.macBrowser). */
  get macBrowser() { return this.usesDocker && this.data.browser === "mac"; }
  /** Which browser on the Mac: a bundle id (myous for Mac's `browser_app`; "" the first installed). */
  get browserApp() { return str(this.data.browser_app); }
  set browserApp(v: string) { this.set("browser_app", v); }
  /** That browser starts hidden, until Show browser (myous for Mac's `browser_hidden`, on unless set off). */
  get browserHidden() { return this.data.browser_hidden !== false; }
  set browserHidden(v: boolean) { this.data.browser_hidden = v; }
  /** The published image: also what nothing configured means. */
  get isImage() { return this.mode === "image" || (!this.isDirect && !this.repo); }
  get usesDocker() { return !this.isDirect; }
  get effectiveDescription() { return this.description.trim() || defaultDescription(this.name, this.isDirect, !this.isDirect || this.hostBrowser); }
  private set(k: string, v: string) {
    if (v) this.data[k] = v;
    else delete this.data[k];
  }
}

export class Worker {
  config = new Config();
  status: Json | null = null;
  statusModified = 0;
  requests: Json[] = [];
  approvals: Json[] = [];
  paused = false;
  review = "trust";

  // The launch state the app keeps itself.
  launchStage: "" | "pulling" | "creating" = "";
  launchedAt = 0;
  stoppedAt = 0;
  stopDoneAt = 0;
  /** When a Stop it outlived was pressed (still running 90 s on; 0 none): "didn't stop" until 120 s after. */
  stopFailedAt = 0;
  stopping = false;
  wasRunning = false;
  stoppedByUs = false;
  restartAfterStop = false;
  /** The last lines a failed start wrote (compose's output, or worker.log), for the Needs attention card. */
  failure: string[] = [];
  /** Without a container, what the last start said of the browser: "" (none asked for), "starting", or why not. */
  browserLaunch = "";
  browserFile: Json | null = null;

  alive = false;
  screen: Screen = "setup";
  attention = "";
  attentionButton = "";
  /** Evaluated at least once: a line that appears later is news, one there from the start is not. */
  evaluated = false;
  /** When the app last looked at it (Model.evaluate). */
  lookedAt = 0;
  /** When the app looked again after a break longer than STALE_AFTER (the computer slept, or the app
   * was suspended) while the worker ran and wrote: it couldn't write while the computer slept, so it
   * has STALE_AFTER seconds from then, as after a write. */
  wokeAt = 0;
  /** When it first looked stopped after running, without a Stop (0 not); see GONE_AFTER. */
  goneSince = 0;
  /** Its app.json writes in flight, and Model.writesEnded when the last one ended (see Model.load). */
  writesInFlight = 0;
  wroteAt = 0;

  constructor(public home: string, public folder: string, public isDefault: boolean, public project: string) {}

  /** Takes what was read. app.json goes into the same Config, so a reference to it (Settings, the
   * app's settings in the first worker's) stays this worker's; and not while the app writes it, or
   * when the read began before the last write ended (`readFrom`, see Model.load): what the app holds
   * then is newer than what was read. */
  apply(f: WorkerFiles, readFrom = Infinity) {
    if (this.writesInFlight === 0 && this.wroteAt <= readFrom) this.config.data = f.config ?? {};
    this.status = f.status;
    this.statusModified = f.status_modified;
    this.requests = f.requests;
    this.approvals = f.approvals;
    this.paused = f.paused;
    this.review = f.review;
    this.browserFile = dict(f.browser);
  }

  /** Its browser on this computer answers (browser.json, written by the app once the port opens). */
  get browserUp() { return num(this.browserFile?.port) > 0 && str(this.browserFile?.kind) === "host"; }
  /** A Mac: the browser on the Mac of a worker in a container answers (myous for Mac's browser.json, for the container). */
  get macBrowserUp() { return num(this.browserFile?.port) > 0 && str(this.browserFile?.host) === "host.docker.internal"; }

  get name() { return this.config.name || "myous"; }
  get isSetUp() { return !!this.config.name || this.status !== null; }
  get phase() { return str(this.status?.phase); }
  get invite() { return dict(this.status?.invite); }
  get paired() { return dict(this.status?.paired); }
  get contacts() { return num(this.status?.contacts); }
  /** The version the worker reports in worker.json ("" from a worker older than that). */
  get version() { return str(this.status?.version); }
  fresh(now: number) { return this.statusModified > 0 && now - Math.max(this.statusModified, this.wokeAt) < STALE_AFTER; }

  /** Stop pressed, or its container about to be removed: Stopping until it is gone, and then
   * Stopped, not "stopped on its own". */
  beginStop(now: number) {
    this.stopping = true;
    this.stoppedAt = now;
    this.stopDoneAt = 0;
    this.launchedAt = 0;
    this.launchStage = "";
  }

  /** The owner removed its running container (Remove stale containers): it is gone as after Stop,
   * so it shows Stopped with Start, not "stopped on its own". */
  removedByOwner(now: number) {
    this.beginStop(now);
    this.stopDoneAt = now;
  }

  /** The one word the menu and the header show. */
  get stateWord(): string {
    switch (this.screen) {
      case "setup":
      case "noruntime":
        return this.config.name ? "Needs attention" : "Not set up";
      case "starting":
        return "Starting…";
      case "stopping":
        return "Stopping…";
      case "stopped":
        return this.attention ? "Needs attention" : "Stopped";
    }
    if (this.attention && !this.attention.startsWith("myous v")) return "Needs attention";
    if (this.paused) return "Paused";
    if (this.approvals.length > 0) return "Needs attention";
    return "Running";
  }

  get stateKind(): Kind {
    switch (this.stateWord) {
      case "Starting…":
      case "Stopping…":
        return "blue";
      case "Running":
        return "green";
      case "Paused":
        return "orange";
      case "Needs attention":
        return "red";
    }
    return "grey";
  }

  /** The allowed exec in progress, if any (a record a crash left behind doesn't count). */
  runningRequest(now: number): Json | null {
    const r = this.requests[0];
    return r && str(r.op) === "exec" && inProgress(r, now) ? r : null;
  }

  requestInProgress(now: number) {
    const r = this.requests[0];
    return !!r && inProgress(r, now);
  }

  get newRequests() {
    return this.requests.filter((r) => num(r.at) > this.config.seenRequestsAt).length;
  }
}

function inProgress(r: Json, now: number) {
  return str(r.decision) === "allow" && !("done_at" in r) && now - num(r.at) < 700;
}

/** What the agent wants to do, in the owner's words. */
export function verb(op: string) {
  return op === "exec" ? "run" : op === "put" ? "write" : "read";
}

/** The command or path a request or question is about. */
export function what(r: Json) {
  return str(r.cmd) || str(r.path);
}

/** How long a row that moved on shows what was found (its green line). */
export const ADVANCE_SHOWS = 8;
/** A row last noticed this long ago is stale: its next change isn't news (the owner wasn't looking). */
export const SEEN_WITHIN = 10;

/** A checklist row as the window last showed it: its mark, its words,
 * and the state behind them (a check's state, the browser's step). */
export interface Seen { mark: string; text: string; step: string; at: number }

/** A row that moved on: when, and what was found, in words. */
export interface Advance { at: number; line: string }

/** Whether a row moved on: it was settled and not ok (✗, ○ or –), and now
 * says something else that is an answer (ok, or the next thing it needs),
 * not "⟳" or "couldn't check". */
export function isAdvance(prevMark: string, prevText: string, mark: string, text: string): boolean {
  return ["✗", "○", "–"].includes(prevMark) && !["⟳", "–"].includes(mark) && text !== prevText;
}

export class Model {
  workers: Worker[] = [];
  current!: Worker;
  agents: Agent[] | null = null;
  selectedAgent: Agent | null = null;
  runtimeName = "";
  capability: Capability | null = null;
  /** The system: "linux", "windows" or "macos". */
  os = "linux";
  checks = { containers: new Check(), runtime: new Check(), agents: new Check(), hub: new Check(), browser: new Check() };
  options: RuntimeOptions | null = null;
  install: Install | null = null;
  browserCheck: BrowserCheck | null = null;
  /** A Mac: the browsers a worker in a container can have on it (null elsewhere, or not asked yet). */
  macBrowsers: MacBrowsers | null = null;
  /** "Install Playwright", running or done. */
  browserInstall: Install | null = null;
  wslPending = false;   // WSL turned on; Windows needs a restart
  latestRelease = "";
  fake = "";
  ticks = 0;
  notified = new Set<string>();
  /** Each checklist row as last shown (containers, runtime, agents, hub, browser), and the ones that just moved on. Not kept. */
  seen: Record<string, Seen> = {};
  advances: Record<string, Advance> = {};

  now: () => number = () => Date.now() / 1000;
  /** `ask` names a question's worker folder and request id (a Mac puts Allow and Refuse on its notification). */
  notify: (kind: string, title: string, body: string, ask?: { home: string; id: string }) => void = () => {};
  start: (w: Worker) => void = () => {};
  /** A worker has come to need attention (its line went from nothing to something, after its first
   * look): the app brings up its window, which may be closed or behind another app. */
  needsAttention: (w: Worker) => void = () => {};
  windowVisible: () => boolean = () => false;
  writeConfig: (w: Worker) => void = () => {};

  /** app.json writes ended so far (each worker's wroteAt is one of these). */
  writesEnded = 0;

  constructor(public version: string, public productName = "myous") {}

  /** One write of a worker's app.json (`write` does it): while it runs and after it ends, a read that
   * began earlier doesn't replace what the app holds (see load). */
  async writing(w: Worker, write: () => Promise<void>) {
    w.writesInFlight++;
    try {
      await write();
    } finally {
      w.writesInFlight--;
      w.wroteAt = ++this.writesEnded;
    }
  }

  /** The runtime's last definite state (engine.rs: ok, missing, stopped, permission, nocompose, error). */
  get runtimeState() { return this.checks.runtime.state; }
  set runtimeState(v: string) { this.checks.runtime.state = v; }

  /** What the runtime still needs, from the checks (see RuntimeStep). */
  runtimeStep(now = this.now()): RuntimeStep {
    const rt = this.checks.runtime;
    switch (rt.state) {
      case "":
        return rt.mark(now) === "unknown" ? "" : "checking";
      case "ok":
        return "";
      case "missing": {
        if (this.os !== "windows") return "get";
        const c = this.capability;
        if (!c) return this.checks.containers.mark(now) === "unknown" ? "get" : "checking";
        return c.state === "wsl-off" ? "wsl" : c.state === "unsupported" ? "novirt" : "get";
      }
      case "stopped":
        return "start";
      case "permission":
        return "permission";
      case "nocompose":
        return "compose";
      default:
        return "error";
    }
  }

  /** What the worker's browser without a container still needs (see BrowserStep). */
  browserStep(w: Worker, now = this.now()): BrowserStep {
    if (w.config.browserOff) return "off";
    const c = this.browserCheck;
    if (!c) return this.checks.browser.mark(now) === "unknown" ? "unknown" : "checking";
    if (!c.browser) return "nobrowser";
    if (c.playwright) return "ready";
    if (!c.python) return "nopython";
    if (!c.python.venv) return "novenv";
    return "noplaywright";
  }

  /** Notes a row as the window shows it now and returns the advance to
   * show under it (null none): set when the row moves on (see isAdvance;
   * `words` says what was found, from the row before, "" if it isn't news),
   * gone after ADVANCE_SHOWS seconds or once the row changes again. While
   * `hold` (an install the owner started is in flight) the row before is
   * kept, so its end is compared with what the owner saw before it. */
  noteAdvance(key: string, cur: Omit<Seen, "at">, words: (prev: Seen) => string, now = this.now(), hold = false): Advance | null {
    const prev = this.seen[key];
    if (prev && cur.text !== prev.text) {
      delete this.advances[key];
      if (!hold && now - prev.at <= SEEN_WITHIN && isAdvance(prev.mark, prev.text, cur.mark, cur.text)) {
        const line = words(prev);
        if (line) this.advances[key] = { at: now, line };
      }
    }
    if (hold && prev) prev.at = now;
    else this.seen[key] = { ...cur, at: now };
    return this.advance(key, now);
  }

  /** A row's advance while it lasts (null none, or after ADVANCE_SHOWS seconds). */
  advance(key: string, now = this.now()): Advance | null {
    const a = this.advances[key];
    if (!a) return null;
    if (now - a.at >= ADVANCE_SHOWS) {
      delete this.advances[key];
      return null;
    }
    return a;
  }

  /** Takes what the backend read: new folders join, known ones keep their launch state. `readFrom`
   * is writesEnded when the read began: a worker's app.json written after that (or being written)
   * stays as the app holds it, as the read may be older than the write. */
  load(files: WorkerFiles[], readFrom = Infinity) {
    const known = new Map(this.workers.map((w) => [w.home, w]));
    this.workers = files.map((f) => {
      const w = known.get(f.home) ?? new Worker(f.home, f.folder, f.is_default, f.project);
      w.apply(f, readFrom);
      return w;
    });
    if (!this.current || !this.workers.includes(this.current)) this.current = this.workers[0];
    // No containers on this computer: every worker runs without one.
    if (this.appConfig.containersOff) {
      for (const w of this.workers) {
        if (w.config.isDirect) continue;
        w.config.mode = "direct";
        if (!this.fake) this.writeConfig(w);
      }
    }
  }

  /** A worker in a container whose browser runs on this Mac (never on Linux or Windows). */
  macBrowser(w: Worker) { return this.os === "macos" && w.config.macBrowser; }

  /** The first worker's app.json holds the app's settings. */
  get appConfig() { return this.workers[0].config; }

  /** Decides one worker's screen and attention line from its files; notifications for what changed. */
  evaluate(w: Worker) {
    const now = this.now();
    // A break in the app's own looking: a worker that ran and wrote until then may have been asleep
    // with the computer, so worker.json's age says nothing yet (see wokeAt). Not one that was quiet
    // already: a worker that died while the app's timer was slowed is found on the next look.
    if (w.lookedAt !== 0 && now - w.lookedAt > STALE_AFTER && w.alive && w.lookedAt - w.statusModified < STALE_AFTER) w.wokeAt = now;
    w.lookedAt = now;
    const phase = w.phase;
    let fresh = w.fresh(now);
    // A worker that ran and went quiet without a Stop: running until that holds GONE_AFTER seconds on.
    if (!fresh && w.alive && !w.stopping) {
      if (w.goneSince === 0) w.goneSince = now;
      if (now - w.goneSince < GONE_AFTER) fresh = true;
    } else w.goneSince = 0;
    // Stop finished and the worker has written nothing since: it is gone, until the next start.
    if (fresh && w.stopDoneAt !== 0 && w.statusModified < w.stopDoneAt) fresh = false;
    const alive = fresh && (phase === "running" || phase === "paused" || phase === "");
    const booting = fresh && (phase === "starting" || phase === "browser" || phase === "registering");
    const launching = w.launchStage !== "" || (w.launchedAt !== 0 && now - w.launchedAt < 90 && !alive);
    const step = w.config.usesDocker ? this.runtimeStep(now) : "";
    const runtimeProblem = step !== "" && step !== "checking";
    const stoppingFor = w.stopping && w.stoppedAt !== 0 ? now - w.stoppedAt : 0;
    if (alive && !w.stopping) w.launchedAt = 0;
    if (alive && stoppingFor > 90) {
      w.stopping = false;
      w.stopFailedAt = w.stoppedAt;
      w.stoppedAt = 0;
    }
    if (!alive) w.stopFailedAt = 0;
    // An install from before names were a setting: take the worker's.
    if (!w.config.name && str(w.status?.alias) && !this.fake) {
      w.config.name = str(w.status?.alias);
      this.writeConfig(w);
    }

    let screen: Screen;
    let attention = "", button = "";
    if (!w.config.name) {
      screen = runtimeProblem ? "noruntime" : "setup";
    } else if (runtimeProblem) {
      screen = "noruntime";
    } else if (w.stopping && (alive || booting)) {
      screen = "stopping";
    } else if (launching || booting) {
      screen = "starting";
    } else if (alive) {
      const invite = w.invite, paired = w.paired;
      if (w.contacts === 0 && str(invite?.code)) screen = "pair";
      else if (paired && num(paired.at) > w.config.seenPairedAt) screen = "paired";
      else screen = "running";
      if (w.stopFailedAt !== 0 && now - w.stopFailedAt < 120) {
        attention = "The worker didn't stop. The log says why.";
        button = "Show log";
      }
      // A Mac: its browser on the Mac can't run (myous for Mac's words).
      const why = this.macBrowser(w) ? this.macBrowsers?.why ?? "" : "";
      if (why && !attention) {
        const sandbox = why.includes("sandbox");
        attention = sandbox
          ? `The worker's browser can't run on this Mac: ${why}. Choose the browser in the container (Settings).`
          : "The worker's browser can't run on this Mac: none of Google Chrome, Microsoft Edge, Brave or Chromium is installed. Any one will do (all free); or choose the browser in the container (Settings).";
        button = sandbox ? "Show log" : "Get a browser…";
      }
    } else {
      screen = "stopped";
      if (w.stopping) {
        w.stopping = false;
        w.stoppedAt = 0;
        w.stoppedByUs = true;
      }
      if (phase.startsWith("error") && fresh) {
        attention = phase.replace(/^error:?\s*/, "");
        button = "Show log";
      } else if (w.wasRunning && !w.stoppedByUs) {
        attention = "The worker stopped on its own.";
        button = "Start";
        this.notifyOnce(w, "stopped", "myous worker stopped", "The worker stopped on its own. Open myous to start it again.");
      } else if (w.launchedAt !== 0 && !launching) {
        attention = "The worker didn't start. The log says why.";
        button = "Show log";
      }
    }
    if (alive) {
      w.wasRunning = true;
      w.stoppedByUs = false;
    }
    if (screen !== "stopped") w.wasRunning = alive;
    if (screen === "stopped" && w.restartAfterStop && !this.fake) {
      w.restartAfterStop = false;
      w.attention = w.attentionButton = "";
      w.screen = screen;
      w.alive = false;
      this.start(w);
      return;
    }
    w.screen = screen;
    w.alive = alive;
    const news = attention !== "" && w.attention === "" && w.evaluated && !attention.startsWith("myous v");
    w.attention = attention;
    w.attentionButton = button;
    w.evaluated = true;
    if (news) this.needsAttention(w);

    if (screen === "pair") {
      const invite = w.invite;
      const left = num(invite?.expires_at) - now;
      if (left > 0 && left < 120 && !this.windowVisible()) {
        this.notifyOnce(w, `expiring-${str(invite?.code)}`, `Still waiting for an agent to pair with ${w.name}`,
          `The code ${str(invite?.code)} expires in ${Math.floor(left / 60) + 1} min; the worker then makes a new one. Open myous to copy the message.`);
      }
    }
    if (screen === "paired" || screen === "running") this.notifyPaired(w);
    this.notifyRefusals(w);
    this.notifyApproval(w);
  }

  notifyOnce(w: Worker, key: string, title: string, body: string, ask?: { home: string; id: string }) {
    let kind = "stopped";
    if (key.startsWith("ask-")) kind = "approval";
    else if (key.startsWith("update-")) kind = "update";
    else if (key.startsWith("refused-")) kind = "refused";
    else if (key.startsWith("paired-") || key.startsWith("expiring-")) kind = "paired";
    if (kind !== "approval" && kind !== "update") key = `${w.folder}/${key}`;
    if (this.notified.has(key)) return;
    this.notified.add(key);
    const app = this.appConfig;
    if (this.fake || !app.notifications || !app.notifies(kind)) return;
    this.notify(kind, title, body, ask);
  }

  private notifyPaired(w: Worker) {
    const p = w.paired;
    const at = num(p?.at);
    if (at === 0 || at < this.now() - 600) return;
    this.notifyOnce(w, `paired-${Math.round(at)}`, `${str(p?.alias) || "Your agent"} is paired with ${w.name}`,
      `Verification code ${str(p?.verify) || "?"}. Your agent shows the same number.`);
  }

  private notifyRefusals(w: Worker) {
    const now = this.now();
    for (const r of w.requests) {
      if (str(r.decision) !== "refuse" || now - num(r.at) > 120) continue;
      if (this.ticks < 2) {
        // From before launch: not news.
        this.notified.add(`${w.folder}/refused-${str(r.id)}`);
        continue;
      }
      this.notifyOnce(w, `refused-${str(r.id)}`, `${w.name} refused a request`,
        `${what(r) || str(r.op)}: ${str(r.reason) || "refused by the review hook"}`);
    }
  }

  private notifyApproval(w: Worker) {
    const q = w.approvals[0];
    if (!q) return;
    this.notifyOnce(w, `ask-${str(q.id)}`, `${str(q.alias) || "Your agent"} wants to ${verb(str(q.op))} on ${w.name}`, what(q),
      { home: w.home, id: str(q.id) });
  }

  /** The tray colour: the worst state over every worker. */
  get worstKind(): Kind {
    const order: Kind[] = ["red", "blue", "orange", "green", "grey"];
    let worst: Kind = "grey";
    for (const w of this.workers) if (order.indexOf(w.stateKind) < order.indexOf(worst)) worst = w.stateKind;
    return worst;
  }

  get newRequests() {
    return this.workers.reduce((n, w) => n + w.newRequests, 0);
  }
}

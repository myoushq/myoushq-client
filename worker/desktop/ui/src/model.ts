// The app's state without any DOM: every worker, the agents on this
// computer, the runtime, the release check. The window renders it; the
// tests drive it with made-up files. A port of myous for Mac's
// `evaluate`, so the three apps show the same screen for the same files.

import { defaultDescription, dict, num, str } from "./text.js";

export type Screen = "setup" | "noruntime" | "starting" | "pair" | "paired" | "running" | "stopping" | "stopped";
export type Kind = "grey" | "blue" | "green" | "orange" | "red";
type Json = Record<string, unknown>;

/** How long worker.json may go unwritten before the worker counts as stopped. */
export const STALE_AFTER = 15;

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

  constructor(public home: string, public folder: string, public isDefault: boolean, public project: string) {}

  apply(f: WorkerFiles) {
    this.config = new Config(f.config ?? {});
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

  get name() { return this.config.name || "myous"; }
  get isSetUp() { return !!this.config.name || this.status !== null; }
  get phase() { return str(this.status?.phase); }
  get invite() { return dict(this.status?.invite); }
  get paired() { return dict(this.status?.paired); }
  get contacts() { return num(this.status?.contacts); }
  fresh(now: number) { return this.statusModified > 0 && now - this.statusModified < STALE_AFTER; }

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
  notify: (kind: string, title: string, body: string) => void = () => {};
  start: (w: Worker) => void = () => {};
  windowVisible: () => boolean = () => false;
  writeConfig: (w: Worker) => void = () => {};

  constructor(public version: string, public productName = "myous") {}

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

  /** Takes what the backend read: new folders join, known ones keep their launch state. */
  load(files: WorkerFiles[]) {
    const known = new Map(this.workers.map((w) => [w.home, w]));
    this.workers = files.map((f) => {
      const w = known.get(f.home) ?? new Worker(f.home, f.folder, f.is_default, f.project);
      w.apply(f);
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

  /** The first worker's app.json holds the app's settings. */
  get appConfig() { return this.workers[0].config; }

  /** Decides one worker's screen and attention line from its files; notifications for what changed. */
  evaluate(w: Worker) {
    const now = this.now();
    const phase = w.phase;
    let fresh = w.fresh(now);
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
      w.stoppedAt = 0;
    }
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
      if (w.stoppedAt !== 0 && now - w.stoppedAt > 90 && now - w.stoppedAt < 120) {
        attention = "The worker didn't stop. The log says why.";
        button = "Show log";
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
    w.attention = attention;
    w.attentionButton = button;

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

  notifyOnce(w: Worker, key: string, title: string, body: string) {
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
    this.notify(kind, title, body);
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
    this.notifyOnce(w, `ask-${str(q.id)}`, `${str(q.alias) || "Your agent"} wants to ${verb(str(q.op))} on ${w.name}`, what(q));
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

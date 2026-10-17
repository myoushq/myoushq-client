// The first launch's words, without a window: the checklist at the top of
// the Set up card, the card for what the runtime still needs, the hub's red
// line and the Needs attention card. Pure, so the tests read every one.
// The Mac app's wording where it has the same thing.

import { Advance, Check, Model, RuntimeStep, Seen, Worker } from "./model.js";

export type CheckName = "containers" | "runtime" | "agents" | "hub" | "browser";

/** One line of the checklist: its check, its mark, its words, its style,
 * and which check its Retry runs again (only on "couldn't check"). */
export interface Row { key: CheckName; mark: string; text: string; cls: "" | "bad" | "todo"; retry?: CheckName }

/** The host the hub check asks, for the screens ("myoushq.com"). */
export function hubHost(hub: string): string {
  return hub.replace(/^https?:\/\//, "").replace(/\/.*$/, "") || "myoushq.com";
}

const DESKTOP_RUNTIMES = ["Docker Desktop", "Rancher Desktop", "OrbStack"];
/** A runtime that is an app the owner opens (the rest are system services). */
export const isDesktopRuntime = (name: string) => DESKTOP_RUNTIMES.includes(name);

/** The checklist for a worker's Set up card, filling in as the checks answer. */
export function checklist(m: Model, w: Worker, sys: string, hub: string, now = m.now()): Row[] {
  const rows: Row[] = [];
  const row = (c: Check, name: CheckName, label: string, ok: string, bad: string, badCls: "bad" | "todo" = "bad"): Row => {
    switch (c.mark(now)) {
      case "checking": return { key: name, mark: "⟳", text: label, cls: "" };
      case "unknown": return { key: name, mark: "–", text: `${label}: couldn't check`, cls: "todo", retry: name };
      case "ok": return { key: name, mark: "✓", text: ok, cls: "" };
      default: return { key: name, mark: badCls === "bad" ? "✗" : "○", text: bad, cls: badCls };
    }
  };
  if (w.config.usesDocker) {
    const cap = m.capability;
    const capText = cap?.state === "wsl-off" ? "Containers need WSL, which is off" : `Containers can't run on ${sys}`;
    rows.push(row(m.checks.containers, "containers", "Containers", `Containers can run on ${sys}`, capText));
    const rt = m.checks.runtime, name = m.runtimeName || "Docker";
    const impossible = m.os === "windows" && cap?.state === "unsupported";
    const runtimeBad: Record<string, string> = {
      missing: impossible ? "No container runtime" : "Container runtime: none found",
      stopped: `Container runtime: ${name} isn't running`,
      permission: `Container runtime: you can't use ${name} yet`,
      nocompose: `Container runtime: ${name} is missing a part it needs`,
    };
    rows.push(row(rt, "runtime", "Container runtime", `Container runtime: ${name}`,
      runtimeBad[rt.state] ?? `Container runtime: ${name} reports a problem`, impossible ? "todo" : "bad"));
  }
  if (m.appConfig.showAgents) {
    const names = agentNames(m);
    const agents = row(m.checks.agents, "agents", `Agents on ${sys}`, `Agents on ${sys}: ${names.join(", ")}`, "");
    rows.push(agents.mark === "✓" && names.length === 0 ? { key: "agents", mark: "○", text: `No agents on ${sys} (none needed)`, cls: "todo" } : agents);
  }
  const host = hubHost(hub);
  rows.push(row(m.checks.hub, "hub", host, `${host} answers`, `Can't reach ${host}`));
  return rows;
}

/** The agents found on this computer, by name. */
function agentNames(m: Model): string[] {
  return (m.agents ?? []).filter((a) => !a.error).map((a) => str(a.status?.alias) || a.home.split(/[\\/]/).pop() || a.home);
}

function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

/** The red line while the hub can't be reached (decision D: Start stays). */
export function hubLine(m: Model, sys: string, hub: string): string {
  if (m.checks.hub.state !== "down") return "";
  return `Can't reach ${hubHost(hub)} from ${sys}. You can still start the worker; it registers once it can reach it.`;
}

/** A runtime card: its title, its text, the command lines it shows (to
 * type in a terminal, with Copy), and the button that fixes it, if the app
 * can ("open": the runtime's app; "start", "compose": run here). */
export interface RuntimeCard { title: string; text: string; lines: string[]; button: "" | "open" | "start" | "compose"; buttonLabel: string; note: string; link: string }

export function runtimeCard(m: Model, step: RuntimeStep, sys: string): RuntimeCard {
  const name = m.runtimeName || "Docker";
  const o = m.options;
  const card: RuntimeCard = { title: "Set up", text: "", lines: [], button: "", buttonLabel: "", note: "", link: "" };
  const desktop = isDesktopRuntime(name);
  switch (step) {
    case "get":
      card.title = "Get a container runtime";
      card.text = `The worker runs in a container, so your agent's commands run in a sandbox, not on your files. Any of these will do.`;
      card.note = o?.tool === "winget"
        ? "Install uses winget: Windows asks for an administrator, and installing accepts the maker's licence for you. Its progress is in the app log."
        : o?.tool === "Homebrew"
          ? "Install uses Homebrew. Its progress is in the app log."
          : o?.tool === "pkexec"
            ? "Install asks for your password, and for Docker Engine also starts it and adds you to the docker group. Its progress is in the app log."
            : "";
      break;
    case "start":
      card.title = "Start the container runtime";
      if (desktop) {
        card.text = `${name} is installed but not running. Open it and wait for it to finish starting; this card leaves by itself.`;
        card.button = "open";
        card.buttonLabel = `Open ${name}`;
      } else if (o?.start) {
        card.text = `${name} is installed but not running. Start it here; this card leaves by itself.`;
        card.button = "start";
        card.buttonLabel = `Start ${name}`;
        card.note = `It runs: ${o.start} (it asks for your password).`;
      } else {
        card.text = `${name} is installed but not running. Start it in a terminal; this card leaves by itself.`;
        card.lines = o?.start_line ? [o.start_line] : [];
      }
      break;
    case "permission":
      card.title = `Allow yourself to use ${name}`;
      if (m.os === "windows") {
        card.text = `You're not allowed to use ${name} yet: Windows lets only the docker-users group use it. Run this in a terminal opened as administrator, then sign out and back in:`;
      } else {
        card.text = `You're not allowed to use ${name} yet: only the docker group may. Run this in a terminal, then log out and back in:`;
      }
      card.lines = o?.group_lines ?? [];
      card.note = "This card leaves by itself once you can.";
      break;
    case "compose":
      card.title = "Install compose";
      card.text = `${name} works, but its compose part is missing: the worker starts its containers with it.`;
      if (o?.compose) {
        card.button = "compose";
        card.buttonLabel = "Install it";
        card.note = `It runs: ${o.compose} (it asks for your password). This card leaves by itself once it's there.`;
      } else if (o?.compose_line) {
        card.text += " Install it in a terminal; this card leaves by itself once it's there:";
        card.lines = [o.compose_line];
      } else {
        card.text += " Install it as its page says; this card leaves by itself once it's there.";
      }
      card.link = o?.compose_url ?? "";
      break;
    case "error":
      card.title = "Container runtime";
      card.text = `${name} answered with an error. If it's still starting, wait a moment; this card leaves by itself once it answers.`;
      card.lines = m.checks.runtime.detail ? [m.checks.runtime.detail] : [];
      if (desktop) {
        card.button = "open";
        card.buttonLabel = `Open ${name}`;
      }
      break;
    default:
      break;
  }
  return card;
}

/** What the get card says while the app installs something, or after. */
export function installLine(m: Model): string {
  const i = m.install;
  if (!i) return "";
  if (i.running) return i.id === "start" ? `⟳ Starting ${i.name}…` : `⟳ Installing ${i.name}… This can take several minutes; the app log shows its progress.`;
  if (i.status === 0) return i.id === "start" ? `✓ ${i.name} started. Waiting for it to answer…` : `✓ ${i.name} is installed. Waiting for it to answer…`;
  const why = i.lastLine ? `: ${i.lastLine}` : ` (exit ${i.status})`;
  return i.id === "start"
    ? `✗ ${i.name} didn't start${why}`
    : `✗ Installing ${i.name} didn't finish${why}. You can get it from its website instead.`;
}

/** The Needs attention card under the red line: why, in a sentence, and the last lines it wrote. */
export function attentionCard(m: Model, w: Worker, sys: string, hub: string): { text: string; log: string[] } {
  if (m.checks.hub.state === "down") {
    return { text: `${hubHost(hub)} can't be reached from ${sys}, so the worker can't register. Try again once ${sys} is online.`, log: w.failure };
  }
  if (w.failure.length) return { text: "Try again starts it the same way. The last lines it wrote:", log: w.failure };
  if (w.attention === "The worker stopped on its own.") return { text: "Try again starts it again; the log may say why it stopped.", log: [] };
  return { text: "Try again starts it the same way; the log says what went wrong.", log: [] };
}

/** A button on the browser row: "install" (Playwright, here), "get"
 * (a download page: a browser, Python). */
export interface RowButton { action: "install" | "get"; label: string; url: string; primary: boolean }

/** The worker browser's row without a container: its line, what fixes
 * it (buttons, or a line to type), a hint, and the link that turns the
 * browser off or on again. */
export interface BrowserRow { mark: string; text: string; cls: "" | "bad" | "todo"; buttons: RowButton[]; lines: string[]; note: string; toggle: string; retry: boolean;
  /** Next to the toggle while the worker runs: the change waits for its next start. */
  later: string }

export function browserRow(m: Model, w: Worker, sys: string, now = m.now()): BrowserRow {
  const r = browserRowFor(m, w, sys, now);
  if (r.toggle && w.alive) r.later = "Takes effect at the worker's next start.";
  return r;
}

function browserRowFor(m: Model, w: Worker, sys: string, now: number): BrowserRow {
  const r: BrowserRow = { mark: "○", text: "", cls: "todo", buttons: [], lines: [], note: "", toggle: "Continue without a browser", retry: false, later: "" };
  const c = m.browserCheck;
  const step = m.browserStep(w, now);
  const i = m.browserInstall;
  const why = "Your agent drives the browser with Playwright, which needs Python.";
  switch (step) {
    case "off":
      return { ...r, text: "No worker browser", toggle: "Use a worker browser" };
    case "checking":
      return { ...r, mark: "⟳", text: "Worker browser", cls: "" };
    case "unknown":
      return { ...r, mark: "–", text: "Worker browser: couldn't check", retry: true };
    case "ready":
      return { ...r, mark: "✓", cls: "", text: `Worker browser: ${c!.browser!.name}, with Python and Playwright`,
        note: `A window of its own on ${sys}, with its own profile: log into sites there for your agent. It never sees your own browser or its logins.` };
    case "nobrowser":
      r.text = "Worker browser: no browser found";
      r.buttons = (c?.get ?? []).map((g, n) => ({ action: "get", label: `Get ${g.name}`, url: g.url, primary: n === 0 }));
      r.note = `It runs Chrome, Edge or Chromium with a profile of its own. Install one and this line ticks by itself.`;
      return r;
    case "nopython": {
      const old = c?.python_old ?? "";
      r.text = old ? `Worker browser: needs Python 3.10 or newer on ${sys} (it has ${old})` : `Worker browser: needs Python 3 on ${sys}`;
      if (c?.python_line && m.os === "linux") {
        r.lines = [c.python_line];
        r.note = `${why} Install it in a terminal; this line ticks by itself once it's there.`;
      } else {
        r.buttons = [{ action: "get", label: "Get Python", url: c?.python_url || "https://www.python.org/downloads/", primary: true }];
        r.note = `${why} Install it from python.org; this line ticks by itself once it's there.`;
      }
      return r;
    }
    case "novenv":
      r.text = `Worker browser: Python ${c!.python!.version} is missing its venv part`;
      if (c?.venv_line) r.lines = [c.venv_line];
      r.note = "myous makes a Python environment of its own for Playwright, which needs that part. Install it in a terminal; this line ticks by itself.";
      return r;
    case "noplaywright":
      if (i?.running) return { ...r, mark: "⟳", cls: "", text: "Installing Playwright… This takes a minute or two; the app log shows its progress.", toggle: "" };
      if (i && i.status === 0) return { ...r, mark: "✓", cls: "", text: "Playwright is installed. Checking…", toggle: "" };
      r.text = i ? `Installing Playwright didn't finish${i.lastLine ? `: ${i.lastLine}` : ` (exit ${i.status})`}` : "Worker browser: install Playwright";
      if (i) { r.mark = "✗"; r.cls = "bad"; }
      r.buttons = [{ action: "install", label: "Install Playwright", url: "", primary: true }];
      r.note = `It goes into a Python environment of myous's own, made with Python ${c!.python!.version}; nothing else on ${sys} changes, and no browser is downloaded.`;
      return r;
  }
}

/** The Browser card of a running worker: its words and its button ("show":
 * bring it forward, "open": start it, again or for the first time while
 * the worker runs; "" none). Without a container the card shows only while
 * the browser is wanted. */
export function browserCard(m: Model, w: Worker): { shown: boolean; text: string; button: "" | "show" | "open"; label: string; row: boolean } {
  const say = `Say "${w.name}'s browser" to your agent when you mean this one.`;
  if (m.macBrowser(w)) {
    // A Mac, a worker in a container with its browser on the Mac: myous for Mac's card.
    const app = str(w.browserFile?.app) || macBrowserName(m, w);
    return w.macBrowserUp
      ? { shown: true, text: `Your agent uses ${app} on this Mac, with its own profile kept to the worker's folder. Log into sites there; it is you at the keyboard, so sites behave. ${say}`, button: "show", label: "Show browser", row: false }
      : { shown: true, text: `Starting ${app} on this Mac… (browser.log in the worker folder says why if it doesn't).`, button: "show", label: "Show browser", row: false };
  }
  if (!w.config.isDirect) {
    return { shown: true, text: `Your agent can use sites you're logged into here. Open it to log in or to watch. ${say}`, button: "show", label: "Open browser", row: false };
  }
  if (!w.config.hostBrowser) return { shown: false, text: "", button: "", label: "", row: false };
  if (w.browserUp) {
    return { shown: true, text: `Your agent can use sites you're logged into here. Show it to log in or to watch. ${say}`, button: "show", label: "Show browser", row: false };
  }
  if (w.browserLaunch === "starting") {
    return { shown: true, text: "The browser is closed, so your agent can't use it. Open it again to let it.", button: "open", label: "Open browser", row: false };
  }
  // Started without it: what it still needs, then it can open now (the
  // worker reads browser.json at each help; no restart).
  if (m.browserStep(w) !== "ready") return { shown: true, text: "The worker runs without its browser until it has what it needs:", button: "", label: "", row: true };
  return { shown: true, text: "The worker runs without its browser. Open it to give it one; your agent learns about it from its next help.", button: "open", label: "Open browser", row: false };
}

/** A Mac: the browser on the Mac a worker in a container runs (its `browser_app`, else the first installed). */
export function macBrowserName(m: Model, w: Worker): string {
  const all = m.macBrowsers?.installed ?? [];
  return (all.find((b) => b.id === w.config.browserApp) ?? all[0])?.name ?? "the browser";
}

/** A Mac's Set up, for a worker in a container: "Where should its browser run?", as myous for Mac
 * asks it. One choice per browser it can run ("On this Mac, in …", the first recommended), then the
 * container; with none, one line that says why (not to be chosen). `selected` follows app.json's
 * `browser` and `browser_app`; with neither, on Set up (`fresh`) the Mac when it has a browser (the
 * recommendation), in Settings the container (where it runs). `get`: "Get a browser…" is offered. */
export interface MacBrowserQuestion { options: { id: string; label: string; disabled: boolean }[]; selected: number; get: boolean }

export function macBrowserQuestion(m: Model, w: Worker, fresh = true): MacBrowserQuestion {
  const why = m.macBrowsers?.why ?? "";
  const installed = why ? [] : m.macBrowsers?.installed ?? [];
  const options: MacBrowserQuestion["options"] = [];
  if (why) options.push({ id: "", label: `On this Mac (${why})`, disabled: true });
  installed.forEach((b, i) => options.push({ id: b.id, label: `On this Mac, in ${b.name}${i === 0 ? " (recommended)" : ""}`, disabled: false }));
  options.push({ id: "", label: "In the container", disabled: false });
  const browser = str(w.config.data.browser);
  const mac = browser === "mac" || browser === "container" || !fresh ? browser === "mac" : installed.length > 0;
  const want = (installed.find((b) => b.id === w.config.browserApp) ?? installed[0])?.id ?? "";
  const at = options.findIndex((o) => o.id !== "" && o.id === want);
  return { options, selected: mac && installed.length && at >= 0 ? at : options.length - 1, get: installed.length === 0 && !why.includes("sandbox") };
}

/** The line under the question, for the choice made. */
export function macBrowserHint(m: Model, onMac: boolean): string {
  const why = m.macBrowsers?.why ?? "";
  if (onMac) return "A real browser with its own profile, kept to the worker's folder by a sandbox: sites see an ordinary Mac, and you use the window itself.";
  return why && !why.includes("sandbox")
    ? "A Chromium inside the container, shown through a window in your browser. Some sites take it for a bot, even when it's you. To run a real browser on this Mac instead, install any of Google Chrome, Microsoft Edge, Brave or Chromium (all free): the choice above turns on by itself."
    : "A Chromium inside the container, shown through a window in your browser. Some sites take it for a bot, even when it's you.";
}

// --- the review level ---------------------------------------------------------

/** The review's levels as Set up and Settings offer them (review.json's `level`): the option's
 * words, the sentence under the select that says what runs unasked and what asks (the wait is the
 * review's, MYOUS_APPROVAL_WAIT, 120 s), and the words in the facts line without a container. */
export const REVIEW_LEVELS: { level: string; label: string; means: string; fact: string }[] = [
  { level: "trust", label: "Trust my agent: everything it asks runs",
    means: "Everything your agent asks runs; nothing asks you first.",
    fact: "everything your agent asks runs" },
  { level: "changes", label: "Ask me before anything that changes things",
    means: "Reads inside the work folder run; anything that changes things, reaches outside it or isn't a plain read asks you first, and is refused if you don't answer within two minutes.",
    fact: "asks you before anything that changes things" },
  { level: "all", label: "Ask me before every request",
    means: "Every request asks you first, even a plain read, and is refused if you don't answer within two minutes.",
    fact: "asks you before every request" },
];

/** A level the review knows; anything else is "all", as the workers read a review.json without a
 * valid level (the app reads one with none as "changes", files.rs's `review_level`). */
export function reviewLevel(level: string): string {
  return REVIEW_LEVELS.some((l) => l.level === level) ? level : "all";
}

const reviewEntry = (level: string) => REVIEW_LEVELS.find((l) => l.level === reviewLevel(level))!;
/** The sentence under the review select (Set up and Settings) for a level. */
export const reviewMeans = (level: string) => reviewEntry(level).means;
/** The level in the facts line on Set up without a container ("Review: …"). */
export const reviewFact = (level: string) => reviewEntry(level).fact;

// --- a row that moved on ------------------------------------------------------

const BROWSER_STEPS = ["nobrowser", "nopython", "novenv", "noplaywright", "ready"];
const RUNTIME_STATES = ["missing", "stopped", "permission", "nocompose", "ok"];

/** What a row that moved on found, in words, from the state behind it
 * before (a check's state, the browser's step) and now: "Found Python
 * 3.12. Next: Playwright." "" when the change isn't news (a step back, or
 * from one problem to an unrelated one). */
export function advanceWords(m: Model, key: CheckName, prev: Omit<Seen, "at">, cur: Omit<Seen, "at">, hub = ""): string {
  const ok = cur.mark === "✓";
  let found = "", next = "";
  switch (key) {
    case "browser": {
      const from = BROWSER_STEPS.indexOf(prev.step), to = BROWSER_STEPS.indexOf(cur.step);
      if (from < 0 || to <= from) return "";
      const c = m.browserCheck;
      found = ({ nobrowser: `Found ${c?.browser?.name ?? "a browser"}.`, nopython: `Found Python ${c?.python?.version ?? "3"}.`,
        novenv: "Python's venv part is there.", noplaywright: "Playwright is installed." } as Record<string, string>)[prev.step];
      next = ({ nopython: "Next: Python.", novenv: "Next: Python's venv part.", noplaywright: "Next: Playwright.",
        ready: "The browser is ready." } as Record<string, string>)[cur.step];
      break;
    }
    case "runtime": {
      const name = m.runtimeName || "Docker";
      const from = RUNTIME_STATES.indexOf(prev.step), to = RUNTIME_STATES.indexOf(cur.step);
      if (from >= 0 && to > from) {
        found = ({ missing: `Found ${name}.`, stopped: `${name} is running.`, permission: `You can use ${name} now.`,
          nocompose: "Compose is there." } as Record<string, string>)[prev.step];
        next = ({ stopped: "Next: start it.", permission: "Next: allow yourself to use it.", nocompose: "Next: its compose part." } as Record<string, string>)[cur.step] ?? "";
      } else if (ok) {
        found = prev.step === "" ? `Found ${name}.` : `${name} answers.`;
      }
      break;
    }
    case "containers":
      if (ok) found = "Containers can run now.";
      break;
    case "hub":
      if (ok) found = `${hubHost(hub)} answers.`;
      break;
    case "agents":
      if (ok && agentNames(m).length) found = `Found ${agentNames(m).join(", ")}.`;
      break;
  }
  if (!found) return ok ? `Done: ${cur.text}` : "";
  return next ? `${found} ${next}` : found;
}

/** Notes a checklist row in the model as shown now: its advance while it lasts (null none). */
export function checkAdvance(m: Model, r: Row, hub: string, now = m.now()): Advance | null {
  const cur = { mark: r.mark, text: r.text, step: m.checks[r.key].state };
  return m.noteAdvance(r.key, cur, (prev) => advanceWords(m, r.key, prev, cur, hub), now);
}

/** Notes the browser row in the model as shown now: its advance while it
 * lasts. While Playwright installs (and until the check after it), the row
 * before stays the one compared, so "ready" at the end is the advance. */
export function browserAdvance(m: Model, w: Worker, r: BrowserRow, now = m.now()): Advance | null {
  const step = m.browserStep(w, now), i = m.browserInstall;
  const cur = { mark: r.mark, text: r.text, step };
  const hold = step === "noplaywright" && !!i && (i.running || i.status === 0);
  return m.noteAdvance("browser", cur, (prev) => advanceWords(m, "browser", prev, cur), now, hold);
}

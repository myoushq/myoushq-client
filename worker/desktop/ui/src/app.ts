// The window: renders the model, runs what the owner presses, keeps the
// tray in step. Every two seconds it rereads the worker folders and
// decides each worker's screen again (the model), then draws the current
// one. The same screens and words as myous for Mac.

import { Backend, makeBackend, PreviewBackend, TrayItem } from "./backend.js";
import { applyFake } from "./fake.js";
import { appMenu, buildMenu, MenuActions, moreMenu } from "./menu.js";
import { Advance, Agent, agentContacts, agentName, isUrgent, Kind, Model, notifyKinds, RuntimeStep, shortNpub, verb, what, Worker } from "./model.js";
import { attentionCard, browserAdvance, browserCard, BrowserRow, browserRow, checkAdvance, CheckName, checklist, hubLine, installLine, Row, runtimeCard } from "./setup.js";
import { byteCount, capitalize, clockTime, countdown, dayLabel, defaultDescription, dict, isDefaultDescription, newer, num, pairingMessage, plural, setHostOs, str, timeAgo } from "./text.js";

const INVITE_SECONDS = 900;
const AGENTS_EVERY = 15; // ticks between reads of the local agents
const HUB_EVERY = 15; // seconds between hub checks while a setup card (or the red line) is up

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const setText = (el: HTMLElement, text: string) => { if (el.textContent !== text) el.textContent = text; };
const show = (el: HTMLElement, on: boolean) => { if (el.hidden === on) el.hidden = !on; };
function el<T extends HTMLElement = HTMLElement>(tag: string, props: Record<string, unknown> = {}, ...kids: (Node | string)[]): T {
  const e = Object.assign(document.createElement(tag), props) as T;
  e.append(...kids);
  return e;
}

/** A line that starts with a mark (✓ ✗ ○ ⟳ –): the mark in a span of its own,
 * so it lines up in a column and takes its colour; the text stays the same. */
const MARKS: Record<string, string> = { "✓": "ok", "✗": "bad", "○": "todo", "–": "todo", "⟳": "busy" };
function marked(mark: string, text: string): (Node | string)[] {
  const cls = MARKS[mark];
  return cls ? [el("span", { className: `mk ${cls}`, textContent: `${mark} ` }), text] : [`${mark} ${text}`];
}
function setMarked(e: HTMLElement, line: string) {
  if (e.textContent === line) return;
  const mark = line.charAt(0), cls = MARKS[mark];
  if (!cls || line.charAt(1) !== " ") { e.textContent = line; return; }
  // The same mark with new words (a clock ticking): keep the mark, so a turning ⟳ doesn't restart.
  const [first, second] = [e.firstChild, e.childNodes[1]];
  if (e.childNodes.length === 2 && first instanceof HTMLElement && first.textContent === `${mark} ` && second?.nodeType === Node.TEXT_NODE) second.textContent = line.slice(2);
  else e.replaceChildren(...marked(mark, line.slice(2)));
}

const be: Backend = makeBackend();
let info: Awaited<ReturnType<Backend["appInfo"]>>;
let m: Model;
let sys = "this computer";
let sep = "/";
let windowShown = false;
let busy = false;
let agentsLoading = false;
let logLines: string[] = [];
let trayActions = new Map<string, () => void>();
let lastTrayItems = "";
let lastTrayIcon = "";
let appMenuActions = new Map<string, () => void>();
let lastAppMenu = "";
let lastCounts = 0;
let setupFilledFor = "";
let lastRequestsKey = "";
let pairAgents: Agent[] = [];
let runtimeProbing = false;
let capabilityChecking = false;
let hubChecking = false;
let lastHubCheck = 0;
let browserChecking = false;
let lastBrowserCheck = 0;
let lastStep: RuntimeStep | "none" = "none";
const startLines = new Map<string, string[]>(); // each worker's output since its last start

// --- the log ---------------------------------------------------------------

function appendLog(line: string) {
  const t = new Date();
  const stamp = [t.getHours(), t.getMinutes(), t.getSeconds()].map((n) => String(n).padStart(2, "0")).join(":");
  logLines.push(`${stamp}  ${line}`);
  if (logLines.length > 2000) logLines = logLines.slice(-2000);
  void be.logLine(`${t.toISOString()} ${line}`);
  const pre = document.querySelector<HTMLPreElement>("#dlg pre.live-log");
  if (pre) pre.textContent = logLines.join("\n");
}

// --- dialogs ----------------------------------------------------------------

interface DialogButton { label: string; value: string; primary?: boolean; danger?: boolean }
interface DialogOpts { title: string; text?: string; pre?: string; liveLog?: boolean; img?: string; input?: { placeholder?: string; value?: string }; buttons?: DialogButton[]; body?: HTMLElement }

let closeCurrent: ((v: { value: string; input: string } | null) => void) | null = null;

function dialog(o: DialogOpts): Promise<{ value: string; input: string } | null> {
  const d = $<HTMLDialogElement>("dlg");
  closeCurrent?.(null);
  d.replaceChildren();
  d.append(el("h2", { textContent: o.title }));
  if (o.text) d.append(el("p", { textContent: o.text }));
  if (o.img) d.append(el("img", { src: o.img, width: 240, height: 240, alt: "QR code" }));
  if (o.pre !== undefined) d.append(el("pre", { textContent: o.pre, className: o.liveLog ? "live-log" : "" }));
  if (o.body) d.append(o.body);
  let input: HTMLInputElement | null = null;
  if (o.input) {
    input = el<HTMLInputElement>("input", { placeholder: o.input.placeholder ?? "", value: o.input.value ?? "" });
    d.append(input);
  }
  const buttons = o.buttons ?? [{ label: "OK", value: "ok", primary: true }];
  const row = el("div", { className: "row" });
  d.append(row);
  return new Promise((resolve) => {
    const done = (v: { value: string; input: string } | null) => {
      if (closeCurrent !== done) return;
      closeCurrent = null;
      if (d.open) d.close();
      resolve(v);
    };
    closeCurrent = done;
    for (const b of buttons) {
      const btn = el<HTMLButtonElement>("button", { textContent: b.label, className: b.primary ? "primary" : b.danger ? "danger" : "" });
      btn.onclick = () => done({ value: b.value, input: input?.value ?? "" });
      row.append(btn);
    }
    d.onclose = () => done(null);
    input?.addEventListener("keydown", (e) => { if (e.key === "Enter") done({ value: buttons[buttons.length - 1].value, input: input!.value }); });
    d.showModal();
    (input ?? row.lastElementChild as HTMLElement)?.focus();
  });
}

const alertBox = (title: string, text: string) => dialog({ title, text });
async function confirmBox(title: string, text: string, ok: string, danger = false) {
  const r = await dialog({ title, text, buttons: [{ label: "Cancel", value: "no" }, { label: ok, value: "yes", primary: !danger, danger }] });
  return r?.value === "yes";
}

// --- start, stop, the browser ----------------------------------------------------

function composePath(w: Worker) {
  return w.config.isImage ? `${w.home}${sep}compose.yml` : `${w.config.repo}${sep}worker${sep}compose.yml`;
}

function composeEnv(w: Worker): Record<string, string> {
  const env: Record<string, string> = {
    MYOUS_WORKER_HOME: w.home,
    MYOUS_DESCRIPTION: w.config.effectiveDescription,
    MYOUS_BROWSER: "container",
  };
  if (w.config.name) env.MYOUS_ALIAS = w.config.name;
  return env;
}

async function saveConfig(w: Worker) {
  if (m.fake) return;
  try {
    await be.writeConfig(w.home, w.config.data);
  } catch (e) {
    appendLog(`could not save settings: ${e}`);
  }
}

async function start(w: Worker) {
  // Made-up states start nothing, except in a plain browser's preview, whose backend only records (the click tests).
  if (m.fake && !be.preview) return;
  void checkHub();
  w.failure = [];
  startLines.set(w.home, []);
  w.launchedAt = m.now();
  w.stopping = false;
  w.stopDoneAt = 0;
  w.wasRunning = false;
  if (w.config.isDirect) {
    try {
      const r = await be.directStart(w.home, w.config.name, w.config.effectiveDescription, w.config.hostBrowser);
      w.browserLaunch = r.browser;
      appendLog(`started myous worker (pid ${r.pid}); its output is in worker.log`);
    } catch (e) {
      appendLog(`could not start: ${e}`);
      w.launchedAt = 0;
      void alertBox("Couldn't start the worker", String(e));
    }
    render();
    return;
  }
  let file: string;
  try {
    file = w.config.isImage ? await be.writeCompose(w.home) : composePath(w);
    if (!w.config.isImage && !(await be.fileExists(file))) throw new Error(`no compose file at ${file}`);
  } catch (e) {
    appendLog(`could not start: ${e}`);
    w.launchedAt = 0;
    return;
  }
  w.launchStage = "pulling";
  const args = ["up", "-d", ...(w.config.isImage ? [] : ["--build"])];
  await be.composeRun(w.home, file, w.project, args, composeEnv(w), "start");
  render();
}

async function stop(w: Worker) {
  if (m.fake) return;
  w.stopping = true;
  w.stoppedAt = m.now();
  w.stopDoneAt = 0;
  w.launchedAt = 0;
  w.launchStage = "";
  if (w.config.isDirect) {
    // Ours: its exit arrives as an event. Started before this app was: ended by pid, gone now.
    const ours = await be.directStop(w.home, w.fresh(m.now()) ? num(w.status?.pid) : 0);
    appendLog("stopping myous worker");
    if (!ours) w.stopDoneAt = m.now();
    void refresh();
    return;
  }
  await be.composeRun(w.home, composePath(w), w.project, ["down", "--remove-orphans"], {}, "stop");
  void refresh();
}

async function openBrowserView(w: Worker = m.current) {
  if (w.config.isDirect) return showBrowser(w);
  const port = await be.browserPort(composePath(w), w.project, w.isDefault);
  if (!port) {
    appendLog("the worker isn't running, so there's no browser to open");
    await alertBox("No browser yet", "The worker isn't running, so there's no browser to open.");
    return;
  }
  const link = `http://localhost:${port}/?autoconnect=1&reconnect=1&resize=remote`;
  appendLog(`opening ${link}`);
  await be.openUrl(link);
}

/** Without a container: the worker's browser on this computer to the front, or started (again, or
 * for the first time while the worker runs: the worker names it in its next help, from browser.json). */
async function showBrowser(w: Worker) {
  if (m.fake && !be.preview) return;
  try {
    appendLog(`browser: ${await be.browserShow(w.home, w.config.name)}`);
    // It is the worker's browser now: the start rows, the tray's item and the card follow browserUp.
    w.browserLaunch = "starting";
  } catch (e) {
    appendLog(`browser: ${e}`);
    await alertBox("Couldn't show the browser", String(e));
  }
  void refresh();
}

async function togglePaused(w: Worker = m.current) {
  if (m.fake) return;
  await be.setPaused(w.home, !w.paused);
  appendLog(w.paused ? "resumed" : "paused: requests are refused while worker.paused exists");
  void refresh();
}

// --- the window and what it shows ----------------------------------------------------

async function showWindow(w?: Worker) {
  if (w) {
    m.current = w;
    m.selectedAgent = null;
  }
  await be.showWindow();
  windowShown = true;
  markSeen();
  render();
}

function markSeen() {
  const now = m.now();
  for (const w of m.workers) {
    if (w.config.seenRequestsAt < now - 1 && w.requests.length > 0 && num(w.requests[0].at) > w.config.seenRequestsAt) {
      w.config.seenRequestsAt = now;
      void saveConfig(w);
    }
  }
}

function kindClass(k: Kind | "none") { return `dot ${k}`; }

function render() {
  const w = m.current;
  const now = m.now();
  let attention = w.attention, button = w.attentionButton;
  const hubDown = hubLine(m, sys, info.hub);
  if (hubDown && !attention) {
    attention = hubDown;
    button = "Retry";
  }
  if (m.latestRelease && m.latestRelease !== m.appConfig.skippedVersion && !attention) {
    attention = `myous ${m.latestRelease} is available (you have ${m.version}).`;
    button = "Download";
  }

  // Header
  $("head-dot").className = kindClass(w.stateKind);
  setText($("head-title"), `${w.stateWord} · ${w.name}`);
  const pairedWith = str(w.paired?.alias) || (w.contacts > 0 ? "your agent" : "");
  setText($("head-right"), w.screen === "setup" || w.screen === "noruntime" ? "" : pairedWith ? `Paired · ${pairedWith}` : "Not paired yet");
  const facts: string[] = [];
  if (w.alive && w.config.name) {
    facts.push(`Requests today: ${w.requests.filter((r) => dayLabel(num(r.at), now) === "Today").length}`);
    const last = num(dict(w.status?.last)?.at);
    facts.push(last ? `Last: ${timeAgo(last, now)}` : "No requests yet");
    if (w.config.isDirect && w.config.hostBrowser && w.browserUp) facts.push(`Browser on ${sys}`);
  } else if (w.config.isDirect && w.config.name) {
    facts.push("Runs without a container");
  } else if (m.runtimeName && w.config.usesDocker && w.config.name) {
    facts.push(`Runs in ${m.runtimeName}`);
  }
  setText($("head-facts"), facts.join("   ·   "));
  show($("head-facts"), facts.length > 0);

  // Cards
  const screen = w.screen;
  const agentShown = !!m.selectedAgent;
  const showRequests = screen === "running" || screen === "paired";
  const stoppedShown = screen === "stopped" && !w.attention && !!w.config.name;
  // Needs attention: the red line says what, the card has Show log and Try again.
  const attentionShown = screen === "stopped" && !!w.attention && !!w.config.name;

  // Banner (its button is the card's when the Needs attention card is up)
  setText($("banner-text"), attention);
  setText($("banner-button"), button);
  show($("banner-button"), !!button && !(attentionShown && !agentShown && attention === w.attention));
  show($("banner"), !!attention);

  show($("card-setup"), !agentShown && screen === "setup");
  show($("card-runtime"), !agentShown && screen === "noruntime");
  show($("card-starting"), !agentShown && screen === "starting");
  show($("card-pair"), !agentShown && screen === "pair");
  show($("card-paired"), !agentShown && screen === "paired");
  show($("card-stopping"), !agentShown && screen === "stopping");
  show($("card-stopped"), !agentShown && stoppedShown);
  show($("card-attention"), !agentShown && attentionShown);
  show($("card-requests"), !agentShown && showRequests);
  const bcard = browserCard(m, w);
  show($("card-browser"), !agentShown && showRequests && bcard.shown);
  show($("card-agent"), agentShown);
  show($("setup-remove"), !w.isDefault);
  show($("stopped-remove"), !w.isDefault);
  setText($("stopped-text"), `The worker is stopped. Your agent can't reach ${sys} until you start it.`);
  setText($("stopping-text"), w.config.isDirect
    ? "Stopping the worker. Your agent's requests are refused from now on."
    : "Stopping the worker. Its container shuts down in a few seconds; your agent's requests are refused meanwhile.");
  setText($("browser-text"), bcard.text);
  setText($("browser-open"), bcard.label);
  $("browser-open").dataset.action = bcard.button;
  show($("browser-open-row"), !!bcard.button);
  show($("browser-row"), bcard.row);
  if (bcard.row && showRequests && !agentShown) fillBrowserRow($("browser-row"), w);
  if (!agentShown) {
    if (screen === "setup") fillSetup(w);
    if (screen === "noruntime") fillRuntime(w);
    if (screen === "starting") fillStarting(w);
    if (screen === "pair") fillPair(w);
    if (screen === "paired") fillPaired(w);
    if (showRequests) fillRequests(w);
    if (attentionShown) fillAttention(w);
  }
  fillApproval(w);
  fillSidebar();
  if (m.selectedAgent) fillAgent(m.selectedAgent);
  renderTray();
  renderAppMenu();
}

/** A row that just moved on: it flashes green (picking up where the flash
 * is if the row is drawn again), and the line under it says what was found. */
function markAdvance(row: HTMLElement, a: Advance | null): HTMLElement[] {
  if (!a) return [];
  row.classList.add("advanced");
  row.style.animationDelay = `${-Math.max(0, m.now() - a.at)}s`;
  return [el(row.tagName.toLowerCase(), { className: "found" }, ...marked("✓", a.line))];
}

function fillChecks(list: HTMLElement, w: Worker) {
  const rows = checklist(m, w, sys, info.hub);
  const advances = rows.map((r) => checkAdvance(m, r, info.hub));
  const key = JSON.stringify([rows, advances]);
  if (list.dataset.key === key) return;
  list.dataset.key = key;
  list.replaceChildren(...rows.flatMap((r: Row, n) => {
    const li = el("li", { className: r.cls }, ...marked(r.mark, r.text));
    if (r.retry) {
      const b = el("button", { className: "link", textContent: "Retry" });
      b.dataset.retry = r.retry;
      b.onclick = () => retryCheck(r.retry!);
      li.append(" ", b);
    }
    return [li, ...markAdvance(li, advances[n])];
  }));
}

function fillSetup(w: Worker) {
  const direct = w.config.isDirect;
  fillChecks($("setup-checks"), w);
  // The first answers aren't in: the checklist alone, the form when they are.
  const checking = !direct && m.runtimeStep() === "checking";
  show($("setup-form"), !checking);
  show($("setup-checking"), checking);
  setText($("setup-checking"), `Checking ${sys}…`);
  const name = $<HTMLInputElement>("setup-name"), desc = $<HTMLTextAreaElement>("setup-desc");
  const key = `${w.home}|${direct}`;
  if (setupFilledFor !== key) {
    setupFilledFor = key;
    name.value = w.config.name || (w.isDefault ? info.hostname : "");
    name.placeholder = info.hostname;
    desc.value = w.config.description || defaultDescription(name.value, direct, !direct || w.config.hostBrowser);
    name.dataset.prev = name.value;
  }
  // Without a container the worker is the one this app carries: nothing to find or install.
  setMarked($("setup-runtime"), direct
    ? `✓ Runs on ${sys} as you${w.config.browserOff ? " · No worker browser" : ""} · Review: ${REVIEW_FACT[w.review] ?? REVIEW_FACT.changes}`
    : m.runtimeState === "ok"
      ? `The worker runs in a container in ${m.runtimeName}, so your agent's commands stay inside it.`
      : "The worker runs in a container, so your agent's commands stay inside it.");
  show($("setup-wbrowser"), direct);
  if (direct) fillBrowserRow($("setup-wbrowser"), w);
  setText($("setup-browser"), direct
    ? `Without a container, nothing but the review stands between your agent's commands and ${sys}. The review starts at "Ask me before anything that changes things" (Settings).`
    : "Its browser runs in the container, shown through a window in your own browser. Some sites take it for a bot, even when it's you.");
  $("setup-browser").className = direct ? "warn" : "";
  setText($("setup-desc-hint"), direct && !w.config.hostBrowser
    ? "Sent to the agent when it pairs, and again when you change it. Say it in your own words."
    : "Sent to the agent when it pairs, and again when you change it. Say it in your own words; the default binds \"the myous browser\" to this worker's browser.");
  setText($("setup-hint"), `Already have an agent on ${sys}? It installs the myous client itself (see the skill). This app runs workers: computers an agent can use.`);
}

/** The worker browser's row without a container (Set up, and the Browser
 * card of a worker started without it): its line, its buttons, the line to
 * type, the link that turns it off or on. */
function fillBrowserRow(box: HTMLElement, w: Worker) {
  const r: BrowserRow = browserRow(m, w, sys);
  const advance = browserAdvance(m, w, r);
  const key = `${w.home}|${JSON.stringify([r, advance])}`;
  if (box.dataset.key === key) return;
  box.dataset.key = key;
  const line = el("p", { className: r.cls }, ...marked(r.mark, r.text));
  if (r.retry) {
    const b = el("button", { className: "link", textContent: "Retry" });
    b.dataset.retry = "browser";
    b.onclick = () => retryCheck("browser");
    line.append(" ", b);
  }
  const kids: HTMLElement[] = [line, ...markAdvance(line, advance)];
  if (r.buttons.length) {
    kids.push(el("div", { className: "row" }, ...r.buttons.map((b) => {
      // On Set up, "Start the worker" is the card's one primary button.
      const btn = el("button", { className: b.primary && !box.closest("#card-setup") ? "primary" : "", textContent: b.label });
      btn.dataset.browser = b.action;
      if (b.url) btn.dataset.url = b.url;
      btn.onclick = () => (b.action === "install" ? void runBrowserInstall(w) : void be.openUrl(b.url));
      return btn;
    })));
  }
  if (r.lines.length) {
    const copy = el("button", { textContent: "Copy" });
    copy.dataset.browser = "copy";
    copy.onclick = async () => {
      await be.copyText(r.lines.join("\n"));
      setText(copy, "Copied");
      setTimeout(() => setText(copy, "Copy"), 2000);
    };
    kids.push(el("pre", { className: "lines", textContent: r.lines.join("\n") }), el("div", { className: "row" }, copy));
  }
  if (r.note) kids.push(el("p", { className: "hint", textContent: r.note }));
  if (r.toggle) {
    const t = el("button", { className: "link", textContent: r.toggle });
    t.dataset.browser = "toggle";
    t.onclick = () => void setBrowserOff(w, !w.config.browserOff);
    kids.push(r.later ? el("div", { className: "row" }, t, el("span", { className: "hint", textContent: r.later })) : t);
  }
  box.replaceChildren(...kids);
}

/** "Continue without a browser" and back. */
async function setBrowserOff(w: Worker, off: boolean) {
  w.config.browserOff = off;
  // The default description follows (it names the browser only when there is one).
  const desc = $<HTMLTextAreaElement>("setup-desc"), name = $<HTMLInputElement>("setup-name");
  if (w.screen === "setup" && isDefaultDescription(desc.value, name.value.trim(), w.config.isDirect)) {
    desc.value = defaultDescription(name.value.trim(), w.config.isDirect, !off);
  }
  await saveConfig(w);
  appendLog(off ? `${w.name}: no worker browser` : `${w.name}: worker browser on ${sys}`);
  if (!off) void checkBrowser(true);
  render();
}

/** "Install Playwright": into the app's own Python environment; the output in the app log. */
async function runBrowserInstall(w: Worker) {
  m.browserInstall = { id: "playwright", name: "Playwright", running: true, status: 0, lastLine: "" };
  render();
  if (m.fake && !be.preview) return;
  try {
    await be.browserInstall(w.home);
  } catch (e) {
    m.browserInstall = { id: "playwright", name: "Playwright", running: false, status: -1, lastLine: String(e) };
    appendLog(`playwright: ${e}`);
  }
  render();
}

/** What the worker's browser needs here (browser, Python, Playwright); only asked when a worker runs without a container. */
async function checkBrowser(force = false) {
  if ((m.fake && !be.preview) || browserChecking) return;
  if (!force && !m.workers.some((w) => w.config.hostBrowser)) return;
  browserChecking = true;
  lastBrowserCheck = m.now();
  m.checks.browser.begin(m.now());
  try {
    const c = await be.browserCheck();
    const was = JSON.stringify(m.browserCheck);
    m.browserCheck = c;
    m.checks.browser.settle("ok", "", m.now());
    if (JSON.stringify(c) !== was) {
      appendLog(`worker browser: ${c.browser ? c.browser.name : "no browser"}; ${c.playwright ? `Playwright ${c.playwright}` : c.python ? `Python ${c.python.version}, no Playwright yet` : "no Python 3.10+"}`);
    }
    // Installed and found: the install line has done its work.
    if (c.playwright && m.browserInstall && !m.browserInstall.running) m.browserInstall = null;
  } catch (e) {
    m.checks.browser.settle("unknown", "", m.now());
    appendLog(`browser check: ${e}`);
  } finally {
    browserChecking = false;
  }
  render();
}

/** The review level as a fact on the setup card without a container. */
const REVIEW_FACT: Record<string, string> = {
  trust: "everything your agent asks runs",
  changes: "asks you before anything that changes things",
  all: "asks you before every request",
};

let directOpen = false;
let optionsLoading = false;
async function loadOptions() {
  if (optionsLoading || (m.fake && !be.preview)) return;
  optionsLoading = true;
  try {
    m.options = await be.runtimeOptions(m.runtimeName);
  } catch (e) {
    appendLog(`runtime options: ${e}`);
  } finally {
    optionsLoading = false;
  }
  render();
}

function fillRuntime(w: Worker) {
  const step = m.runtimeStep();
  const cap = m.capability ?? { state: "ok", reason: "", machine: "" };
  const unsupported = step === "novirt";
  const wslOff = step === "wsl";
  fillChecks($("runtime-checks"), w);
  if (!m.options && step !== "wsl" && step !== "novirt") void loadOptions();
  const card = runtimeCard(m, step, sys);
  let title = card.title, text = card.text;
  if (unsupported) {
    title = "Containers can't run here";
    const why = cap.reason === "virtual"
      ? `${capitalize(sys)} is a virtual machine${cap.machine ? ` (${cap.machine})` : ""} that doesn't pass virtualization on to it; only its host could change that (nested virtualization).`
      : cap.reason === "cpu"
        ? "This PC's processor has no virtualization."
        : "Virtualization is turned off in this PC's firmware settings (Intel VT-x, AMD-V or SVM in the BIOS or UEFI setup). Turn it on there and restart, and this page offers the container runtimes.";
    text = `✗ Docker Desktop and Rancher Desktop run containers in WSL 2, which needs virtualization. ${why} You can still run the worker without a container.`;
  } else if (wslOff && m.wslPending) {
    title = "Turn on WSL";
    text = "✓ WSL is turned on. Restart Windows to finish; then get Docker Desktop or Rancher Desktop here.";
  } else if (wslOff) {
    title = "Turn on WSL";
    text = "✗ No container runtime, and WSL isn't turned on. Docker Desktop and Rancher Desktop run containers in WSL 2, so it comes first: Windows asks for an administrator, then for a restart.";
  }
  setText($("runtime-title"), title);
  setMarked($("runtime-text"), text);

  // Get a container runtime: one line per runtime, Install where the app can, its website always.
  const installing = !!m.install?.running;
  const list = $("runtime-list");
  const runtimes = step === "get" ? m.options?.runtimes ?? [] : [];
  show(list, runtimes.length > 0 && !installing);
  // Until the backend says which runtimes this system can have (it asks the
  // package manager, which can take a while on a cold start).
  show($("runtime-loading"), step === "get" && !m.options && !installing);
  const listKey = JSON.stringify(runtimes);
  if (list.dataset.key !== listKey) {
    list.dataset.key = listKey;
    const page = (r: { url: string }, label: string, cls = "") => {
      const b = el("button", { className: cls, textContent: label });
      b.dataset.url = r.url;
      b.onclick = () => void be.openUrl(r.url);
      return b;
    };
    // Nothing the app can install here: the Mac's row of "Get …" buttons, each to its page.
    if (!runtimes.some((r) => r.install)) {
      list.replaceChildren(el("div", { className: "row" }, ...runtimes.map((r) => page(r, `Get ${r.name}`))));
    } else {
      // The first runtime the app can install is the recommended one: the card's one primary button.
      const first = runtimes.find((r) => r.install);
      list.replaceChildren(...runtimes.map((r) => {
        const line = el("div", { className: "runtime-option" }, el("span", { className: "name", textContent: r.name, title: r.name }));
        if (!r.install) line.append(el("span"));
        else {
          const b = el("button", { className: r === first ? "primary" : "", textContent: "Install", title: r.install });
          b.dataset.install = r.id;
          b.onclick = () => void runInstall(r.id, r.name);
          line.append(b);
        }
        line.append(page(r, "Website", "link"));
        return line;
      }));
    }
  }
  // An install, a start or a compose install the app runs, on the card it belongs to.
  const own = m.install && (m.install.id === "start" ? step === "start" : m.install.id === "compose" ? step === "compose" : step === "get");
  const iline = own ? installLine(m) : "";
  setMarked($("runtime-install"), iline);
  $("runtime-install").className = iline.startsWith("✗") ? "bad" : "";
  show($("runtime-install"), !!iline);
  setText($("runtime-lines"), card.lines.join("\n"));
  show($("runtime-lines-box"), card.lines.length > 0);
  setText($("runtime-fix"), card.buttonLabel);
  $("runtime-fix").dataset.action = card.button;
  ($("runtime-fix") as HTMLButtonElement).disabled = installing;
  show($("runtime-fix-row"), !!card.button && !(own && installing));
  const note = step === "get" && !runtimes.some((r) => r.install) ? "" : card.note;
  setText($("runtime-note"), note);
  show($("runtime-note"), !!note && !installing);
  show($("runtime-link"), !!card.link);
  show($("wsl-row"), wslOff && !m.wslPending);
  // Without a container: the way on when containers can't run, an option otherwise (decision A).
  const offerDirect = step === "get" || wslOff || unsupported;
  const toggle = $("direct-toggle");
  show(toggle, offerDirect && !unsupported);
  setText(toggle, `${directOpen ? "▾" : "▸"} ${wslOff ? "Continue without containers" : "Run without a container (not recommended)"}`);
  show($("direct-more"), unsupported || (directOpen && offerDirect));
  setText($("direct-warning"), `Without a container, commands your agent sends run on ${sys} as you, with everything you can reach, and only the review stands in the way (it starts at "Ask me before anything that changes things"). Its browser, if you want one, runs on ${sys} too and needs Python.`
    + (unsupported ? "" : " Use a container unless you know why you don't want one."));
  const use = $("direct-use");
  setText(use, unsupported || wslOff ? "Continue without containers" : "Run without a container");
  use.className = unsupported ? "primary" : "";
}

/** Installs a runtime, starts Docker Engine ("start") or adds compose
 * ("compose"); the output goes to the app log, the card follows the checks. */
async function runInstall(id: string, name: string) {
  m.install = { id, name, running: true, status: 0, lastLine: "" };
  render();
  if (m.fake && !be.preview) return;
  try {
    await be.runtimeInstall(id, m.runtimeName);
  } catch (e) {
    m.install = { id, name, running: false, status: -1, lastLine: String(e) };
    appendLog(`${id}: ${e}`);
  }
  render();
}

function runtimeFix() {
  switch ($("runtime-fix").dataset.action) {
    case "open": return void be.launchRuntime(m.runtimeName);
    case "start": return void runInstall("start", m.runtimeName);
    case "compose": return void runInstall("compose", "compose");
  }
}

function fillAttention(w: Worker) {
  const c = attentionCard(m, w, sys, info.hub);
  setText($("attention-text"), c.text);
  setText($("attention-log"), c.log.join("\n"));
  show($("attention-log"), c.log.length > 0);
}

/** Try again: the same start, from the stopped worker (no Stop needed). */
async function tryAgain() {
  const w = m.current;
  w.attention = w.attentionButton = "";
  w.wasRunning = false;
  w.stoppedByUs = true;
  await start(w);
  void refresh();
}

function fillStarting(w: Worker) {
  const now = m.now();
  const phase = w.phase;
  const pulled = w.launchStage === "" || w.launchStage === "creating";
  const inContainer = phase !== "" && w.fresh(now);
  const browserDone = inContainer && (phase === "registering" || phase === "running");
  const elapsed = w.launchedAt ? now - w.launchedAt : 0;
  const clock = elapsed > 0 ? `  ${countdown(elapsed)}` : "";
  const mark = (done: boolean, active: boolean) => (done ? "✓" : active ? "⟳" : "○");
  const items = $("start-rows").querySelectorAll("li");
  if (w.config.isDirect) {
    const up = w.alive || phase === "running";
    const direct = [`✓ No container: the worker runs on ${sys}`];
    if (w.browserLaunch === "starting") direct.push(`${w.browserUp ? "✓" : "⟳"} Starting the browser`);
    else if (w.browserLaunch) direct.push(`○ No worker browser: ${w.browserLaunch}`);
    direct.push(`${up ? "✓" : "⟳"} Starting the worker and registering with myoushq.com${up ? "" : clock}`);
    items.forEach((li, i) => {
      setMarked(li, direct[i] ?? "");
      li.hidden = !direct[i];
      li.className = direct[i]?.startsWith("○") ? "todo" : "";
    });
    setText($("start-note"), elapsed > 60 ? "This is taking longer than usual. The worker's log says why (Show worker log, in the ⋯ menu)." : "Usually a few seconds.");
    return;
  }
  items.forEach((li) => (li.hidden = false));
  const rows = [
    "✓ Container runtime ready",
    `${mark(pulled, !pulled)} Downloading the worker (first time only)${!pulled ? clock : ""}`,
    `${mark(browserDone, pulled && inContainer)} Starting the browser${!browserDone && pulled && inContainer ? clock : ""}`,
    phase.startsWith("error") ? `✗ Registering with myoushq.com: ${phase.replace(/^error:?\s*/, "")}`
      : browserDone ? `⟳ Registering with myoushq.com${clock}` : "○ Registering with myoushq.com",
  ];
  items.forEach((li, i) => {
    setMarked(li, rows[i]);
    li.className = rows[i].startsWith("○") ? "todo" : rows[i].startsWith("✗") ? "bad" : "";
  });
  const slow = elapsed > 300 || (pulled && elapsed > 180);
  setText($("start-note"), slow
    ? "This is taking longer than usual. The log shows what's happening (Show app log, in the ⋯ menu)."
    : `About two minutes the first time (the image is about 2 GB). ${capitalize(sys)} must stay awake.`);
}

function fillPair(w: Worker) {
  const invite = w.invite;
  const code = str(invite?.code);
  const msg = pairingMessage(invite, w.config.name);
  const area = $<HTMLTextAreaElement>("pair-message");
  if (area.value !== msg) {
    area.value = msg;
    // A Mac shows the message as a block of text, as tall as it is.
    if (info.os === "macos") {
      area.style.height = "0"; // so scrollHeight is the text's, not the rows
      area.style.height = `${area.scrollHeight + 2}px`;
    }
  }
  const left = Math.max(0, num(invite?.expires_at) - m.now());
  setText($("pair-code"), `Code ${code} · valid for ${countdown(left)}`);
  $<HTMLProgressElement>("pair-bar").value = Math.min(1, left / INVITE_SECONDS);
  // The agents on this computer can be the other side.
  pairAgents = (m.agents ?? []).filter((a) => !a.error);
  const select = $<HTMLSelectElement>("pair-which");
  const titles = ["Another agent (paste the message)", ...pairAgents.map((a) => `${agentName(a)} on ${sys}`)];
  if ([...select.options].map((o) => o.text).join("|") !== titles.join("|")) {
    const keep = select.selectedIndex;
    select.replaceChildren(...titles.map((t, i) => el("option", { text: t, value: String(i) })));
    select.selectedIndex = keep > 0 && keep < titles.length ? keep : m.fake === "pairlocal" ? 1 : 0;
  }
  show($("pair-which-row"), titles.length > 1);
  pairWhichChanged();
}

function pairWhichChanged() {
  const i = $<HTMLSelectElement>("pair-which").selectedIndex;
  const a = i > 0 ? pairAgents[i - 1] : undefined;
  show($("pair-local"), !!a);
  show($("pair-copy"), !a);
  if (a) setText($("pair-local"), `Let ${agentName(a)} accept the code`);
}

function fillPaired(w: Worker) {
  setText($("paired-text"), `Paired with ${str(w.paired?.alias) || "your agent"}.`);
  setText($("paired-code"), str(w.paired?.verify));
}

function outcome(r: Record<string, unknown>): [string, string] {
  const exit = num(r.exit);
  if (str(r.decision) === "refuse") return ["refused", "refused"];
  if (str(r.decision) === "pending") return ["waiting for you", "waiting"];
  if (!("done_at" in r)) return ["running…", "quiet"];
  if (r.stopped === true) return ["stopped by you", "stopped"];
  if ("exit" in r && exit !== 0) return [`exit ${exit}`, "failed"];
  return ["ok", "quiet"];
}

function fillRequests(w: Worker) {
  const now = m.now();
  show($("stop-command"), !!w.runningRequest(now));
  show($("paused-note"), w.paused);
  show($("requests-log-row"), info.os === "macos"); // the log has no ⋯ menu to be in there
  setText($("pause"), w.paused ? "Resume" : "Pause");
  setText($("requests-title"), w.requests.length ? `${w.requests.length} recent, newest first` : `Nothing yet. Ask your agent to run something on ${sys}.`);
  const key = `${w.home}|${w.config.seenRequestsAt}|${w.requests.map((r) => `${str(r.id)}:${"done_at" in r}:${str(r.decision)}`).join(",")}|${dayLabel(now, now)}`;
  if (key === lastRequestsKey) return;
  lastRequestsKey = key;
  const body = $("requests").querySelector("tbody")!;
  body.replaceChildren(...w.requests.map((r) => {
    const at = num(r.at);
    const day = dayLabel(at, now);
    const op = { exec: "ran", put: "put", get: "got" }[str(r.op)] ?? str(r.op);
    let text = what(r);
    if (num(r.size) > 0) text += ` (${byteCount(num(r.size))})`;
    if (str(r.decision) === "refuse" && str(r.reason)) text += ` — ${str(r.reason)}`;
    const [out, cls] = outcome(r);
    const tr = el("tr", { className: at > w.config.seenRequestsAt ? "new" : "" },
      el("td", { textContent: day === "Today" ? clockTime(at) : day }),
      el("td", { textContent: str(r.alias) || "?" }),
      el("td", { textContent: op }),
      el("td", { textContent: text, title: text }),
      el("td", { textContent: out, className: cls }));
    tr.onclick = () => void showRequestDetail(r);
    tr.tabIndex = 0;
    tr.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); void showRequestDetail(r); } };
    return tr;
  }));
}

function showRequestDetail(r: Record<string, unknown>) {
  const now = m.now();
  const lines = [`${dayLabel(num(r.at), now)}  ${clockTime(num(r.at))}  ${str(r.alias)}`, ""];
  lines.push(str(r.op) === "exec" ? `$ ${str(r.cmd)}` : `${str(r.op)} ${str(r.path)}`);
  if (str(r.decision) === "refuse") {
    lines.push("", `Refused: ${str(r.reason) || "by the review hook"}`);
  } else {
    let s = "";
    if ("exit" in r) s += `Exit ${num(r.exit)}`;
    if (num(r.duration)) s += ` in ${num(r.duration).toFixed(1)} s`;
    if (s) lines.push("", s);
    if (num(r.size)) lines.push(`${num(r.size)} bytes`);
    if (str(r.stdout)) lines.push("", "--- output ---", str(r.stdout));
    if (str(r.stderr)) lines.push("", "--- errors ---", str(r.stderr));
    if (r.truncated === true) lines.push("", "[output was cut to fit]");
  }
  return dialog({ title: "Request", pre: lines.join("\n") });
}

function fillApproval(w: Worker) {
  const q = w.approvals[0];
  show($("approval-row"), !!q && !m.selectedAgent);
  if (!q) return;
  const left = num(q.asked_at) + num(q.wait) - m.now();
  setText($("approval-text"), `${str(q.alias) || "Your agent"} wants to ${verb(str(q.op))} ${what(q)}${left > 0 ? `  (${Math.floor(left)} s left)` : ""}`);
}

async function answer(verdict: "allow" | "refuse") {
  const w = m.current;
  const q = w.approvals[0];
  if (!q) return;
  if (!m.fake) await be.answerApproval(w.home, str(q.id), verdict);
  appendLog(`${verdict}: ${str(q.op)} ${what(q)}`);
  w.approvals = [];
  render();
}

function fillAgent(a: Agent) {
  $("head-dot").className = kindClass("none");
  setText($("head-title"), `${agentName(a)} · agent on ${sys}`);
  setText($("head-right"), a.error ? "" : plural(agentContacts(a).length, "contact"));
  show($("head-facts"), false);
  show($("banner"), false);
  const s = a.status ?? {};
  if (a.error) {
    setText($("agent-info"), `${a.error}\n${a.home}`);
  } else {
    const last = num(s.last_used) ? timeAgo(num(s.last_used), m.now()) : "never";
    setText($("agent-info"), `${shortNpub(str(s.identity))}\n${str(s.client)} client ${str(s.version)} · last used ${last} · ${num(s.unread)} unread\n${a.home}`);
  }
  const pending = Array.isArray(s.pending_pairings) ? (s.pending_pairings as unknown[]).map(str).filter(Boolean) : [];
  setText($("agent-pending"), pending.length ? `Pairing in progress: ${pending.join(", ")}` : "");
  const body = $("agent-contacts").querySelector("tbody")!;
  body.replaceChildren(...agentContacts(a).map((c) => el("tr", {},
    el("td", { textContent: str(c.alias) || str(c.npub) }),
    el("td", { textContent: str(c.status) === "blocked" ? "blocked" : str(c.relationship) }),
    el("td", { textContent: num(c.paired_at) ? dayLabel(num(c.paired_at), m.now()) : "" }),
    el("td", { textContent: str(c.added_by) === "owner" ? "paired by you" : str(c.added_by) }))));
}

function fillSidebar() {
  const agents = m.agents ?? [];
  const on = m.workers.length > 1 || agents.length > 0;
  show($("sidebar"), on);
  if (!on) return;
  setText($("side-title"), `On ${sys}`);
  const rows: HTMLElement[] = [el("div", { className: "side-head", textContent: "WORKERS" })];
  for (const w of m.workers) {
    const row = el("div", { className: `side-row${!m.selectedAgent && w === m.current ? " selected" : ""}` }, el("span", { className: kindClass(w.stateKind) }), el("span", { textContent: w.name, title: w.name }));
    row.onclick = () => { m.current = w; m.selectedAgent = null; setupFilledFor = ""; render(); };
    rows.push(row);
  }
  if (agents.length) {
    rows.push(el("div", { className: "side-head", textContent: "AGENTS" }));
    for (const a of agents) {
      const row = el("div", { className: `side-row${m.selectedAgent?.home === a.home ? " selected" : ""}` }, el("span", { className: kindClass(a.error ? "grey" : "none") }), el("span", { textContent: agentName(a), title: agentName(a) }));
      row.onclick = () => { m.selectedAgent = a; render(); };
      rows.push(row);
    }
  }
  for (const r of rows) {
    if (!r.classList.contains("side-row")) continue;
    r.tabIndex = 0;
    r.setAttribute("role", "button");
    r.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); r.click(); } };
  }
  $("side-list").replaceChildren(...rows);
}

// --- the tray ----------------------------------------------------------------------

const KIND_COLORS: Record<Kind, string> = { grey: "#8e8e93", blue: "#0a84ff", green: "#2fa84f", orange: "#e8890c", red: "#e5484d" };

/** The mark (worker/icons/mark.svg) in its 32 by 32 box: a rounded square, and a mu drawn as one stroke with a dot on each free end. */
const MARK_SQUARE = "M7 0H25A7 7 0 0 1 32 7V25A7 7 0 0 1 25 32H7A7 7 0 0 1 0 25V7A7 7 0 0 1 7 0Z";
const MARK_MU = "M7.5 6.5V25.5M7.5 13.5a5.25 5.25 0 0 0 10.5 0M18 6.5V15C18 21.5 22 24 26.25 24";
const MARK_DOTS: [number, number][] = [[7.5, 6.5], [7.5, 25.5], [18, 6.5], [26.25, 24]];

/** The mark on a square in `color`, scaled from its 32-point box and moved in by `offset` pixels. */
function drawMark(g: CanvasRenderingContext2D, scale: number, offset: number, color: string) {
  g.setTransform(scale, 0, 0, scale, offset, offset);
  g.fillStyle = color;
  g.fill(new Path2D(MARK_SQUARE));
  g.strokeStyle = g.fillStyle = "#ffffff";
  g.lineWidth = 2.75;
  g.lineJoin = "round";
  g.stroke(new Path2D(MARK_MU));
  for (const [x, y] of MARK_DOTS) {
    g.beginPath();
    g.arc(x, y, 3.1, 0, Math.PI * 2);
    g.fill();
  }
  g.setTransform(1, 0, 0, 1, 0, 0);
}

/** The tray icon: the mark on a square in the state's colour, a white dot in its corner while a request runs.
 * A Mac's menu bar shows any icon 18 points high, so there it is drawn at 36 pixels (two per point on a
 * Retina screen, halved evenly on others) with myous for Mac's proportions (Icon.m): the square one point
 * in from each edge, the dot 5.2 points wide in the top right corner. */
function trayIcon(kind: Kind, dot: boolean): string {
  const mac = info.os === "macos";
  const c = document.createElement("canvas");
  c.width = c.height = mac ? 36 : 64;
  const g = c.getContext("2d");
  if (!g) return "";
  drawMark(g, mac ? 1 : 60 / 32, 2, KIND_COLORS[kind]);
  if (dot) {
    const [x, y, r, w] = mac ? [28, 8, 5.2, 2.4] : [52, 12, 9, 3];
    g.fillStyle = "#ffffff";
    g.strokeStyle = KIND_COLORS[kind];
    g.lineWidth = w;
    g.beginPath();
    g.arc(x, y, r, 0, Math.PI * 2);
    g.fill();
    g.stroke();
  }
  return c.toDataURL("image/png").replace(/^data:image\/png;base64,/, "");
}

const menuActions: MenuActions = {
  show: (w) => void showWindow(w),
  showAgent: (i) => { m.selectedAgent = (m.agents ?? [])[i] ?? null; void showWindow(); },
  openBrowser: (w) => void openBrowserView(w),
  togglePaused: (w) => void togglePaused(w),
  start: (w) => void start(w),
  stop: (w) => void stop(w),
  removeWorker: (w) => { m.current = w; void removeWorker(); },
  addWorker: () => void addWorker(),
  settings: () => void openSettings(),
  checkUpdates: () => void checkForUpdates(true),
  useImage: () => void useImage(),
  chooseRepo: () => void chooseRepo(),
  useDirect: () => void useDirect(),
  useContainers: () => void useContainers(),
  showLog: () => void showLog(),
  containerLog: () => void containerLog(),
  openFolder: () => void be.openPath(m.current.home),
  removeContainers: () => void removeContainers(),
  addAgentHome: () => void addAgentHome(),
  quit: () => void be.quit(),
};

function renderTray() {
  const { items, actions } = buildMenu(m, windowShown, menuActions, sys);
  trayActions = actions;
  const kind = m.worstKind;
  const dot = m.current.requestInProgress(m.now());
  const iconKey = `${kind}${dot}`;
  const itemsKey = JSON.stringify(items);
  const n = m.newRequests;
  // A Mac shows the count next to the menu bar icon and on the Dock icon, as myous for Mac does.
  const fresh = windowShown ? 0 : n;
  if (info.os === "macos" && fresh !== lastCounts) {
    lastCounts = fresh;
    void be.setCounts(fresh).catch((e) => appendLog(`counts: ${e}`));
  }
  const tooltip = `myous · ${m.workers.map((w) => `${w.name}: ${w.stateWord}`).join(" · ")}${n > 0 && !windowShown ? ` · ${n} new` : ""}`;
  if (iconKey === lastTrayIcon && itemsKey === lastTrayItems) return;
  const icon = iconKey === lastTrayIcon ? null : trayIcon(kind, dot);
  lastTrayIcon = iconKey;
  lastTrayItems = itemsKey;
  void be.setTray(icon || null, tooltip, items as TrayItem[]).catch((e) => appendLog(`tray: ${e}`));
}

// --- the ⋯ menu in the window ----------------------------------------------------------

function openMoreMenu() {
  const menu = $("more-menu");
  if (!menu.hidden) {
    menu.hidden = true;
    return;
  }
  menu.replaceChildren(...moreMenu(m, menuActions).map((e) => {
    if (!e) return el("hr");
    const b = el("button", { textContent: e[0] });
    b.onclick = () => { menu.hidden = true; e[1](); };
    return b;
  }));
  menu.hidden = false;
}

/** A Mac: the ⋯ menu's entries in the app menu, sent when they change. */
function renderAppMenu() {
  if (info.os !== "macos") return;
  const { items, actions } = appMenu(m, menuActions);
  appMenuActions = actions;
  const key = JSON.stringify(items);
  if (key === lastAppMenu) return;
  lastAppMenu = key;
  void be.setAppMenu(items).catch((e) => appendLog(`app menu: ${e}`));
}

// --- a Mac's window fits its content ---------------------------------------------------

/** As myous for Mac: 560 points wide (and the list of workers when it shows), as high as what the
 * screen shows (Rust keeps it on the screen), the top edge staying where it is. An open dialog
 * gets the room it needs. Asked whenever the content's size changes. */
const FIT_WIDTH = 560, FIT_SIDEBAR = 150, FIT_MIN = 320, FIT_MAX = 760;
let lastFit = "";
function fitWindow() {
  if (info.os !== "macos") return;
  const width = FIT_WIDTH + ($("sidebar").hidden ? 0 : FIT_SIDEBAR);
  let height = $("layout").offsetHeight;
  const d = $<HTMLDialogElement>("dlg");
  if (d.open) {
    // What its scrolled parts (the settings, a log) would need, within the 86% of the window a dialog takes.
    const extra = [...d.querySelectorAll<HTMLElement>(".settings, pre")].reduce((n, e) => n + e.scrollHeight - e.clientHeight, 0);
    height = Math.max(height, Math.ceil((d.offsetHeight + extra) / 0.86) + 8);
  }
  height = Math.min(Math.max(Math.ceil(height), FIT_MIN), FIT_MAX);
  // Asked again when the page's own height changed (the window took the size; its title bar shows now).
  const key = `${width}x${height}@${innerHeight}`;
  if (key === lastFit) return;
  lastFit = key;
  void be.fitWindow(width, height, innerHeight).catch((e) => appendLog(`window size: ${e}`));
}

// --- actions ------------------------------------------------------------------------

async function showLog() {
  await dialog({ title: "myous log", pre: logLines.join("\n") || "Nothing yet.", liveLog: true });
}

async function containerLog() {
  if (m.current.config.isDirect) {
    const text = await be.workerLog(m.current.home);
    await dialog({ title: "Worker log", pre: text || "Nothing yet." });
    return;
  }
  const text = await be.containerLogs(composePath(m.current), m.current.project);
  await dialog({ title: "Container log", pre: text || "No output (the worker may not be running)." });
}

async function removeContainers() {
  appendLog(await be.removeStaleContainers());
  void refresh();
}

async function useImage() {
  const w = m.current;
  if (w.alive && w.config.isDirect) await stop(w);
  w.config.mode = "image";
  await saveConfig(w);
  appendLog("mode: the published image");
  void refresh();
}

/** One worker without a container (⋯ menu), after a warning. */
async function useDirect() {
  const w = m.current;
  if (!(await confirmBox("Run without a container?", `Commands your agent sends will run on ${sys} as you, with everything you can reach, and only the review stands in the way (it starts at "Ask me before anything that changes things"). Its browser, if you want one, runs on ${sys} too and needs Python. A running worker restarts.`, "Run without a container", true))) return;
  await goDirect([w]);
  appendLog("mode: no container");
}

/** From the runtime card: this computer runs workers without containers. */
async function continueWithoutContainers() {
  m.appConfig.containersOff = true;
  await saveConfig(m.workers[0]);
  await goDirect(m.workers);
  appendLog("no containers on this computer: workers run without one");
}

async function goDirect(workers: Worker[]) {
  for (const w of workers) {
    const restart = w.alive && !w.config.isDirect;
    if (restart) await stop(w);
    w.config.mode = "direct";
    await saveConfig(w);
    // No container around the commands: ask before anything that changes things, unless the owner chose otherwise.
    if (!m.fake && !(await be.fileExists(`${w.home}${sep}review.json`))) await be.setReview(w.home, "changes");
    if (restart) w.restartAfterStop = true;
  }
  setupFilledFor = "";
  void refresh();
}

/** Undo "no containers": look for a runtime again; workers go back to the published image. */
async function useContainers() {
  const app = m.appConfig;
  app.containersOff = false;
  await saveConfig(m.workers[0]);
  for (const w of m.workers) {
    if (!w.config.isDirect) continue;
    if (w.alive) await stop(w);
    w.config.mode = "image";
    await saveConfig(w);
  }
  appendLog("mode: containers (the published image)");
  m.capability = null;
  setupFilledFor = "";
  await probeRuntime();
  void refresh();
}

async function enableWsl() {
  const b = $<HTMLButtonElement>("wsl-enable");
  b.disabled = true;
  appendLog("turning on WSL (wsl --install --no-distribution)");
  const r = await be.enableWsl();
  b.disabled = false;
  if (r.output) appendLog(r.output);
  if (r.status === 0) {
    m.wslPending = true;
    await alertBox("Restart Windows to finish", "WSL is turned on. After the restart, open myous again to get Docker Desktop or Rancher Desktop.");
  } else {
    await alertBox("WSL wasn't turned on", `${r.output.trim().split("\n").pop() || `exit ${r.status}`}. You can also run wsl --install in a terminal opened as administrator.`);
  }
  render();
}

async function checkCapability() {
  if (m.fake) return;
  if (info.os !== "windows") return m.checks.containers.settle("ok", "", m.now());
  if (capabilityChecking) return;
  capabilityChecking = true;
  m.checks.containers.begin(m.now());
  let c;
  try {
    c = await be.containersCheck();
  } finally {
    capabilityChecking = false;
  }
  const changed = JSON.stringify(c) !== JSON.stringify(m.capability);
  m.capability = c;
  m.checks.containers.settle(c.state, c.reason, m.now());
  if (changed) appendLog(`containers on ${sys}: ${c.state}${c.reason ? ` (${c.reason}${c.machine ? `, ${c.machine}` : ""})` : ""}`);
  render();
}

/** One GET of the hub's /config.json (5 s at most); the red line while it fails. */
async function checkHub() {
  if ((m.fake && !be.preview) || hubChecking) return;
  hubChecking = true;
  lastHubCheck = m.now();
  m.checks.hub.begin(m.now());
  try {
    const r = await be.hubCheck();
    const was = m.checks.hub.state;
    m.checks.hub.settle(r.state, r.detail, m.now());
    if (r.state !== was) appendLog(r.state === "ok" ? `${info.hub} answers` : `can't reach ${info.hub}: ${r.detail}`);
  } catch (e) {
    m.checks.hub.settle("unknown", "", m.now());
    appendLog(`hub check: ${e}`);
  } finally {
    hubChecking = false;
  }
  render();
}

/** "Retry" on a check that couldn't answer. */
function retryCheck(name: CheckName) {
  m.checks[name].retry(m.now());
  if (name === "runtime") void probeRuntime();
  if (name === "containers") void checkCapability();
  if (name === "hub") void checkHub();
  if (name === "browser") void checkBrowser(true);
  if (name === "agents") m.agents = null;
  void refresh();
}

async function chooseRepo() {
  const path = await be.pickFolder();
  if (!path) return;
  if (!(await be.fileExists(`${path}${sep}worker${sep}compose.yml`))) {
    await alertBox("Not a checkout", "That folder has no worker/compose.yml. Pick the myoushq-client checkout.");
    return;
  }
  const w = m.current;
  if (w.alive && w.config.isDirect) await stop(w);
  w.config.repo = path;
  w.config.mode = "docker";
  await saveConfig(w);
  appendLog(`mode: checkout at ${path}`);
  void refresh();
}

async function addWorker() {
  if (m.fake) return;
  const home = await be.newWorker();
  m.load(await be.readWorkers());
  const w = m.workers.find((x) => x.home === home);
  if (w) {
    m.current = w;
    m.selectedAgent = null;
  }
  setupFilledFor = "";
  for (const x of m.workers) m.evaluate(x);
  await showWindow();
}

async function removeWorker() {
  const w = m.current;
  if (w.isDefault || m.fake) return;
  if (w.isSetUp && !(await confirmBox(`Remove ${w.name}?`, "Its folder with its key, logins and files is moved to the trash, and the agent paired with it loses it.", "Remove", true))) return;
  await stop(w);
  setTimeout(async () => {
    try {
      await be.removeWorker(w.home);
    } catch (e) {
      appendLog(`could not remove ${w.home}: ${e}`);
    }
    m.current = m.workers[0];
    void refresh();
  }, 3000);
}

async function addAgentHome() {
  const path = await be.pickFolder();
  if (!path) return;
  const c = m.appConfig;
  c.agentHomes = [...c.agentHomes, path];
  await saveConfig(m.workers[0]);
  m.agents = null;
  void refresh();
}

async function acceptAsLocalAgent() {
  const i = $<HTMLSelectElement>("pair-which").selectedIndex;
  const a = pairAgents[i - 1];
  const code = str(m.current.invite?.code);
  if (!a || !code || m.fake) return;
  const btn = $<HTMLButtonElement>("pair-local");
  btn.disabled = true;
  setText($("pair-wait"), `Asking ${agentName(a)} to accept the code…`);
  const r = await be.agentRun(a.home, a.binary, ["accept", code, "--wait", "90", "--added-by", "owner", "--relationship", "other", "--sharing", "my own worker; run commands there for me"]);
  btn.disabled = false;
  if (r.status !== 0) {
    setText($("pair-wait"), `The agent couldn't accept the code: ${r.output.trim().split("\n").pop()}`);
    appendLog(`accept as ${agentName(a)}: ${r.output}`);
    return;
  }
  setText($("pair-wait"), `${agentName(a)} accepted. The worker confirms in a moment.`);
  m.agents = null;
}

async function pairAgentWithCode() {
  const a = m.selectedAgent;
  if (!a || m.fake) return;
  const r = await dialog({ title: `Pair ${agentName(a)}`, text: `The code or link the other side gave you. ${agentName(a)} accepts it as itself; the contact is marked as paired by you.`,
    input: { placeholder: "4821-K7F3QX or https://myoushq.com/p/4821#K7F3QX" }, buttons: [{ label: "Cancel", value: "no" }, { label: "Pair", value: "yes", primary: true }] });
  const code = r?.value === "yes" ? r.input.trim() : "";
  if (!code) return;
  const out = await be.agentRun(a.home, a.binary, ["accept", code, "--wait", "90", "--added-by", "owner"]);
  await alertBox(out.status === 0 ? "Paired" : "Couldn't pair", out.output.trim().split("\n").pop() ?? "");
  m.agents = null;
  void refresh();
}

async function checkForUpdates(manual: boolean) {
  let latest = "", error = "";
  try {
    latest = await be.latestRelease();
  } catch (e) {
    error = String(e);
  }
  const app = m.appConfig;
  app.lastUpdateCheck = m.now();
  await saveConfig(m.workers[0]);
  const isNewer = !error && newer(latest, m.version);
  m.latestRelease = isNewer ? latest : "";
  if (isNewer) m.notifyOnce(m.current, `update-${latest}`, `myous ${latest} is available`, "Open myous to download it.");
  if (manual) {
    if (error) await alertBox("Couldn't check", error);
    else if (isNewer) {
      if (await confirmBox(`myous ${latest} is available`, `You have ${m.version}. The download replaces the app; the worker keeps its identity and logins.`, "Download")) {
        await be.openUrl(info.hub + info.download_path);
      }
    } else await alertBox("You're up to date", `You have ${m.version}.`);
  }
  render();
}

// --- settings --------------------------------------------------------------------------

const REVIEW: [string, string][] = [["trust", "Trust my agent: everything it asks runs"], ["changes", "Ask me before anything that changes things"], ["all", "Ask me before every request"]];

async function openSettings() {
  const w = m.current, app = m.appConfig;
  const body = el("div", { className: "settings" });
  const name = el<HTMLInputElement>("input", { value: w.config.name });
  const desc = el<HTMLTextAreaElement>("textarea", { rows: 4, value: w.config.effectiveDescription });
  const review = el<HTMLSelectElement>("select");
  review.append(...REVIEW.map(([v, t]) => el("option", { value: v, text: t, selected: v === w.review })));
  const check = (label: string, on: boolean, cls = "") => {
    const box = el<HTMLInputElement>("input", { type: "checkbox", checked: on });
    return [el("label", { className: `check ${cls}` }, box, label), box] as const;
  };
  const mac = info.os === "macos";
  const [dockRow, dock] = check("Show an icon in the Dock as well as the menu bar", app.dock);
  const [loginRow, login] = check("Open myous at login", m.fake ? false : await be.autostartGet());
  const [notifyRow, notify] = check("Notify me", app.notifications);
  const kinds = notifyKinds.map(([k, label]) => check(label, app.notifies(k), "indent"));
  const sync = () => kinds.forEach(([, b]) => (b.disabled = !notify.checked));
  notify.onchange = sync;
  sync();
  const [updateRow, update] = check("Check for a new version daily", app.autoUpdate);
  const [agentsRow, agents] = check(`Show the agents on ${sys} (their myous folders)`, app.showAgents);
  const [browserCheckEl, browserBox] = check(`A worker browser on ${sys} (it needs Python; it starts with the worker)`, w.config.hostBrowser);
  const browserRowEl = el("div", {}, browserCheckEl);
  if (w.alive) browserRowEl.append(el("p", { className: "hint indent", textContent: "A change takes effect at the worker's next start; a browser that is open stays until the worker stops." }));
  body.append(
    el("label", { textContent: "Worker name" }), name,
    el("p", { className: "hint", textContent: "The name your agent sees. A new name applies at the next start, and the agent is told (it keeps its own name for this computer until it renames)." }),
    el("label", { textContent: "Description" }), desc,
    el("p", { className: "hint", textContent: "How it describes this computer to your agent (its card). Sent at the next start." }),
    el("label", { textContent: "Review" }), review,
    ...(w.config.isDirect ? [browserRowEl] : []),
    el("p", { className: "hint", textContent: "Questions arrive as notifications and at the top of the Requests list; an agent waits up to two minutes for your answer." }),
    ...(mac ? [dockRow] : []), loginRow, notifyRow, ...kinds.map(([row]) => row), updateRow, agentsRow,
  );
  const r = await dialog({ title: "myous settings", body, buttons: [{ label: "Cancel", value: "no" }, { label: "Save", value: "yes", primary: true }] });
  if (r?.value !== "yes") return;
  if (name.value.trim()) w.config.name = name.value.trim();
  const d = desc.value.trim();
  w.config.description = isDefaultDescription(d, w.config.name, w.config.isDirect) ? "" : d;
  if (w.config.isDirect && browserBox.checked === w.config.browserOff) {
    w.config.browserOff = !browserBox.checked;
    appendLog(w.config.browserOff ? `${w.name}: no worker browser` : `${w.name}: worker browser on ${sys}`);
    if (!w.config.browserOff) void checkBrowser(true);
  }
  app.notifications = notify.checked;
  notifyKinds.forEach(([k], i) => app.setNotifies(k, kinds[i][1].checked));
  app.autoUpdate = update.checked;
  const dockChanged = mac && dock.checked !== app.dock;
  if (mac) app.dock = dock.checked;
  if (agents.checked !== app.showAgents) m.agents = null;
  app.showAgents = agents.checked;
  if (dockChanged) void be.setDock(app.dock).catch((e) => appendLog(`Dock icon: ${e}`));
  if (m.fake) return render();
  await saveConfig(w);
  if (app !== w.config) await saveConfig(m.workers[0]);
  if (review.value !== w.review) {
    await be.setReview(w.home, review.value);
    appendLog(`review level: ${review.value}`);
  }
  if (login.checked !== (await be.autostartGet())) {
    try {
      await be.autostartSet(login.checked);
    } catch (e) {
      appendLog(`open at login: ${e}`);
    }
  }
  void refresh();
}

// --- the refresh loop -------------------------------------------------------------------

async function probeRuntime() {
  if (runtimeProbing) return;
  runtimeProbing = true;
  const rt = m.checks.runtime;
  rt.begin(m.now());
  try {
    const r = await be.probeRuntime();
    const changed = r.state !== rt.state || r.name !== m.runtimeName || r.detail !== rt.detail;
    rt.settle(r.state, r.detail, m.now());
    if (r.state !== "unknown") m.runtimeName = r.name;
    // A runtime that answers is proof that containers run here.
    if (r.state === "ok") m.checks.containers.settle("ok", "", m.now());
    if (changed) {
      appendLog(`container runtime: ${r.state}${r.name ? ` (${r.name})` : ""}${r.detail ? `: ${r.detail}` : ""}`);
      m.options = null;
    }
    // No runtime on Windows: can there be one (WSL 2)? Asked now and then, as WSL can be turned on meanwhile.
    if (r.state === "missing" && (m.capability === null || m.ticks % 30 === 0)) void checkCapability();
    if (changed) void refresh();
  } catch (e) {
    rt.settle("unknown", "", m.now());
    appendLog(`runtime check: ${e}`);
  } finally {
    runtimeProbing = false;
  }
}

async function readAgents() {
  if (agentsLoading) return;
  agentsLoading = true;
  m.checks.agents.begin(m.now());
  try {
    const list = await be.readAgents(m.appConfig.agentHomes);
    m.agents = list;
    m.checks.agents.settle("ok", "", m.now());
    if (m.selectedAgent) m.selectedAgent = list.find((a) => a.home === m.selectedAgent!.home) ?? null;
    render();
  } finally {
    agentsLoading = false;
  }
}

async function refresh() {
  if (busy) return;
  busy = true;
  try {
    if (!m.fake) {
      m.ticks++;
      m.load(await be.readWorkers());
      // While a Set up card is up the runtime is asked every tick (2 s), so installing or starting one shows at once.
      const setupUp = m.current.screen === "setup" || m.current.screen === "noruntime";
      if (m.current.config.usesDocker && !m.appConfig.containersOff && (setupUp || m.ticks % 5 === 0)) void probeRuntime();
      if ((setupUp || m.checks.hub.state === "down") && m.now() - lastHubCheck >= HUB_EVERY) void checkHub();
      // The worker browser's needs, while something shows them: every 5 s then, else every 5 min.
      const browserShown = m.current.config.hostBrowser && m.browserStep(m.current) !== "ready";
      if (m.now() - lastBrowserCheck >= (browserShown ? 5 : 300)) void checkBrowser();
      if (m.appConfig.showAgents && (m.agents === null || m.ticks % AGENTS_EVERY === 0)) void readAgents();
      if (!m.appConfig.showAgents) {
        m.agents = [];
        m.selectedAgent = null;
      }
    }
    for (const w of m.workers) m.evaluate(w);
    // A finished install's line stays until the card moves on.
    const step = m.runtimeStep();
    if (step !== lastStep && lastStep !== "none" && m.install && !m.install.running) m.install = null;
    lastStep = step;
    render();
  } catch (e) {
    appendLog(`refresh: ${e}`);
  } finally {
    busy = false;
  }
}

// --- wiring --------------------------------------------------------------------------------

function wire() {
  $("more").onclick = (e) => { e.stopPropagation(); openMoreMenu(); };
  document.addEventListener("click", (e) => { if (!$("more-menu").contains(e.target as Node)) $("more-menu").hidden = true; });
  $("banner-button").onclick = () => {
    switch ($("banner-button").textContent) {
      case "Start": return void start(m.current);
      case "Show log": return void showLog();
      case "Download": return void be.openUrl(info.hub + info.download_path);
      case "Retry": m.checks.hub.retry(m.now()); return void checkHub();
    }
  };
  $("setup-name").oninput = () => {
    const name = $<HTMLInputElement>("setup-name"), desc = $<HTMLTextAreaElement>("setup-desc");
    const direct = m.current.config.isDirect;
    const browser = !direct || m.current.config.hostBrowser;
    if (isDefaultDescription(desc.value, name.dataset.prev ?? "", direct)) desc.value = defaultDescription(name.value.trim(), direct, browser);
    name.dataset.prev = name.value.trim();
  };
  $("setup-start").onclick = async () => {
    const w = m.current;
    const name = $<HTMLInputElement>("setup-name").value.trim();
    if (!name) return $("setup-name").focus();
    w.config.name = name;
    const d = $<HTMLTextAreaElement>("setup-desc").value.trim();
    w.config.description = isDefaultDescription(d, name, w.config.isDirect) ? "" : d;
    await saveConfig(w);
    await start(w);
    void refresh();
  };
  $("setup-remove").onclick = $("stopped-remove").onclick = () => void removeWorker();
  $("runtime-fix").onclick = () => runtimeFix();
  $("runtime-copy").onclick = async () => {
    await be.copyText($("runtime-lines").textContent ?? "");
    setText($("runtime-copy"), "Copied");
    setTimeout(() => setText($("runtime-copy"), "Copy"), 2000);
  };
  $("runtime-link").onclick = () => { if (m.options?.compose_url) void be.openUrl(m.options.compose_url); };
  $("attention-show-log").onclick = () => void (m.current.config.isDirect ? containerLog() : showLog());
  $("attention-retry").onclick = () => void tryAgain();
  $("wsl-enable").onclick = () => void enableWsl();
  $("direct-toggle").onclick = () => { directOpen = !directOpen; render(); };
  $("direct-use").onclick = () => void continueWithoutContainers();
  $("pair-copy").onclick = async () => {
    await be.copyText($<HTMLTextAreaElement>("pair-message").value);
    setText($("pair-wait"), "Copied. Paste it to your agent; this screen moves on when it accepts.");
  };
  $("pair-qr").onclick = async () => {
    const link = str(m.current.invite?.link) || str(m.current.invite?.code);
    if (link) await dialog({ title: "Pair with your phone's agent", img: await be.qrPng(link), text: link });
  };
  $("pair-new").onclick = async () => {
    if (!m.fake) await be.sendCommand(m.current.home, "new-code");
    setText($("pair-wait"), "Asking the worker for a new code…");
  };
  $("pair-which").onchange = pairWhichChanged;
  $("pair-local").onclick = () => void acceptAsLocalAgent();
  $("paired-continue").onclick = async () => {
    m.current.config.seenPairedAt = m.now();
    await saveConfig(m.current);
    void refresh();
  };
  $("unpair").onclick = async () => {
    if (!(await confirmBox("Unpair this worker?", "The agent loses access, and a new pairing code appears. Use this if the verification numbers differ.", "Unpair", true))) return;
    const w = m.current;
    if (!m.fake) await be.sendCommand(w.home, "unpair");
    w.config.seenPairedAt = m.now();
    await saveConfig(w);
    void refresh();
  };
  $("pause").onclick = () => void togglePaused();
  $("requests-log").onclick = () => void showLog();
  $("stop-command").onclick = async () => {
    const w = m.current;
    const r = w.runningRequest(m.now());
    if (!r) return;
    if (!m.fake) await be.sendCommand(w.home, `stop-${str(r.id)}`);
    appendLog(`stop: ${what(r)}`);
    ($("stop-command") as HTMLButtonElement).disabled = true;
    setTimeout(() => (($("stop-command") as HTMLButtonElement).disabled = false), 5000);
  };
  $("allow").onclick = () => void answer("allow");
  $("refuse").onclick = () => void answer("refuse");
  $("browser-open").onclick = () => void openBrowserView();
  $("stopped-start").onclick = () => void start(m.current);
  $("agent-refresh").onclick = () => { m.agents = null; void refresh(); };
  $("agent-pair").onclick = () => void pairAgentWithCode();
  $("side-add").onclick = () => void addWorker();

  // The tray's menu and a Mac's app menu (Tauri reports both through the tray's handler).
  be.listen("tray", (id: string) => (trayActions.get(id) ?? appMenuActions.get(id))?.());
  be.listen("window", (visible: boolean) => {
    windowShown = visible;
    if (visible) markSeen();
    render();
  });
  be.listen("app-log", (p: { home: string; line: string }) => {
    appendLog(p.line);
    if (m.install?.running) m.install.lastLine = p.line;
    if (m.browserInstall?.running && !p.line.startsWith("playwright: ")) m.browserInstall.lastLine = p.line;
    const lines = startLines.get(p.home);
    if (lines) {
      lines.push(p.line);
      if (lines.length > 30) lines.shift();
    }
    const w = m.workers.find((x) => x.home === p.home);
    if (w?.launchStage && /Pulled|Created|Started|Running/.test(p.line)) w.launchStage = "creating";
  });
  be.listen("install-done", (p: { id: string; status: number }) => {
    if (p.id === "playwright") {
      if (m.browserInstall) {
        m.browserInstall.running = false;
        m.browserInstall.status = p.status;
      }
      appendLog(`playwright: ${p.status === 0 ? "installed" : `exit ${p.status}`}`);
      void checkBrowser(true);
      render();
      return;
    }
    if (m.install?.id === p.id) {
      m.install.running = false;
      m.install.status = p.status;
    }
    appendLog(`${p.id}: ${p.status === 0 ? "done" : `exit ${p.status}`}`);
    m.options = null;
    void probeRuntime();
    render();
  });
  be.listen("compose-done", (p: { home: string; op: string; status: number }) => {
    const w = m.workers.find((x) => x.home === p.home);
    if (!w) return;
    if (p.op === "start") {
      w.launchStage = "";
      if (p.status !== 0) {
        appendLog("the start command failed; see above");
        // Not starting any more: "didn't start" now, with what it said, not after 90 s.
        w.failure = (startLines.get(w.home) ?? []).slice(-6);
        if (!w.alive) w.launchedAt = m.now() - 90;
      }
      startLines.delete(w.home);
    } else if (p.op === "stop" && p.status === 0) {
      w.stopDoneAt = m.now();
    } else if (p.op === "direct-exit") {
      appendLog(`myous worker exited with status ${p.status}`);
      if (w.stopping) w.stopDoneAt = m.now();
      else {
        // It ended by itself: the last lines of its log say why.
        void be.workerLog(w.home).then((t) => {
          const last = t.trim().split("\n").slice(-8);
          last.forEach((l) => appendLog(`worker.log: ${l}`));
          w.failure = last.filter((l) => l.trim());
          render();
        });
        if (w.launchedAt !== 0 && !w.alive) w.launchedAt = m.now() - 90; // not starting any more: "didn't start"
      }
    }
    void refresh();
  });
}

async function main() {
  info = await be.appInfo();
  // The system's look where it has its own (style.css's .os-macos); the other systems share one.
  document.documentElement.classList.add(`os-${info.os}`);
  setHostOs(info.os);
  sys = info.system_name;
  sep = info.os === "windows" ? "\\" : "/";
  m = new Model(info.version, info.product_name);
  m.os = info.os;
  m.notify = (kind, title, body) => {
    void be.notify(title, body, isUrgent(kind)).catch((e) => appendLog(`notification: ${e}`));
    if (isUrgent(kind)) void showWindow();
  };
  m.start = (w) => void start(w);
  m.windowVisible = () => windowShown;
  m.writeConfig = (w) => void saveConfig(w);
  wire();
  if (info.fake) {
    const t = Date.now() / 1000;
    m.now = () => t; // a still picture: the screenshots don't change while taken
    const fakeHome = (<Record<string, string>>{ windows: "C:\\Users\\max\\AppData\\Local\\myous\\worker", macos: "/Users/max/.myous-worker" })[info.os] ?? "/home/max/.myous-worker";
    applyFake(m, info.fake, fakeHome);
  } else {
    m.load(await be.readWorkers());
    // Every check at once, none waited for: the window shows "⟳" where one is still out.
    if (!m.appConfig.containersOff) {
      void probeRuntime();
      void checkCapability();
    }
    void checkHub();
    void checkBrowser();
  }
  for (const w of m.workers) m.evaluate(w);
  render();
  if (info.os === "macos") {
    fitWindow(); // before it shows, so it opens at its size
    const watch = new ResizeObserver(() => fitWindow());
    watch.observe($("layout"));
    watch.observe($("dlg"));
    window.addEventListener("resize", () => fitWindow());
  }
  if (m.current.screen !== "running" || be.preview) await showWindow();
  appendLog(`myous ${info.version} on ${info.os}${info.fake ? ` (made-up state: ${info.fake})` : ""}`);
  if (!m.fake) {
    setInterval(() => void refresh(), 2000);
    if (m.appConfig.autoUpdate && m.now() - m.appConfig.lastUpdateCheck > 86400) void checkForUpdates(false);
    setInterval(() => { if (m.appConfig.autoUpdate) void checkForUpdates(false); }, 86400 * 1000);
  }
  (window as unknown as { myous: object }).myous = { model: m, backend: be, preview: be instanceof PreviewBackend };
}

main().catch((e) => {
  document.body.textContent = `myous couldn't start: ${e}`;
  void be.logLine(`window failed to start: ${e}`);
});

// Made-up data for every screen, so each can be looked at (and
// screenshotted, and tested) without a worker: MYOUS_FAKE_STATE in the
// app, ?state= in a browser preview of ui/dist.

import { Agent, BrowserCheck, Container, MacBrowsers, Model, RuntimeOptions, Worker, WorkerFiles } from "./model.js";
import { advanceWords } from "./setup.js";

export const fakeStates = ["checking", "couldntcheck", "setup", "hubdown", "noruntime", "installing", "stoppedruntime", "stoppedengine",
  "nopermission", "nocompose", "runtimeerror", "nowsl", "novirt", "direct", "directnobrowser", "directnopython", "directnovenv",
  "directnoplaywright", "found", "directinstalling", "directoff", "directstarting", "directrunning", "directbrowserclosed", "directbrowserlater", "starting", "failed", "pair",
  "paired", "running", "macbrowser", "paused", "stopping", "stopped", "approval", "busy", "agents", "agent", "pairlocal", "workers"] as const;


function files(home: string, extra: Partial<WorkerFiles> = {}): WorkerFiles {
  return { home, folder: home.split(/[\\/]/).pop()!, is_default: !/-\d+$/.test(home), project: "myous-worker", config: {},
    status: null, status_modified: 0, requests: [], approvals: [], paused: false, review: "trust", ...extra };
}

export function applyFake(m: Model, state: string, home = "/home/max/.myous-worker") {
  m.fake = state;
  const NAME = m.os === "macos" ? "Max's Mac" : "Max's PC";
  const now = m.now();
  // Every check answered: ok, no agents; the cases below change what they're about.
  for (const c of Object.values(m.checks)) {
    c.state = "ok";
    c.detail = "";
    c.unsureSince = 0;
  }
  m.runtimeName = "Docker Desktop";
  m.capability = { state: "ok", reason: "", machine: "" };
  m.options = previewOptions(m.os);
  m.install = null;
  m.browserCheck = previewBrowserCheck(m.os, "ready");
  m.macBrowsers = m.os === "macos" ? previewMacBrowsers(m.os) : null;
  m.browserInstall = null;
  m.seen = {};
  m.advances = {};
  m.agents = [];
  m.selectedAgent = null;
  const f = files(home, { config: { notifications: true, auto_update: true, show_agents: true } });
  const status = (phase: string, extra: Record<string, unknown> = {}) => {
    f.status = { phase, alias: NAME, npub: "npub1exampleq7x4f", contacts: 1, updated: now,
      paired: { alias: "Max's Muse", verify: "4821", at: now - 3600 }, last: { at: now - 125, op: "exec" }, ...extra };
    f.status_modified = now;
    f.config!.seen_paired_at = now - 1800; // the pairing an hour ago was acknowledged
  };
  const requests = () => {
    f.requests = [
      { id: "r4", op: "exec", alias: "Max's Muse", cmd: "python3 /work/quotes.py", at: now - 125, decision: "allow", done_at: now - 118, exit: 0, duration: 6.4, stdout: "BABA 95.4\nPRM 27.75\n" },
      { id: "r3", op: "put", alias: "Max's Muse", path: "quotes.py", size: 4977, at: now - 130, decision: "allow", done_at: now - 129 },
      { id: "r2", op: "exec", alias: "Max's Muse", cmd: "rm -rf /work/old", at: now - 3600, decision: "refuse", reason: "the owner didn't answer in time" },
      // Sent while the worker was off; when it came back, its agent had long stopped waiting.
      { id: "r6", op: "exec", alias: "Max's Muse", cmd: "python3 /work/quotes.py --all", at: now - 7200, done_at: now - 7200, decision: "expired",
        reason: "expired: sent 3 hours ago, its ttl is 150 seconds; not run" },
      { id: "r1", op: "get", alias: "Max's Muse", path: "report.pdf", size: 120340, at: now - 86400 * 1.5, decision: "allow", done_at: now - 86400 * 1.5 + 2 },
    ];
    f.config!.seen_requests_at = now - 1000;
  };
  const named = () => (f.config!.name = NAME);
  let launch: (w: Worker) => void = () => {};
  const extraWorkers: WorkerFiles[] = [];
  const direct = () => {
    f.config!.containers = false;
    f.config!.mode = "direct";
    f.review = "changes";
  };
  const runtime = (state: string, name: string, detail = "") => {
    m.runtimeState = state;
    m.runtimeName = name;
    m.checks.runtime.detail = detail;
  };
  switch (state) {
    case "checking":
      // Just launched: nothing has answered yet.
      for (const c of Object.values(m.checks)) {
        c.state = "";
        c.unsureSince = now - 1;
      }
      m.agents = null;
      break;
    case "couldntcheck":
      // The runtime and the hub didn't answer within 10 s; the agents did.
      m.checks.runtime.state = "";
      m.checks.runtime.unsureSince = now - 12;
      m.checks.hub.state = "";
      m.checks.hub.unsureSince = now - 12;
      m.agents = fakeAgents(now);
      break;
    case "hubdown":
      m.checks.hub.state = "down";
      m.checks.hub.detail = "Connection Failed: Connect error: connection refused";
      break;
    case "installing":
      runtime("missing", "Docker");
      m.install = { id: m.os === "windows" ? "docker-desktop" : "docker-engine", name: m.os === "windows" ? "Docker Desktop" : "Docker Engine", running: true, status: 0, lastLine: "" };
      break;
    case "stoppedengine":
      runtime("stopped", "Docker Engine", "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?");
      break;
    case "nopermission":
      runtime("permission", m.os === "windows" ? "Docker Desktop" : "Docker Engine", "permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock");
      break;
    case "nocompose":
      runtime("nocompose", "Docker Engine", "docker: 'compose' is not a docker command.");
      break;
    case "runtimeerror":
      runtime("error", "Docker Desktop", "Error response from daemon: Docker Desktop is unable to start");
      break;
    case "failed":
      named();
      f.status = { phase: "starting", alias: NAME, contacts: 0 };
      f.status_modified = now - 3600;
      launch = (w) => {
        w.launchedAt = now - 95;
        w.failure = ["worker Pulling", "worker Error Get \"https://ghcr.io/v2/\": dial tcp: lookup ghcr.io: no such host",
          "Error response from daemon: Get \"https://ghcr.io/v2/\": dial tcp: lookup ghcr.io: no such host"];
      };
      break;
    case "nowsl":
    case "novirt":
      m.runtimeState = "missing";
      m.runtimeName = "Docker";
      m.capability = state === "nowsl" ? { state: "wsl-off", reason: "", machine: "" } : { state: "unsupported", reason: "virtual", machine: "Amazon EC2 m5.large" };
      m.checks.containers.state = m.capability.state;
      break;
    case "direct":
      direct();
      break;
    case "directnobrowser":
    case "directnopython":
    case "directnovenv":
    case "directnoplaywright":
      direct();
      m.browserCheck = previewBrowserCheck(m.os, state.replace("direct", ""));
      break;
    case "found":
      // Python was just installed: the row moved on to Playwright a second ago and says what it found.
      direct();
      m.browserCheck = previewBrowserCheck(m.os, "noplaywright");
      m.advances.browser = { at: now - 1, line: advanceWords(m, "browser", { mark: "○", text: "", step: "nopython" }, { mark: "○", text: "", step: "noplaywright" }) };
      break;
    case "directinstalling":
      direct();
      m.browserCheck = previewBrowserCheck(m.os, "noplaywright");
      m.browserInstall = { id: "playwright", name: "Playwright", running: true, status: 0, lastLine: "" };
      break;
    case "directoff":
      direct();
      f.config!.browser = "none";
      break;
    case "directstarting":
      direct();
      named();
      launch = (w) => { w.launchedAt = now - 3; w.browserLaunch = "starting"; };
      break;
    case "directrunning":
    case "directbrowserclosed":
    case "directbrowserlater":
      direct();
      named();
      status("running");
      requests();
      if (state === "directrunning") f.browser = { host: "127.0.0.1", port: 41234, pid: 4243, app: m.os === "windows" ? "Edge" : "Chrome", kind: "host", python: "" };
      // directbrowserlater: started before Playwright was installed, which it is now.
      launch = state === "directbrowserlater"
        ? (w) => { w.browserLaunch = "Playwright isn't installed in myous's Python environment yet (Set up offers it); the worker runs without a browser"; }
        : (w) => { w.browserLaunch = "starting"; };
      break;
    case "noruntime":
      runtime("missing", "Docker");
      break;
    case "stoppedruntime":
      named();
      m.runtimeState = "stopped";
      break;
    case "starting":
      named();
      status("browser", { contacts: 0 });
      launch = (w) => { w.launchedAt = now - 47; w.launchStage = "creating"; };
      break;
    case "pair":
    case "pairlocal":
      named();
      status("running", { contacts: 0, paired: null, invite: { code: "4821-K7F3QX", link: "https://myoushq.com/p/4821#K7F3QX", expires_at: now + 702 } });
      if (state === "pairlocal") m.agents = fakeAgents(now);
      break;
    case "paired":
      named();
      status("running", { paired: { alias: "Max's Muse", verify: "4821", at: now - 5 } });
      f.config!.seen_paired_at = now - 10;
      break;
    case "macbrowser":
      // A Mac: a worker in a container with its browser on the Mac (myous for Mac's made-up
      // "macbrowser"); elsewhere the same as "running".
      named();
      status("running");
      requests();
      f.config!.browser = "mac";
      f.config!.browser_app = "com.google.Chrome";
      f.browser = { host: "host.docker.internal", port: 52345, pid: 4244, app: "Google Chrome", at: now };
      break;
    case "running":
    case "agents":
    case "agent":
      named();
      status("running");
      requests();
      if (state !== "running") m.agents = fakeAgents(now);
      break;
    case "busy":
      named();
      status("running");
      requests();
      f.requests.unshift({ id: "r5", op: "exec", alias: "Max's Muse", cmd: "python3 /work/scrape.py --slow", at: now - 20, decision: "allow" });
      break;
    case "approval":
      named();
      status("running");
      requests();
      f.requests.unshift({ id: "r5", op: "exec", alias: "Max's Muse", cmd: "rm /work/quotes.py", at: now - 8, decision: "pending" });
      f.approvals = [{ id: "r5", op: "exec", alias: "Max's Muse", cmd: "rm /work/quotes.py", asked_at: now - 8, wait: 120 }];
      break;
    case "paused":
      named();
      status("paused");
      requests();
      f.paused = true;
      break;
    case "stopping":
      named();
      status("running");
      launch = (w) => { w.stopping = true; w.stoppedAt = now - 3; };
      break;
    case "stopped":
      named();
      f.status = { phase: "running", alias: NAME, contacts: 1 };
      f.status_modified = now - 3600;
      launch = (w) => { w.stoppedByUs = true; };
      break;
    case "workers":
      named();
      status("running");
      requests();
      extraWorkers.push(files(`${home}-2`, { project: "myous-worker-2", config: { name: `${NAME} for Codex` },
        status: { phase: "running", alias: `${NAME} for Codex`, contacts: 0, invite: { code: "9012-ABCDEF", expires_at: now + 500 } }, status_modified: now }));
      break;
  }
  m.workers = [];
  m.load([f, ...extraWorkers]);
  launch(m.workers[0]);
  if (state === "agent" && m.agents) m.selectedAgent = m.agents[0];
}

export function fakeAgents(now: number): Agent[] {
  return [{
    home: "/home/max/.myous-claude", binary: "/home/max/.myous-claude/venv/bin/myous", error: "",
    status: { alias: "Claude Code", identity: "npub1q7abcdefghijklmnopqrstuvwxyz0x4f", client: "python", version: "0.6.1", unread: 2, last_used: now - 900,
      contact_list: [
        { alias: "Max's Muse", npub: "npub1muse", status: "approved", relationship: "other", paired_at: now - 86400 * 3, added_by: "owner", sharing: "we work on the same projects" },
        { alias: "Sam's agent", npub: "npub1sam", status: "approved", relationship: "friend", paired_at: now - 86400 * 9 },
      ],
      pending_pairings: [] },
  }];
}

/** The worker containers in the previews (Remove stale containers): the first worker's on an older
 * image, the second worker's on this app's `version`, a test's left running, and two stopped. With the
 * made-up "workers" (two workers) that is one of each kind; with one worker the second's is another's. */
export function previewContainers(version: string): Container[] {
  const c = (id: string, name: string, state: string, project: string, image: string, version = ""): Container =>
    ({ id, name, state, running: state === "running", image, project, version });
  const published = (v: string) => `ghcr.io/myoushq/worker:${v}`;
  return [
    c("ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb", "myous-worker-worker-1", "running", "myous-worker", published("0.6.0"), "0.6.0"),
    c("3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d", "myous-worker-2-worker-1", "running", "myous-worker-2", published(version), version),
    c("2e7d2c03a9507ae265ecf5b5356885a53393a2029d241394997265a1a25aefc6", "myous-desktop-e2e-1791700000-worker-1", "running", "myous-desktop-e2e-1791700000", "myous-desktop-e2e-1791700000-worker"),
    c("18ac3e7343f016890c510e93f935261169d9e3f565436429830faf0934f4f8e4", "myous-worker-3-worker-1", "exited", "myous-worker-3", published("0.5.2"), "0.5.2"),
    c("3f79bb7b435b05321651daefd374cdc681dc06faa65e374e38337b88ca046dea", "myous-worker", "created", "", published("0.4.3"), "0.4.3"),
  ];
}

/** What browser.rs finds: everything ("ready"), or one thing missing. */
/** A Mac's browsers in the previews: Google Chrome and Microsoft Edge (myous for Mac's made-up "setup"); none elsewhere. */
export function previewMacBrowsers(os: string): MacBrowsers {
  if (os !== "macos") return { installed: [], why: "not a Mac" };
  return { installed: [
    { id: "com.google.Chrome", name: "Google Chrome", path: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" },
    { id: "com.microsoft.edgemac", name: "Microsoft Edge", path: "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge" },
  ], why: "" };
}

export function previewBrowserCheck(os: string, kind: string): BrowserCheck {
  const win = os === "windows";
  if (os === "macos") return macBrowserCheck(kind);
  const c: BrowserCheck = {
    browser: win ? { id: "msedge", name: "Edge", path: "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe" }
      : { id: "chrome", name: "Chrome", path: "/usr/bin/google-chrome-stable" },
    python: null, python_old: "", playwright: "1.63.0",
    env_python: win ? "C:\\Users\\max\\AppData\\Local\\myous\\browser-python\\Scripts\\python.exe" : "/home/max/.local/share/myous/browser-python/bin/python",
    python_line: win ? "" : "sudo apt install python3 python3-venv", venv_line: win ? "" : "sudo apt install python3-venv",
    python_url: win ? "https://www.python.org/downloads/windows/" : "https://www.python.org/downloads/",
    get: win ? [{ name: "Edge", url: "https://www.microsoft.com/edge/download" }, { name: "Chrome", url: "https://www.google.com/chrome/" }]
      : [{ name: "Chrome", url: "https://www.google.com/chrome/" }, { name: "Edge", url: "https://www.microsoft.com/edge/download" }],
  };
  const py = { path: win ? "C:\\Python312\\python.exe" : "/usr/bin/python3", version: "3.12", venv: true };
  switch (kind) {
    case "nobrowser": return { ...c, browser: null };
    case "nopython": return { ...c, playwright: "" };
    case "oldpython": return { ...c, playwright: "", python_old: "3.9" };
    case "novenv": return { ...c, playwright: "", python: { ...py, version: "3.11", venv: false } };
    case "noplaywright": return { ...c, playwright: "", python: py };
  }
  return c;
}

/** browser.rs on a Mac: Chrome in /Applications, Homebrew's Python, the app's environment in Application Support. */
function macBrowserCheck(kind: string): BrowserCheck {
  const c: BrowserCheck = {
    browser: { id: "chrome", name: "Chrome", path: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" },
    python: null, python_old: "", playwright: "1.63.0",
    env_python: "/Users/max/Library/Application Support/myous/browser-python/bin/python",
    python_line: "", venv_line: "", python_url: "https://www.python.org/downloads/macos/",
    get: [{ name: "Chrome", url: "https://www.google.com/chrome/" }, { name: "Edge", url: "https://www.microsoft.com/edge/download" }],
  };
  const py = { path: "/opt/homebrew/bin/python3", version: "3.12", venv: true };
  switch (kind) {
    case "nobrowser": return { ...c, browser: null };
    case "nopython": return { ...c, playwright: "" };
    case "oldpython": return { ...c, playwright: "", python_old: "3.9" };
    case "novenv": return { ...c, playwright: "", python: { ...py, version: "3.11", venv: false } };
    case "noplaywright": return { ...c, playwright: "", python: py };
  }
  return c;
}

/** What install.rs offers on a computer with the tools (winget on Windows;
 * pkexec, systemd and apt on an Ubuntu): the previews show every button. */
export function previewOptions(os: string): RuntimeOptions {
  if (os === "macos") {
    return { tool: "Homebrew", start: "", start_line: "", group_lines: [], compose: "brew install docker-compose", compose_line: "brew install docker-compose",
      compose_url: "https://docs.docker.com/compose/install/", runtimes: [
        { id: "docker-desktop", name: "Docker Desktop", url: "https://www.docker.com/products/docker-desktop/", install: "brew install --cask docker-desktop" },
        { id: "orbstack", name: "OrbStack", url: "https://orbstack.dev/", install: "brew install --cask orbstack" },
      ] };
  }
  if (os === "windows") {
    return { tool: "winget", start: "", start_line: "", group_lines: ['net localgroup docker-users "max" /add'], compose: "", compose_line: "",
      compose_url: "https://docs.docker.com/compose/install/", runtimes: [
        { id: "docker-desktop", name: "Docker Desktop", url: "https://www.docker.com/products/docker-desktop/", install: "winget install --exact --id Docker.DockerDesktop" },
        { id: "rancher-desktop", name: "Rancher Desktop", url: "https://rancherdesktop.io/", install: "winget install --exact --id SUSE.RancherDesktop" },
      ] };
  }
  return { tool: "pkexec", runtimes: [
    { id: "docker-engine", name: "Docker Engine", url: "https://docs.docker.com/engine/install/", install: "apt-get update && apt-get install -y docker.io docker-compose-v2 && systemctl enable --now docker && usermod -aG docker max" },
    { id: "podman", name: "Podman", url: "https://podman.io/docs/installation", install: "apt-get update && apt-get install -y podman podman-compose" },
    { id: "docker-desktop", name: "Docker Desktop", url: "https://www.docker.com/products/docker-desktop/", install: "" },
  ], start: "systemctl enable --now docker", start_line: "sudo systemctl enable --now docker", group_lines: ["sudo usermod -aG docker $USER"],
  compose: "apt-get install -y docker-compose-v2", compose_line: "sudo apt-get install docker-compose-v2", compose_url: "https://docs.docker.com/compose/install/linux/" };
}

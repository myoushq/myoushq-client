// The tray menu, built from the model: what each worker can do now, the
// agents on this computer, settings and the Advanced submenu. Pure, so
// the tests check it; the window sends the items to the tray and keeps
// the actions to run when one is clicked.

import type { TrayItem } from "./backend.js";
import { Agent, agentContacts, agentName, Model, Worker } from "./model.js";
import { plural, str } from "./text.js";

export interface MenuActions {
  show(w?: Worker): void;
  showAgent(a: Agent): void;
  openBrowser(w: Worker): void;
  togglePaused(w: Worker): void;
  start(w: Worker): void;
  stop(w: Worker): void;
  removeWorker(w: Worker): void;
  addWorker(): void;
  settings(): void;
  checkUpdates(): void;
  useImage(w: Worker): void;
  chooseRepo(w: Worker): void;
  useDirect(w: Worker): void;
  useContainers(): void;
  showLog(): void;
  containerLog(w: Worker): void;
  openFolder(w: Worker): void;
  removeContainers(): void;
  addAgentHome(): void;
  quit(): void;
}

/** The menu; `sys` is the app's word for the system ("this PC" on Windows,
 * "this computer" on Linux). With one worker and no agents it lists the
 * worker alone; with a second worker or an agent, WORKERS and AGENTS ON
 * THIS PC sections, each a grey (disabled) label and a closing separator. */
export function buildMenu(m: Model, windowShown: boolean, act: MenuActions, sys = "this computer"): { items: TrayItem[]; actions: Map<string, () => void> } {
  const actions = new Map<string, () => void>();
  // An id names its entry, not its position: the window rebuilds the menu and the actions every two
  // seconds, so the menu the owner has open may be one build older than the actions, and a position
  // would then run another entry. A worker's entry (`w` given) names its worker too, by its folder:
  // `w:<folder>:<label>`; a label alone would be another worker's once this one's is gone (Stop of the
  // second worker, from a menu built before the first stopped). The others are `m:<label>` ("#2" for
  // a second with the same id), an agent's `a:<folder>`. A click whose id the new actions don't have
  // does nothing, and a worker's runs only while the window still has that worker.
  const seen = new Map<string, number>();
  const item = (label: string, run?: () => void, w?: Worker, fixedId = ""): TrayItem => {
    let id = fixedId || (w ? `w:${w.home}:${label}` : `${run ? "m" : "d"}:${label}`);
    const k = (seen.get(id) ?? 0) + 1;
    seen.set(id, k);
    if (k > 1) id += `#${k}`;
    if (run) actions.set(id, w ? onWorker(m, w, run) : run);
    return { id, label, enabled: !!run };
  };
  const sep: TrayItem = { separator: true };
  const several = m.workers.length > 1;
  const agents = m.agents ?? [];
  const items: TrayItem[] = [];
  const close = () => {
    if (items.length && !items[items.length - 1].separator) items.push(sep);
  };

  if (several || agents.length) items.push(item("WORKERS"));
  for (const w of m.workers) {
    let label = `${w.name} · ${w.stateWord}`;
    if (w.newRequests > 0 && !windowShown) label += ` · ${w.newRequests} new`;
    const pairedWith = str(w.paired?.alias);
    if (!several) {
      items.push(item(label, () => act.show(w), w, `w:${w.home}`));
      if (pairedWith) items.push(item(`   paired with ${pairedWith}`, undefined, w));
      continue;
    }
    const sub: TrayItem[] = [];
    if (pairedWith) sub.push(item(`Paired with ${pairedWith}`, undefined, w));
    sub.push(...workerActions(m, w, act, item));
    if (!w.isDefault) sub.push(sep, item("Remove…", () => act.removeWorker(w), w));
    items.push({ ...item(label, () => act.show(w), w, `w:${w.home}`), children: [item("Show…", () => act.show(w), w), ...sub] });
  }
  if (several) items.push(item("Add a worker…", act.addWorker));
  if (agents.length) {
    close();
    items.push(item(`AGENTS ON ${sys.toUpperCase()}`));
    for (const a of agents) {
      const what = a.error ? "not readable" : plural(agentContacts(a).length, "contact");
      items.push(item(`${agentName(a)} · ${what}`, () => act.showAgent(a), undefined, `a:${a.home}`));
    }
  }
  if (!several) {
    close();
    items.push(...workerActions(m, m.current, act, item));
  }
  close();
  items.push(item("Settings…", act.settings), item("Check for updates…", act.checkUpdates));
  const cur = m.current, c = cur.config, off = m.appConfig.containersOff;
  const mode = c.isDirect ? "no container" : c.isImage ? "published image" : "checkout";
  items.push({ label: "Advanced", enabled: true, children: [
    item(`Mode: ${mode}`),
    ...modeEntries(m, cur, act).map(([label, run, w]) => item(label, run, w)),
    sep,
    item("Show app log", act.showLog),
    item(c.isDirect ? "Show worker log" : "Show container log", () => act.containerLog(cur), cur),
    item("Open worker folder", () => act.openFolder(cur), cur),
    ...(off ? [] : [item("Remove stale containers…", act.removeContainers)]),
    sep,
    item("Add a worker…", act.addWorker),
    item("Add an agent's folder…", act.addAgentHome),
  ] });
  items.push(sep, item("Open myous…", () => act.show(), undefined, "open"), item("Quit myous", act.quit));
  return { items, actions };
}

/** A worker's entry runs while the window still has that worker: a menu can be clicked after it went. */
function onWorker(m: Model, w: Worker, run: () => void): () => void {
  return () => {
    if (m.workers.includes(w)) run();
  };
}

/** How the worker `w` can run: the published image, a checkout, or no
 * container; on a computer without containers, only the way back to them
 * (the whole app's, not the worker's). */
export function modeEntries(m: Model, w: Worker, act: MenuActions): Action[] {
  if (m.appConfig.containersOff) return [["Use containers (look for a runtime again)", act.useContainers]];
  const out: Action[] = [];
  if (!w.config.isImage) out.push(["Use the published image", () => act.useImage(w), w]);
  out.push(["Use a checkout…", () => act.chooseRepo(w), w]);
  if (!w.config.isDirect) out.push(["Run without a container…", () => act.useDirect(w), w]);
  return out;
}

function workerActions(m: Model, w: Worker, act: MenuActions, item: (l: string, r?: () => void, w?: Worker) => TrayItem): TrayItem[] {
  const sep: TrayItem = { separator: true };
  const its = (label: string, run?: () => void) => item(label, run, w);
  switch (w.screen) {
    case "running":
    case "pair":
    case "paired":
      return [
        ...browserItems(w, m.macBrowser(w)).map((label) => its(label, () => act.openBrowser(w))),
        its(w.paused ? "Resume" : "Pause", () => act.togglePaused(w)),
        its("Show requests…", () => act.show(w)),
        sep,
        its("Stop", () => act.stop(w)),
      ];
    case "stopping":
      return [its("Stopping…")];
    case "starting":
      return [its("Starting…"), its("Stop", () => act.stop(w))];
    case "stopped":
      return [its("Start", () => act.start(w))];
  }
  return [its("Set up…", () => act.show(w))];
}

/** The browser's item for a running worker: the container's browser
 * view; without a container, its browser on this computer while it has
 * one ("Show browser", or "Open browser" once the owner closed it). A
 * Mac's worker in a container with its browser on the Mac (`onMac`):
 * "Show browser", as myous for Mac says. */
export function browserItems(w: Worker, onMac = false): string[] {
  if (!w.config.isDirect) return [onMac ? "Show browser" : "Open browser"];
  if (!w.config.hostBrowser) return [];
  if (w.browserUp) return ["Show browser"];
  return w.browserLaunch === "starting" ? ["Open browser"] : [];
}

/** One entry of the window's own menu: its label, its action, and the worker it acts on (none: the whole app's). */
export type Action = [string, () => void, Worker?];
/** An entry, or a separator. */
export type Entry = Action | null;

function browserEntries(m: Model, act: MenuActions): Entry[] {
  const w = m.current;
  const running = w.screen === "running" || w.screen === "pair" || w.screen === "paired";
  return running ? browserItems(w, m.macBrowser(w)).map((label): Entry => [label, () => act.openBrowser(w), w]) : [];
}

function toolEntries(m: Model, act: MenuActions): Entry[] {
  const w = m.current;
  return [
    ["Show app log", act.showLog], [w.config.isDirect ? "Show worker log" : "Show container log", () => act.containerLog(w), w],
    ["Open worker folder", () => act.openFolder(w), w], ...(m.appConfig.containersOff ? [] : [["Remove stale containers…", act.removeContainers] as Entry]), null,
    ["Add a worker…", act.addWorker], ["Add an agent's folder…", act.addAgentHome], null,
    ...modeEntries(m, w, act),
  ];
}

/** The ⋯ menu in the window (Linux and Windows): the browser, settings, the logs and folders, the modes, Quit.
 * A worker's entry acts on the worker shown when it opened, while the window still has it. */
export function moreMenu(m: Model, act: MenuActions): Entry[] {
  const browser = browserEntries(m, act);
  const entries: Entry[] = [
    ...browser, ...(browser.length ? [null] : []),
    ["Settings…", act.settings], ["Check for updates…", act.checkUpdates], null,
    ...toolEntries(m, act), null,
    ["Quit myous", act.quit],
  ];
  return entries.map((e) => (e?.[2] ? [e[0], onWorker(m, e[2], e[1]), e[2]] : e));
}

/** A Mac has no ⋯: the same entries are the app menu's, Settings… first with ⌘, as on every Mac
 * app. Rust puts About above them and Hide and Quit below (and adds the Edit and Window menus). */
export function appMenu(m: Model, act: MenuActions): { items: TrayItem[]; actions: Map<string, () => void> } {
  const actions = new Map<string, () => void>();
  const browser = browserEntries(m, act);
  const entries: Entry[] = [
    ["Settings…", act.settings], ["Check for updates…", act.checkUpdates], null,
    ...browser, ...(browser.length ? [null] : []),
    ...toolEntries(m, act),
  ];
  // Named, as the tray's entries are (see buildMenu), not numbered: entries come and go (the browser's),
  // which would move every later number onto another entry. A worker's names its folder too.
  const items = entries.map((e): TrayItem => {
    if (!e) return { separator: true };
    const [label, run, w] = e;
    const id = w ? `app:w:${w.home}:${label}` : `app:${label}`;
    actions.set(id, w ? onWorker(m, w, run) : run);
    return { id, label, enabled: true, ...(label === "Settings…" ? { accelerator: "CmdOrCtrl+," } : {}) };
  });
  return { items, actions };
}

// The tray menu, built from the model: what each worker can do now, the
// agents on this computer, settings and the Advanced submenu. Pure, so
// the tests check it; the window sends the items to the tray and keeps
// the actions to run when one is clicked.

import type { TrayItem } from "./backend.js";
import { agentContacts, agentName, Model, Worker } from "./model.js";
import { plural, str } from "./text.js";

export interface MenuActions {
  show(w?: Worker): void;
  showAgent(index: number): void;
  openBrowser(w: Worker): void;
  togglePaused(w: Worker): void;
  start(w: Worker): void;
  stop(w: Worker): void;
  removeWorker(w: Worker): void;
  addWorker(): void;
  settings(): void;
  checkUpdates(): void;
  useImage(): void;
  chooseRepo(): void;
  useDirect(): void;
  useContainers(): void;
  showLog(): void;
  containerLog(): void;
  openFolder(): void;
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
  // An id names its entry (the label, "#2" for a second entry with the same label), not its
  // position: the window rebuilds the menu and the actions every two seconds, so the menu the owner
  // has open may be one build older than the actions, and a position would then run another entry.
  const seen = new Map<string, number>();
  const item = (label: string, run?: () => void, enabled = true, fixedId = ""): TrayItem => {
    let id = fixedId || `${run ? "m" : "d"}:${label}`;
    const k = (seen.get(id) ?? 0) + 1;
    seen.set(id, k);
    if (k > 1) id += `#${k}`;
    if (run) actions.set(id, run);
    return { id, label, enabled: enabled && !!run };
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
      items.push(item(label, () => act.show(w)));
      if (pairedWith) items.push(item(`   paired with ${pairedWith}`));
      continue;
    }
    const sub: TrayItem[] = [];
    if (pairedWith) sub.push(item(`Paired with ${pairedWith}`));
    sub.push(...workerActions(m, w, act, item));
    if (!w.isDefault) sub.push(sep, item("Remove…", () => act.removeWorker(w)));
    items.push({ ...item(label, () => act.show(w)), children: [item("Show…", () => act.show(w)), ...sub] });
  }
  if (several) items.push(item("Add a worker…", act.addWorker));
  if (agents.length) {
    close();
    items.push(item(`AGENTS ON ${sys.toUpperCase()}`));
    agents.forEach((a, i) => {
      const what = a.error ? "not readable" : plural(agentContacts(a).length, "contact");
      items.push(item(`${agentName(a)} · ${what}`, () => act.showAgent(i)));
    });
  }
  if (!several) {
    close();
    items.push(...workerActions(m, m.current, act, item));
  }
  close();
  items.push(item("Settings…", act.settings), item("Check for updates…", act.checkUpdates));
  const c = m.current.config, off = m.appConfig.containersOff;
  const mode = c.isDirect ? "no container" : c.isImage ? "published image" : "checkout";
  items.push({ label: "Advanced", enabled: true, children: [
    item(`Mode: ${mode}`),
    ...modeEntries(m.current, off, act).map(([label, run]) => item(label, run)),
    sep,
    item("Show app log", act.showLog),
    item(c.isDirect ? "Show worker log" : "Show container log", act.containerLog),
    item("Open worker folder", act.openFolder),
    ...(off ? [] : [item("Remove stale containers", act.removeContainers)]),
    sep,
    item("Add a worker…", act.addWorker),
    item("Add an agent's folder…", act.addAgentHome),
  ] });
  items.push(sep, item("Open myous…", () => act.show(), true, "open"), item("Quit myous", act.quit));
  return { items, actions };
}

/** How a worker can run: the published image, a checkout, or no container;
 * on a computer without containers, only the way back to them. */
export function modeEntries(w: Worker, containersOff: boolean, act: MenuActions): [string, () => void][] {
  if (containersOff) return [["Use containers (look for a runtime again)", act.useContainers]];
  const out: [string, () => void][] = [];
  if (!w.config.isImage) out.push(["Use the published image", act.useImage]);
  out.push(["Use a checkout…", act.chooseRepo]);
  if (!w.config.isDirect) out.push(["Run without a container…", act.useDirect]);
  return out;
}

function workerActions(m: Model, w: Worker, act: MenuActions, item: (l: string, r?: () => void) => TrayItem): TrayItem[] {
  const sep: TrayItem = { separator: true };
  switch (w.screen) {
    case "running":
    case "pair":
    case "paired":
      return [
        ...browserItems(w, m.macBrowser(w)).map((label) => item(label, () => act.openBrowser(w))),
        item(w.paused ? "Resume" : "Pause", () => act.togglePaused(w)),
        item("Show requests…", () => act.show(w)),
        sep,
        item("Stop", () => act.stop(w)),
      ];
    case "stopping":
      return [item("Stopping…")];
    case "starting":
      return [item("Starting…"), item("Stop", () => act.stop(w))];
    case "stopped":
      return [item("Start", () => act.start(w))];
  }
  return [item("Set up…", () => act.show(w))];
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

/** One entry of the window's own menu (label and action), or a separator. */
export type Entry = [string, () => void] | null;

function browserEntries(m: Model, act: MenuActions): Entry[] {
  const w = m.current;
  const running = w.screen === "running" || w.screen === "pair" || w.screen === "paired";
  return running ? browserItems(w, m.macBrowser(w)).map((label) => [label, () => act.openBrowser(w)] as Entry) : [];
}

function toolEntries(m: Model, act: MenuActions): Entry[] {
  return [
    ["Show app log", act.showLog], [m.current.config.isDirect ? "Show worker log" : "Show container log", act.containerLog],
    ["Open worker folder", act.openFolder], ...(m.appConfig.containersOff ? [] : [["Remove stale containers", act.removeContainers] as Entry]), null,
    ["Add a worker…", act.addWorker], ["Add an agent's folder…", act.addAgentHome], null,
    ...modeEntries(m.current, m.appConfig.containersOff, act),
  ];
}

/** The ⋯ menu in the window (Linux and Windows): the browser, settings, the logs and folders, the modes, Quit. */
export function moreMenu(m: Model, act: MenuActions): Entry[] {
  const browser = browserEntries(m, act);
  return [
    ...browser, ...(browser.length ? [null] : []),
    ["Settings…", act.settings], ["Check for updates…", act.checkUpdates], null,
    ...toolEntries(m, act), null,
    ["Quit myous", act.quit],
  ];
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
  const items = entries.map((e, i): TrayItem => {
    if (!e) return { separator: true };
    const id = `app${i}`;
    actions.set(id, e[1]);
    return { id, label: e[0], enabled: true, ...(e[0] === "Settings…" ? { accelerator: "CmdOrCtrl+," } : {}) };
  });
  return { items, actions };
}

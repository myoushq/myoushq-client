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
  let n = 0;
  const item = (label: string, run?: () => void, enabled = true, fixedId = ""): TrayItem => {
    const id = fixedId || (run ? `m${++n}` : `d${++n}`);
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
    sub.push(...workerActions(w, act, item));
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
    items.push(...workerActions(m.current, act, item));
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

function workerActions(w: Worker, act: MenuActions, item: (l: string, r?: () => void) => TrayItem): TrayItem[] {
  const sep: TrayItem = { separator: true };
  switch (w.screen) {
    case "running":
    case "pair":
    case "paired":
      return [
        ...browserItems(w).map((label) => item(label, () => act.openBrowser(w))),
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
 * one ("Show browser", or "Open browser" once the owner closed it). */
export function browserItems(w: Worker): string[] {
  if (!w.config.isDirect) return ["Open browser"];
  if (!w.config.hostBrowser) return [];
  if (w.browserUp) return ["Show browser"];
  return w.browserLaunch === "starting" ? ["Open browser"] : [];
}

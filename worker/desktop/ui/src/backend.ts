// Everything the window asks of the computer goes through here: the
// Tauri commands in src-tauri/src/lib.rs, or, in a plain browser (a
// preview of ui/dist, the screenshots, the tests), a stand-in that does
// nothing and reports a fake state from the page's address.

import { previewBrowserCheck, previewOptions } from "./fake.js";
import type { Agent, BrowserCheck, Capability, RuntimeOptions, WorkerFiles } from "./model.js";

export interface AppInfo {
  version: string;
  os: string;
  fake: string;
  system_name: string;
  product_name: string;
  download_path: string;
  hostname: string;
  hub: string;
  image_tag: string;
}

export interface TrayItem {
  id?: string;
  label?: string;
  enabled?: boolean;
  separator?: boolean;
  children?: TrayItem[];
}

type Handler = (payload: any) => void;

export interface Backend {
  readonly preview: boolean;
  appInfo(): Promise<AppInfo>;
  readWorkers(): Promise<WorkerFiles[]>;
  writeConfig(home: string, config: object): Promise<void>;
  sendCommand(home: string, name: string): Promise<void>;
  setPaused(home: string, on: boolean): Promise<void>;
  answerApproval(home: string, id: string, verdict: string): Promise<void>;
  setReview(home: string, level: string): Promise<void>;
  newWorker(): Promise<string>;
  removeWorker(home: string): Promise<void>;
  probeRuntime(): Promise<{ state: string; name: string; detail: string }>;
  launchRuntime(name: string): Promise<void>;
  runtimeOptions(runtime: string): Promise<RuntimeOptions>;
  runtimeInstall(id: string, runtime: string): Promise<void>;
  hubCheck(): Promise<{ state: string; detail: string }>;
  writeCompose(home: string): Promise<string>;
  composeRun(home: string, composeFile: string, project: string, args: string[], env: Record<string, string>, op: string): Promise<void>;
  browserPort(composeFile: string, project: string, legacy: boolean): Promise<string>;
  containerLogs(composeFile: string, project: string): Promise<string>;
  removeStaleContainers(): Promise<string>;
  readAgents(extra: string[]): Promise<Agent[]>;
  agentRun(home: string, binary: string, args: string[]): Promise<{ status: number; output: string }>;
  latestRelease(): Promise<string>;
  qrPng(text: string): Promise<string>;
  setTray(icon: string | null, tooltip: string, items: TrayItem[]): Promise<void>;
  /** A Mac: the count next to the menu bar icon and on the Dock icon (0 clears them). */
  setCounts(fresh: number): Promise<void>;
  /** A Mac: the Dock icon on or off, at once. */
  setDock(on: boolean): Promise<void>;
  showWindow(): Promise<void>;
  quit(): Promise<void>;
  logLine(line: string): Promise<void>;
  /** An urgent one also plays a sound and asks for attention on the window. */
  notify(title: string, body: string, urgent: boolean): Promise<void>;
  openUrl(url: string): Promise<void>;
  openPath(path: string): Promise<void>;
  pickFolder(): Promise<string | null>;
  copyText(text: string): Promise<void>;
  autostartGet(): Promise<boolean>;
  autostartSet(on: boolean): Promise<void>;
  fileExists(path: string): Promise<boolean>;
  containersCheck(): Promise<Capability>;
  enableWsl(): Promise<{ status: number; output: string }>;
  directStart(home: string, name: string, description: string, browser: boolean): Promise<{ pid: number; browser: string }>;
  directStop(home: string, pid: number): Promise<boolean>;
  workerLog(home: string): Promise<string>;
  browserCheck(): Promise<BrowserCheck>;
  browserInstall(home: string): Promise<void>;
  browserShow(home: string, name: string): Promise<string>;
  listen(event: string, handler: Handler): void;
}

declare global {
  interface Window {
    __TAURI__?: { core: { invoke: (cmd: string, args?: object) => Promise<any> }; event: { listen: (e: string, h: (ev: { payload: any }) => void) => Promise<unknown> } };
  }
}

class TauriBackend implements Backend {
  readonly preview = false;
  private invoke = (cmd: string, args?: object) => window.__TAURI__!.core.invoke(cmd, args);
  appInfo() { return this.invoke("app_info"); }
  readWorkers() { return this.invoke("read_workers"); }
  writeConfig(home: string, config: object) { return this.invoke("write_config", { home, config }); }
  sendCommand(home: string, name: string) { return this.invoke("send_command", { home, name }); }
  setPaused(home: string, on: boolean) { return this.invoke("set_paused", { home, on }); }
  answerApproval(home: string, id: string, verdict: string) { return this.invoke("answer_approval", { home, id, verdict }); }
  setReview(home: string, level: string) { return this.invoke("set_review", { home, level }); }
  newWorker() { return this.invoke("new_worker"); }
  removeWorker(home: string) { return this.invoke("remove_worker", { home }); }
  probeRuntime() { return this.invoke("probe_runtime"); }
  launchRuntime(name: string) { return this.invoke("launch_runtime", { name }); }
  runtimeOptions(runtime: string) { return this.invoke("runtime_options", { runtime }); }
  runtimeInstall(id: string, runtime: string) { return this.invoke("runtime_install", { id, runtime }); }
  hubCheck() { return this.invoke("hub_check"); }
  writeCompose(home: string) { return this.invoke("write_compose", { home }); }
  composeRun(home: string, composeFile: string, project: string, args: string[], env: Record<string, string>, op: string) {
    return this.invoke("compose_run", { home, composeFile, project, args, env, op });
  }
  browserPort(composeFile: string, project: string, legacy: boolean) { return this.invoke("browser_port", { composeFile, project, legacy }); }
  containerLogs(composeFile: string, project: string) { return this.invoke("container_logs", { composeFile, project }); }
  removeStaleContainers() { return this.invoke("remove_stale_containers"); }
  readAgents(extra: string[]) { return this.invoke("read_agents", { extra }); }
  agentRun(home: string, binary: string, args: string[]) { return this.invoke("agent_run", { home, binary, args }); }
  latestRelease() { return this.invoke("latest_release"); }
  qrPng(text: string) { return this.invoke("qr_png", { text }); }
  setTray(icon: string | null, tooltip: string, items: TrayItem[]) { return this.invoke("set_tray", { icon, tooltip, items }); }
  setCounts(fresh: number) { return this.invoke("set_counts", { fresh }); }
  setDock(on: boolean) { return this.invoke("set_dock", { on }); }
  showWindow() { return this.invoke("show_window"); }
  quit() { return this.invoke("quit"); }
  logLine(line: string) { return this.invoke("log_line", { line }); }
  notify(title: string, body: string, urgent: boolean) { return this.invoke("notify", { title, body, urgent }); }
  openUrl(url: string) { return this.invoke("open_url", { url }); }
  openPath(path: string) { return this.invoke("open_path", { path }); }
  pickFolder() { return this.invoke("pick_folder"); }
  copyText(text: string) { return this.invoke("copy_text", { text }); }
  autostartGet() { return this.invoke("autostart_get"); }
  autostartSet(on: boolean) { return this.invoke("autostart_set", { on }); }
  fileExists(path: string) { return this.invoke("file_exists", { path }); }
  containersCheck() { return this.invoke("containers_check"); }
  enableWsl() { return this.invoke("enable_wsl"); }
  directStart(home: string, name: string, description: string, browser: boolean) { return this.invoke("direct_start", { home, name, description, browser }); }
  directStop(home: string, pid: number) { return this.invoke("direct_stop", { home, pid }); }
  workerLog(home: string) { return this.invoke("worker_log", { home }); }
  browserCheck() { return this.invoke("browser_check"); }
  browserInstall(home: string) { return this.invoke("browser_install", { home }); }
  browserShow(home: string, name: string) { return this.invoke("browser_show", { home, name }); }
  listen(event: string, handler: Handler) { void window.__TAURI__!.event.listen(event, (ev) => handler(ev.payload)); }
}

/** A browser without Tauri: the screens with made-up data, no effects.
 * What the page did is kept in `calls`, for the browser tests. */
export class PreviewBackend implements Backend {
  readonly preview = true;
  calls: [string, unknown[]][] = [];
  tray: { icon: string | null; tooltip: string; items: TrayItem[] } | null = null;
  constructor(private info: Partial<AppInfo> = {}) {}
  private rec<T>(name: string, args: unknown[], value: T): Promise<T> {
    this.calls.push([name, args]);
    return Promise.resolve(value);
  }
  appInfo(): Promise<AppInfo> {
    const os = this.info.os ?? "linux";
    return Promise.resolve({ version: "0.6.1", fake: "running", hostname: "maxs-pc", hub: "https://myoushq.com", image_tag: "0.6.1",
      system_name: os === "windows" ? "this PC" : os === "macos" ? "this Mac" : "this computer",
      product_name: os === "windows" ? "myous for Windows" : os === "macos" ? "myous for Mac" : "myous for Linux",
      download_path: `/download/${os}`, ...this.info, os });
  }
  readWorkers() { return this.rec("readWorkers", [], [] as WorkerFiles[]); }
  writeConfig(...a: [string, object]) { return this.rec("writeConfig", a, undefined); }
  sendCommand(...a: [string, string]) { return this.rec("sendCommand", a, undefined); }
  setPaused(...a: [string, boolean]) { return this.rec("setPaused", a, undefined); }
  answerApproval(...a: [string, string, string]) { return this.rec("answerApproval", a, undefined); }
  setReview(...a: [string, string]) { return this.rec("setReview", a, undefined); }
  newWorker() { return this.rec("newWorker", [], "/home/max/.myous-worker-3"); }
  removeWorker(...a: [string]) { return this.rec("removeWorker", a, undefined); }
  /** What the runtime check answers (a test sets it). */
  probeResult = { state: "ok", name: "Docker Desktop", detail: "" };
  probeRuntime() { return this.rec("probeRuntime", [], this.probeResult); }
  launchRuntime(...a: [string]) { return this.rec("launchRuntime", a, undefined); }
  runtimeOptions(...a: [string]) { return this.rec("runtimeOptions", a, previewOptions(this.info.os ?? "linux")); }
  runtimeInstall(...a: [string, string]) { return this.rec("runtimeInstall", a, undefined); }
  hubCheck() { return this.rec("hubCheck", [], { state: "ok", detail: "" }); }
  writeCompose(...a: [string]) { return this.rec("writeCompose", a, `${a[0]}/compose.yml`); }
  composeRun(...a: [string, string, string, string[], Record<string, string>, string]) { return this.rec("composeRun", a, undefined); }
  browserPort(...a: [string, string, boolean]) { return this.rec("browserPort", a, "32768"); }
  containerLogs(...a: [string, string]) { return this.rec("containerLogs", a, "worker-1  | running"); }
  removeStaleContainers() { return this.rec("removeStaleContainers", [], "no worker containers found"); }
  readAgents(...a: [string[]]) { return this.rec("readAgents", a, [] as Agent[]); }
  agentRun(...a: [string, string, string[]]) { return this.rec("agentRun", a, { status: 0, output: "paired" }); }
  latestRelease() { return this.rec("latestRelease", [], "v0.6.1"); }
  qrPng(...a: [string]) {
    const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='240' height='240'><rect width='240' height='240' fill='white'/><rect x='20' y='20' width='60' height='60'/><rect x='160' y='20' width='60' height='60'/><rect x='20' y='160' width='60' height='60'/></svg>`;
    return this.rec("qrPng", a, `data:image/svg+xml;base64,${btoa(svg)}`);
  }
  setTray(icon: string | null, tooltip: string, items: TrayItem[]) {
    this.tray = { icon: icon ?? this.tray?.icon ?? null, tooltip, items };
    return this.rec("setTray", [icon ? "<png>" : null, tooltip], undefined);
  }
  setCounts(...a: [number]) { return this.rec("setCounts", a, undefined); }
  setDock(...a: [boolean]) { return this.rec("setDock", a, undefined); }
  showWindow() { return this.rec("showWindow", [], undefined); }
  quit() { return this.rec("quit", [], undefined); }
  logLine(...a: [string]) { return this.rec("logLine", a, undefined); }
  notify(...a: [string, string, boolean]) { return this.rec("notify", a, undefined); }
  openUrl(...a: [string]) { return this.rec("openUrl", a, undefined); }
  openPath(...a: [string]) { return this.rec("openPath", a, undefined); }
  pickFolder() { return this.rec("pickFolder", [], null); }
  copyText(...a: [string]) { return this.rec("copyText", a, undefined); }
  autostartGet() { return this.rec("autostartGet", [], false); }
  autostartSet(...a: [boolean]) { return this.rec("autostartSet", a, undefined); }
  fileExists(...a: [string]) { return this.rec("fileExists", a, true); }
  containersCheck() { return this.rec("containersCheck", [], { state: "ok", reason: "", machine: "" } as Capability); }
  enableWsl() { return this.rec("enableWsl", [], { status: 0, output: "" }); }
  directStart(...a: [string, string, string, boolean]) { return this.rec("directStart", a, { pid: 4242, browser: a[3] ? "starting" : "" }); }
  directStop(...a: [string, number]) { return this.rec("directStop", a, true); }
  workerLog(...a: [string]) { return this.rec("workerLog", a, "worker Max's PC (npub1example…), work directory work"); }
  /** What the browser check answers (a test sets it). */
  browserResult: BrowserCheck = previewBrowserCheck(this.info.os ?? "linux", "ready");
  browserCheck() { return this.rec("browserCheck", [], this.browserResult); }
  browserInstall(...a: [string]) { return this.rec("browserInstall", a, undefined); }
  browserShow(...a: [string, string]) { return this.rec("browserShow", a, "brought to the front"); }
  private handlers = new Map<string, Handler[]>();
  listen(event: string, handler: Handler) { this.handlers.set(event, [...(this.handlers.get(event) ?? []), handler]); }
  /** What Rust would send (an install's end, a log line), for the browser tests. */
  emit(event: string, payload: unknown) { for (const h of this.handlers.get(event) ?? []) h(payload); }
}

export function makeBackend(): Backend {
  if (window.__TAURI__) return new TauriBackend();
  const q = new URLSearchParams(location.search);
  return new PreviewBackend({ fake: q.get("state") ?? "running", os: q.get("os") ?? "linux" });
}

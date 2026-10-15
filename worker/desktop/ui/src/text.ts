// The words and numbers the screens use, the same as myous for Mac's.
// Times are Unix seconds, as the worker writes them.

export function timeAgo(unix: number, now: number): string {
  const d = Math.max(0, now - unix);
  if (d < 60) return `${Math.floor(d)} s ago`;
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
}

const pad = (n: number) => String(n).padStart(2, "0");

/** "21:12", local time. */
export function clockTime(unix: number): string {
  const t = new Date(unix * 1000);
  return `${pad(t.getHours())}:${pad(t.getMinutes())}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "Today", "Yesterday", "3 Oct". */
export function dayLabel(unix: number, now: number): string {
  const t = new Date(unix * 1000);
  const n = new Date(now * 1000);
  const same = (a: Date, b: Date) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
  if (same(t, n)) return "Today";
  const y = new Date(n);
  y.setDate(y.getDate() - 1);
  if (same(t, y)) return "Yesterday";
  return `${t.getDate()} ${MONTHS[t.getMonth()]}`;
}

/** "11:42" from seconds left (never negative). */
export function countdown(left: number): string {
  const s = Math.max(0, Math.floor(left));
  return `${Math.floor(s / 60)}:${pad(s % 60)}`;
}

/** "4.9 KB", like a file manager. */
export function byteCount(n: number): string {
  if (n < 1000) return `${Math.floor(n)} bytes`;
  if (n < 1e6) return `${(n / 1e3).toFixed(1)} KB`;
  if (n < 1e9) return `${(n / 1e6).toFixed(1)} MB`;
  return `${(n / 1e9).toFixed(1)} GB`;
}

/** "1 contact", "2 contacts". */
export function plural(n: number, word: string): string {
  return n === 1 ? `1 ${word}` : `${n} ${word}s`;
}

export function capitalize(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s;
}

/** The system the app runs on ("linux", "windows", "macos"), set once at
 * start; the words below depend on it. */
export let hostOs = "linux";
export function setHostOs(os: string): void { hostOs = os; }

const BROWSER_SENTENCE = `Its browser is where I log into sites for you: when I say "the myous browser" or "the worker browser", I mean that one, never your own.`;

/** The worker's card when the owner wrote none: the Mac app's sentence,
 * so agents hear one thing whichever app runs the worker. Without a
 * container on Windows it also says the commands are PowerShell: agents
 * are used to the container workers' Linux shell. Without a container,
 * the browser sentence only when the worker has its browser on this
 * computer (`browser`); a container worker always has one. */
export function defaultDescription(name: string, direct = false, browser = false): string {
  const who = name || "this worker";
  const b = browser ? ` ${BROWSER_SENTENCE}` : "";
  if (direct && hostOs === "windows") return `My own Windows PC, ${who}.${b} Commands you send are PowerShell, not Linux commands, and they run directly on it, as me, not in a container: take care, and ask me before anything you're unsure of.`;
  if (direct) return `My own computer, ${who}.${b} Commands you send run directly on it, as me, not in a container: take care, and ask me before anything you're unsure of.`;
  return `My own computer, ${who}. ${BROWSER_SENTENCE} Commands you send run in a container on it.`;
}

/** Whether a description is one of the defaults for this name and mode
 * (with or without the browser), so it follows the name and the browser
 * setting rather than being kept as the owner's own words. */
export function isDefaultDescription(text: string, name: string, direct: boolean): boolean {
  const t = text.trim();
  return t === defaultDescription(name, direct, false) || t === defaultDescription(name, direct, true);
}

/** What the owner pastes to the agent: the worker's own message when it
 * wrote one, else the same sentence built here. */
export function pairingMessage(invite: Record<string, unknown> | null, name: string): string {
  const msg = str(invite?.message);
  if (msg) return msg;
  return (
    `Pair with my worker "${name || "worker"}" (an environment I set up for you, not something to run yourself): accept the pairing code ` +
    `${str(invite?.code)} with relationship other and sharing "my own worker; run commands there for me". Then send it the message help, ` +
    `and read the Workers section of https://myoushq.com/skill.md before using it.`
  );
}

export function str(v: unknown): string {
  return typeof v === "string" ? v : "";
}

export function num(v: unknown): number {
  return typeof v === "number" && isFinite(v) ? v : 0;
}

export function dict(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

/** A notification's body for Linux, whose notification servers (GNOME's among them) read it as
 * markup: &, < and > as entities, so an agent's command shows as it is. */
export function escapeMarkup(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Whether `latest` ("v0.6.2") is newer than `mine`; a dev build never is. */
export function newer(latest: string, mine: string): boolean {
  if (!mine || mine === "dev" || !latest) return false;
  const parts = (s: string) => s.replace(/^v/, "").split(".").map((p) => parseInt(p.replace(/[^0-9]/g, ""), 10) || 0);
  const a = parts(latest), b = parts(mine);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const x = a[i] ?? 0, y = b[i] ?? 0;
    if (x !== y) return x > y;
  }
  return false;
}

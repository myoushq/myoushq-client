// Where an agent keeps its myous data.
//
// `Storage` is the interface; `FileStorage` keeps everything in one
// directory, for agents with a persistent disk. Agents without one
// (serverless runtimes, for example) implement `Storage` on what they have:
// a secrets store for the key, a database or object store for the rest.
//
// Must be durable: the key (losing it loses the identity) and "contacts".
// Should be durable: "state", history, "settings".
// Can be lost: "hub" (refetched) and "pending/*" (expires in 15 minutes).

import { promises as fs, constants as fsc } from "node:fs";
import { homedir } from "node:os";
import { join, dirname } from "node:path";

export interface Storage {
  /** The private key (nsec), or null. */
  loadKey(): Promise<string | null>;
  /** Store the private key. Must refuse to overwrite an existing one. */
  saveKey(nsec: string): Promise<void>;
  /** A small JSON document by name ("contacts", "state", "pending/4821", ...). */
  get<T>(name: string, fallback: T): Promise<T>;
  /** Replace a document. Should be atomic. */
  put(name: string, value: unknown): Promise<void>;
  /** Remove a document; no error if missing. */
  delete(name: string): Promise<void>;
  /** Names of documents starting with prefix (e.g. "pending/"). */
  names(prefix: string): Promise<string[]>;
  appendHistory(entry: HistoryEntry): Promise<void>;
  readHistory(): Promise<HistoryEntry[]>;
  /**
   * Run fn while holding a lock, for agents whose runs can overlap.
   * Returns undefined without running fn if wait is false and the lock is
   * taken. Optional: without it, operations aren't serialized.
   */
  withLock?<T>(name: string, fn: () => Promise<T>, wait?: boolean): Promise<T | undefined>;
}

export interface HistoryEntry {
  seq: number;
  type: "message" | "file" | "result" | "ack" | "paired" | "pairing_failed" | "update" | "notice";
  direction?: "in" | "out";
  peer?: string;
  alias?: string;
  text: string;
  at: number;
  sent_at?: number;
  /** "update" entries: the release announced. */
  version?: string;
  /** The contact's current relationship context, filled in when read. */
  relationship?: string | null;
  sharing?: string | null;
  /** A long message whose missing parts never arrived. */
  incomplete?: boolean;
  /** "notice" entries: the notice's id and an optional link; worker replies: the request's id. */
  id?: string;
  url?: string;
  /** "file" entries (protocol section 6): what the message says about the blob. The
   * key and nonce stay here, in the private history, like the messages. */
  name?: string;
  mime?: string;
  size?: number;
  x?: string;
  ox?: string;
  key?: string;
  nonce?: string;
  /** Worker semantics on a file (["put", id, path] or ["file", id]). */
  w?: string[];
  /** "result" and "ack" entries (protocol section 7): the worker's reply, parsed. */
  exit?: number;
  stdout?: string;
  stderr?: string;
  truncated?: boolean;
  ok?: boolean;
  path?: string;
  sha256?: string;
  error?: string;
}

/** Runs fn under storage's lock if it has one. */
export async function locked<T>(st: Storage, name: string, fn: () => Promise<T>, wait = true): Promise<T | undefined> {
  return st.withLock ? st.withLock(name, fn, wait) : fn();
}

/**
 * Everything in one directory (default ~/.myous, or $MYOUS_HOME), in the
 * same layout as the Python client: key, contacts.json, state.json,
 * settings.json, hub.json, pending/*.json, messages.jsonl.
 */
export class FileStorage implements Storage {
  readonly home: string;
  private locks = new Map<string, Promise<unknown>>();

  constructor(home?: string) {
    this.home = home ?? process.env.MYOUS_HOME ?? join(homedir(), ".myous");
  }

  path(name: string): string {
    return join(this.home, name);
  }

  async loadKey(): Promise<string | null> {
    try {
      return (await fs.readFile(this.path("key"), "utf8")).trim();
    } catch (e) {
      if (isMissing(e)) return null;
      throw e;
    }
  }

  async saveKey(nsec: string): Promise<void> {
    await fs.mkdir(this.home, { recursive: true, mode: 0o700 });
    // "wx": never replace an existing key, even in a race.
    const f = await fs.open(this.path("key"), "wx", 0o600);
    try {
      await f.writeFile(nsec + "\n");
      await f.sync();
    } finally {
      await f.close();
    }
  }

  async get<T>(name: string, fallback: T): Promise<T> {
    try {
      return JSON.parse(await fs.readFile(this.path(name + ".json"), "utf8")) as T;
    } catch (e) {
      if (isMissing(e)) return fallback;
      throw e;
    }
  }

  async put(name: string, value: unknown): Promise<void> {
    await writePrivate(this.path(name + ".json"), JSON.stringify(value, null, 2) + "\n");
  }

  async delete(name: string): Promise<void> {
    await fs.rm(this.path(name + ".json"), { force: true });
  }

  async names(prefix: string): Promise<string[]> {
    const slash = prefix.lastIndexOf("/");
    const dir = slash >= 0 ? prefix.slice(0, slash) : "";
    const stem = prefix.slice(slash + 1);
    let files: string[];
    try {
      files = await fs.readdir(dir ? this.path(dir) : this.home);
    } catch (e) {
      if (isMissing(e)) return [];
      throw e;
    }
    return files
      .filter((f) => f.endsWith(".json") && f.startsWith(stem))
      .map((f) => (dir ? `${dir}/` : "") + f.slice(0, -5))
      .sort();
  }

  async appendHistory(entry: HistoryEntry): Promise<void> {
    await fs.mkdir(this.home, { recursive: true, mode: 0o700 });
    const f = await fs.open(this.path("messages.jsonl"), "a", 0o600);
    try {
      await f.writeFile(JSON.stringify(entry) + "\n");
      await f.sync();
    } finally {
      await f.close();
    }
  }

  async readHistory(): Promise<HistoryEntry[]> {
    let text: string;
    try {
      text = await fs.readFile(this.path("messages.jsonl"), "utf8");
    } catch (e) {
      if (isMissing(e)) return [];
      throw e;
    }
    return text.split("\n").filter((l) => l.trim()).map((l) => JSON.parse(l) as HistoryEntry);
  }

  /**
   * A lock file under locks/, so separate processes (a listener and a
   * one-off poll, say) don't step on each other. Created with O_EXCL and
   * treated as stale after 2 minutes.
   */
  async withLock<T>(name: string, fn: () => Promise<T>, wait = true): Promise<T | undefined> {
    // Serialize within this process first: the lock file isn't reentrant.
    const previous = this.locks.get(name) ?? Promise.resolve();
    let release!: () => void;
    const mine = new Promise<void>((r) => (release = r));
    this.locks.set(name, previous.then(() => mine));
    await previous;
    try {
      const file = join(this.home, "locks", name.replace(/\//g, "_"));
      await fs.mkdir(dirname(file), { recursive: true, mode: 0o700 });
      const deadline = Date.now() + 60_000;
      for (;;) {
        try {
          const f = await fs.open(file, fsc.O_CREAT | fsc.O_EXCL | fsc.O_WRONLY, 0o600);
          await f.close();
          break;
        } catch (e) {
          if ((e as NodeJS.ErrnoException).code !== "EEXIST") throw e;
          const stat = await fs.stat(file).catch(() => null);
          if (stat && Date.now() - stat.mtimeMs > 120_000) {
            await fs.rm(file, { force: true });
            continue;
          }
          if (!wait || Date.now() > deadline) return undefined;
          await new Promise((r) => setTimeout(r, 50));
        }
      }
      try {
        return await fn();
      } finally {
        await fs.rm(file, { force: true });
      }
    } finally {
      release();
    }
  }
}

async function writePrivate(path: string, text: string): Promise<void> {
  await fs.mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const tmp = path + ".tmp";
  const f = await fs.open(tmp, "w", 0o600);
  try {
    await f.writeFile(text);
    await f.sync();
  } finally {
    await f.close();
  }
  await fs.rename(tmp, path);
}

function isMissing(e: unknown): boolean {
  return (e as NodeJS.ErrnoException)?.code === "ENOENT";
}

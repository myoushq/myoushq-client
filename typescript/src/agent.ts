// The library API: everything an agent does with myous, without assuming
// how it runs (VM, container, serverless) or when it wakes up.
//
//   const agent = await Agent.open(new FileStorage());   // or your own Storage
//   await agent.createIdentity();                         // once, ever
//   await agent.register("Sam's Muse");
//   const invite = await agent.invite();                  // share invite.link / invite.code
//   await agent.poll();                                   // pairings + new messages
//   await agent.send("Alex's Muse", "hi");
//   await agent.unread();

import { readFileSync } from "node:fs";
import { generateSecretKey, getPublicKey, nip19, type Event } from "nostr-tools";
import * as contacts from "./contacts.js";
import type { Contact, Contacts } from "./contacts.js";
import { record, type State } from "./history.js";
import { Hub, type HubConfig, type Notice } from "./hub.js";
import { Pairing, type Invite, type Pending } from "./pairing.js";
import * as parts from "./parts.js";
import { Connection, unwrap, type Unwrapped } from "./relay.js";
import { locked, type HistoryEntry, type Storage } from "./storage.js";

/** This client's release, from package.json. */
export const VERSION: string = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;

const SEEN_RETENTION = 3 * 86400; // longer than relay retention plus timestamp jitter

export class IdentityError extends Error {}

const LOST =
  "the private key is missing but contacts exist, so the identity was lost. " +
  "Do NOT create a new key: tell your owner. Restore the key from wherever it " +
  "was kept, or, if it is truly gone, the owner must clear the contacts and " +
  "pair with everyone again.";

export class Agent {
  private secretKey?: Uint8Array;

  private constructor(readonly st: Storage, readonly hub: Hub) {}

  static async open(st: Storage, hubUrl?: string): Promise<Agent> {
    if (hubUrl) {
      const settings = await st.get<Record<string, unknown>>("settings", {});
      await st.put("settings", { ...settings, hub: hubUrl.replace(/\/+$/, "") });
    }
    return new Agent(st, await Hub.open(st));
  }

  // --- identity ------------------------------------------------------------

  async hasIdentity(): Promise<boolean> {
    return (await this.st.loadKey()) !== null;
  }

  /** Generate the agent's key. Refuses if one exists or a previous key looks lost. */
  async createIdentity(): Promise<string> {
    if ((await this.st.loadKey()) !== null) {
      throw new IdentityError("a key already exists; refusing to replace the agent's identity");
    }
    if (Object.keys(await contacts.load(this.st)).length) throw new IdentityError(LOST);
    const sk = generateSecretKey();
    await this.st.saveKey(nip19.nsecEncode(sk));
    this.secretKey = sk;
    return getPublicKey(sk);
  }

  async key(): Promise<Uint8Array> {
    if (!this.secretKey) {
      const nsec = await this.st.loadKey();
      if (nsec === null) {
        throw new IdentityError(Object.keys(await contacts.load(this.st)).length ? LOST : "no identity yet; create one first");
      }
      this.secretKey = nip19.decode(nsec).data as Uint8Array;
    }
    return this.secretKey;
  }

  async pubkey(): Promise<string> {
    return getPublicKey(await this.key());
  }

  async npub(): Promise<string> {
    return nip19.npubEncode(await this.pubkey());
  }

  async alias(): Promise<string> {
    return (await this.st.get<Record<string, string>>("settings", {})).alias ?? "agent";
  }

  /** Publish profile and inbox relays; the first time, with proof of work, this registers. */
  async register(alias?: string): Promise<void> {
    if (alias) {
      const settings = await this.st.get<Record<string, unknown>>("settings", {});
      await this.st.put("settings", { ...settings, alias });
    }
    const cfg = await this.hub.config(true);
    const registered = (await this.st.get<State>("state", {})).registered;
    const conn = await this.connect(cfg);
    try {
      try {
        await conn.register(await this.alias(), registered ? 0 : cfg.pow_difficulty);
      } catch (e) {
        if (!String((e as Error).message).includes("pow:")) throw e;
        // The hub forgot us (e.g. rebuilt): register again with proof of work.
        await conn.register(await this.alias(), cfg.pow_difficulty);
      }
      await conn.publishInboxRelays(cfg.relays);
    } finally {
      conn.close();
    }
    await locked(this.st, "state", async () => {
      const state = await this.st.get<State>("state", {});
      await this.st.put("state", { ...state, registered: true });
    });
  }

  async isRegistered(): Promise<boolean> {
    return Boolean((await this.st.get<State>("state", {})).registered);
  }

  // --- pairing -------------------------------------------------------------

  async pairing(): Promise<Pairing> {
    return new Pairing(this.st, this.hub, await this.pubkey(), await this.alias());
  }

  /** Start a pairing; it finishes during a later poll(), listen() or advancePairings(). */
  /** Start a pairing; the optional relationship context is recorded on the new contact. */
  async invite(context?: contacts.ContactContext): Promise<Invite> {
    contacts.contextFields(context); // validate
    return (await this.pairing()).invite(context);
  }

  /** Join a pairing; returns with stage "done", "failed", or still pending. */
  async accept(codeOrLink: string, wait = 60, context?: contacts.ContactContext): Promise<Pending> {
    contacts.contextFields(context); // validate
    return (await this.pairing()).accept(codeOrLink, wait, context);
  }

  /** Record how the owner knows a contact and what may be shared with it. */
  async setContext(name: string, context: contacts.ContactContext): Promise<Contact> {
    const fields = contacts.contextFields(context);
    return (await locked(this.st, "state", async () =>
      Object.keys(fields).length ? contacts.update(this.st, name, fields) : (await contacts.find(this.st, name))[1]))!;
  }

  async advancePairings(): Promise<Pending[]> {
    return (await this.pairing()).advanceAll();
  }

  // --- messages ------------------------------------------------------------

  async send(name: string, text: string): Promise<HistoryEntry> {
    const [pubkey, contact] = await contacts.find(this.st, name);
    if (contact.status !== "approved") throw new Error(`${contact.alias} is ${contact.status}`);
    const chunks = parts.split(text); // throws if it's too long to send
    const conn = await this.connect();
    try {
      if (chunks.length === 1) {
        await conn.sendMessage(pubkey, text);
      } else {
        const targets = await conn.deliveryTargets(pubkey);
        const id = parts.newId();
        for (const [i, chunk] of chunks.entries()) {
          try {
            await conn.sendMessage(pubkey, chunk, [["part", id, String(i + 1), String(chunks.length)]], targets);
          } catch (e) {
            throw new Error(`sent ${i} of ${chunks.length} parts, then: ${(e as Error).message ?? e}`);
          }
        }
      }
    } finally {
      conn.close();
    }
    return (await locked(this.st, "state", () =>
      record(this.st, { type: "message", direction: "out", peer: contact.npub, alias: contact.alias, text })))!;
  }

  /** Pass on what the hub announces, once each: a newer client release (an
   * "update" entry) and notices (a "notice" entry each). Both are
   * information only; what to do about them is up to the agent. */
  async checkNotices(): Promise<void> {
    let cfg: HubConfig;
    try {
      cfg = await this.hub.config();
    } catch {
      return;
    }
    const latest = cfg.latest_release && newer(cfg.latest_release, VERSION) ? cfg.latest_release : undefined;
    const notices = (Array.isArray(cfg.notices) ? cfg.notices : []).filter(noticeApplies);
    if (!latest && !notices.length) return;
    await locked(this.st, "state", async () => {
      const state = await this.st.get<State>("state", {});
      const seen = state.seen_notices ?? [];
      const entries: Omit<HistoryEntry, "seq" | "at">[] = [];
      const url = this.hub.url;
      if (latest && state.announced_release !== latest) {
        state.announced_release = latest;
        entries.push({
          type: "update", version: latest,
          text: `myous ${latest} is available (this client is v${VERSION}). Consider upgrading: get the release, ` +
            `verify its signature and build it as in ${url}/skill.md. What changed: ${url}/changelog.md`,
        });
      }
      const hub = new URL(url);
      for (const n of notices) {
        if (seen.includes(n.id)) continue;
        seen.push(n.id);
        const text = [...n.text].filter((c) => !/\p{Cc}/u.test(c)).slice(0, 500).join("");
        const entry: Omit<HistoryEntry, "seq" | "at"> = { type: "notice", id: n.id, text: `Notice from ${hub.host}: ${text}` };
        try {
          const link = typeof n.url === "string" ? new URL(n.url) : undefined;
          if (link && link.protocol === hub.protocol && link.host === hub.host) {
            entry.url = n.url;
            entry.text += ` (more: ${n.url})`;
          }
        } catch {}
        entries.push(entry);
      }
      if (!entries.length) return;
      state.seen_notices = seen.slice(-200);
      await this.st.put("state", state);
      for (const e of entries) await record(this.st, e);
    });
  }

  /** Advance pairings and fetch waiting messages, once. Returns new history entries. */
  async poll(): Promise<HistoryEntry[]> {
    const before = await this.nextSeq();
    await this.advancePairings();
    await this.checkNotices();
    const conn = await this.connect();
    let wraps: Event[];
    try {
      wraps = await conn.fetchWraps();
    } finally {
      conn.close();
    }
    await locked(this.st, "state", async () => {
      await this.handleWraps(wraps);
      const state = await this.st.get<State>("state", {});
      await this.st.put("state", { ...state, last_poll: Math.floor(Date.now() / 1000) });
    });
    return this.entriesSince(before);
  }

  /**
   * Stay connected and handle messages as they arrive, until the connection
   * closes (then resolve; call again to reconnect). Calls onNew for new
   * messages and pairing results, and onTick every `tick` seconds (every 3
   * while a pairing is pending).
   */
  async listen(onNew?: (entries: HistoryEntry[]) => unknown, onTick?: () => unknown, tick = 20): Promise<string> {
    const conn = await this.connect();
    let stopped = false;
    const notify = async (entries: HistoryEntry[]) => {
      if (entries.length && onNew) await onNew(entries);
    };
    const housekeeping = (async () => {
      while (!stopped) {
        const before = await this.nextSeq();
        await this.advancePairings();
        await this.checkNotices();
        await notify(await this.entriesSince(before));
        await onTick?.();
        const pending = (await (await this.pairing()).pending()).length;
        await new Promise((r) => setTimeout(r, (pending ? 3 : tick) * 1000));
      }
    })();
    let queue = Promise.resolve();
    try {
      return await conn.streamWraps((wrap) => {
        queue = queue.then(async () => {
          const stored = await locked(this.st, "state", () => this.handleWraps([wrap]));
          await notify(stored ?? []);
        });
      });
    } finally {
      stopped = true;
      conn.close();
      await queue;
      void housekeeping;
    }
  }

  async unread(markRead = true): Promise<HistoryEntry[]> {
    return (await locked(this.st, "state", async () => {
      const state = await this.st.get<State>("state", {});
      const last = state.read_seq ?? 0;
      const entries = (await this.st.readHistory()).filter((e) => e.seq > last && e.direction !== "out");
      // The contact's current relationship context, so the agent has it when it answers.
      for (const e of entries) if (e.peer) Object.assign(e, await contacts.contextOf(this.st, e.peer));
      if (markRead && entries.length) await this.st.put("state", { ...state, read_seq: entries[entries.length - 1].seq });
      return entries;
    }))!;
  }

  async history(contact?: string, limit = 50): Promise<HistoryEntry[]> {
    let entries = await this.st.readHistory();
    if (contact) {
      const [, c] = await contacts.find(this.st, contact);
      entries = entries.filter((e) => e.peer === c.npub);
    }
    return entries.slice(-limit);
  }

  // --- contacts ------------------------------------------------------------

  contacts(): Promise<Contacts> {
    return contacts.load(this.st);
  }

  async block(name: string): Promise<Contact> {
    return (await locked(this.st, "state", () => contacts.update(this.st, name, { status: "blocked" })))!;
  }

  async unblock(name: string): Promise<Contact> {
    return (await locked(this.st, "state", () => contacts.update(this.st, name, { status: "approved" })))!;
  }

  async rename(name: string, alias: string): Promise<Contact> {
    return (await locked(this.st, "state", () => contacts.update(this.st, name, { alias })))!;
  }

  // --- internals -----------------------------------------------------------

  /** Store messages from approved contacts, drop everything else. Caller holds the lock. */
  private async handleWraps(wraps: Event[]): Promise<HistoryEntry[]> {
    const state = await this.st.get<State>("state", {});
    const seen = state.seen ?? {};
    const now = Math.floor(Date.now() / 1000);
    const fresh = wraps.filter((w) => {
      if (seen[w.id]) return false;
      seen[w.id] = now;
      return true;
    });
    state.seen = Object.fromEntries(Object.entries(seen).filter(([, t]) => now - t < SEEN_RETENTION));
    await this.st.put("state", state);

    // Wrap timestamps are randomized, so order by when messages were written.
    const sk = await this.key();
    const messages = fresh
      .map((w) => unwrap(sk, w))
      .filter((m): m is Unwrapped => m !== null)
      .sort((a, b) => a[2] - b[2] || a[3] - b[3] || (a[4]?.index ?? 0) - (b[4]?.index ?? 0));
    const buf = await this.st.get<parts.Buffer>("partials", {});
    let changed = false;
    const stored: HistoryEntry[] = [];
    for (const [sender, rawText, rawSentAt, ms, part] of messages) {
      const contact = await contacts.approved(this.st, sender);
      if (!contact) continue; // not paired, or blocked: drop silently
      let text = rawText, sentAt = rawSentAt;
      if (part) {
        changed = true;
        const done = parts.add(buf, sender, part, rawText, rawSentAt, ms, now);
        if (!done) continue; // waiting for the other parts
        [text, sentAt] = done;
      }
      stored.push(await record(this.st, {
        type: "message", direction: "in", peer: contact.npub, alias: contact.alias, text, sent_at: sentAt,
      }));
    }
    for (const [sender, text, sentAt] of parts.expire(buf, now)) {
      changed = true;
      const contact = await contacts.approved(this.st, sender);
      if (contact) {
        stored.push(await record(this.st, {
          type: "message", direction: "in", peer: contact.npub, alias: contact.alias, text, sent_at: sentAt, incomplete: true,
        }));
      }
    }
    if (changed) await this.st.put("partials", buf);
    return stored;
  }

  private async connect(cfg?: HubConfig): Promise<Connection> {
    return Connection.open(await this.key(), (cfg ?? (await this.hub.config())).relays);
  }

  private async nextSeq(): Promise<number> {
    return (await this.st.get<State>("state", {})).next_seq ?? 1;
  }

  private async entriesSince(seq: number): Promise<HistoryEntry[]> {
    if ((await this.nextSeq()) === seq) return [];
    return (await this.st.readHistory()).filter((e) => e.seq >= seq && e.direction !== "out");
  }
}

/** Whether release tag a ("v1.2.3") is newer than version b ("1.2.0"). */
function newer(a: string, b: string): boolean {
  const parse = (v: string) => /^v?(\d+)\.(\d+)\.(\d+)$/.exec(v.trim())?.slice(1).map(Number);
  const x = parse(a), y = parse(b);
  if (!x || !y) return false;
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i];
  return false;
}

/** Whether a notice from the hub is well-formed, current, and meant for this
 * client's version. */
function noticeApplies(n: any): n is Notice {
  if (!n || typeof n.id !== "string" || !n.id || typeof n.text !== "string" || !n.text.trim()) return false;
  if (typeof n.expires === "number" && n.expires <= Date.now() / 1000) return false;
  if (typeof n.min_version === "string" && newer(n.min_version, VERSION)) return false;
  if (typeof n.max_version === "string" && newer(VERSION, n.max_version)) return false;
  return true;
}

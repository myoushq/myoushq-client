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

import { generateSecretKey, getPublicKey, nip19, type Event } from "nostr-tools";
import * as contacts from "./contacts.js";
import type { Contact, Contacts } from "./contacts.js";
import { record, type State } from "./history.js";
import { Hub, type HubConfig } from "./hub.js";
import { Pairing, type Invite, type Pending } from "./pairing.js";
import { Connection, unwrap } from "./relay.js";
import { locked, type HistoryEntry, type Storage } from "./storage.js";

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
  async invite(): Promise<Invite> {
    return (await this.pairing()).invite();
  }

  /** Join a pairing; returns with stage "done", "failed", or still pending. */
  async accept(codeOrLink: string, wait = 60): Promise<Pending> {
    return (await this.pairing()).accept(codeOrLink, wait);
  }

  async advancePairings(): Promise<Pending[]> {
    return (await this.pairing()).advanceAll();
  }

  // --- messages ------------------------------------------------------------

  async send(name: string, text: string): Promise<HistoryEntry> {
    const [pubkey, contact] = await contacts.find(this.st, name);
    if (contact.status !== "approved") throw new Error(`${contact.alias} is ${contact.status}`);
    const conn = await this.connect();
    try {
      await conn.sendMessage(pubkey, text);
    } finally {
      conn.close();
    }
    return (await locked(this.st, "state", () =>
      record(this.st, { type: "message", direction: "out", peer: contact.npub, alias: contact.alias, text })))!;
  }

  /** Advance pairings and fetch waiting messages, once. Returns new history entries. */
  async poll(): Promise<HistoryEntry[]> {
    const before = await this.nextSeq();
    await this.advancePairings();
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
      .filter((m): m is [string, string, number, number] => m !== null)
      .sort((a, b) => a[2] - b[2] || a[3] - b[3]);
    const stored: HistoryEntry[] = [];
    for (const [sender, text, sentAt] of messages) {
      const contact = await contacts.approved(this.st, sender);
      if (!contact) continue; // not paired, or blocked: drop silently
      stored.push(await record(this.st, {
        type: "message", direction: "in", peer: contact.npub, alias: contact.alias, text, sent_at: sentAt,
      }));
    }
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

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
import { promises as fs } from "node:fs";
import { basename, join } from "node:path";
import { generateSecretKey, getPublicKey, nip19, type Event } from "nostr-tools";
import * as contacts from "./contacts.js";
import type { Contact, Contacts } from "./contacts.js";
import { Blobs, decryptFile, encryptFile, fileTags, parseFileTags, sanitizeName, type FileInfo } from "./files.js";
import { record, type State } from "./history.js";
import { Hub, HubError, type HubConfig, type Notice } from "./hub.js";
import { Pairing, type Invite, type Pending } from "./pairing.js";
import * as parts from "./parts.js";
import { Connection, KIND_FILE, unwrap, type Unwrapped } from "./relay.js";
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

  // --- cards (protocol section 4) -------------------------------------------

  /** This agent's self-description, in its owner's words ("" if unset). */
  async card(): Promise<string> {
    return (await this.st.get<Record<string, string>>("settings", {})).card ?? "";
  }

  /** Set (or clear, with "") what this agent says about itself. It reaches
   * contacts at the next syncCards() (poll does one). */
  async setCard(about: string | null | undefined): Promise<string> {
    about = (about ?? "").trim();
    if ([...about].length > contacts.MAX_ABOUT) throw new Error(`a card's description is limited to ${contacts.MAX_ABOUT} characters`);
    const settings = await this.st.get<Record<string, unknown>>("settings", {});
    await this.st.put("settings", { ...settings, card: about });
    return about;
  }

  /**
   * Approved contacts that haven't been told this agent's current alias and
   * card: new contacts (when there is a card to send), and every contact
   * after a rename or a card change.
   */
  async cardsDue(): Promise<Contact[]> {
    const name = await this.alias(), about = await this.card();
    const due: Contact[] = [];
    for (const c of Object.values(await contacts.load(this.st))) {
      if (c.status !== "approved") continue;
      const knows = c.peer_knows ?? { name: null, about: "" }; // from before cards: no record
      if (knows.name === name && (knows.about || "") === about) continue;
      if (!about && !knows.name) continue; // nothing to say yet: no card, and no name they know us by
      due.push(c);
    }
    return due;
  }

  /**
   * Send this agent's card to every contact that is due one (see cardsDue).
   * Returns the contacts told; a contact that can't be reached now is tried
   * again next time. Only a hub problem (nothing can be sent) propagates.
   */
  async syncCards(): Promise<Contact[]> {
    const told: Contact[] = [];
    for (const c of await this.cardsDue()) {
      try {
        await this.sendCard(c.npub);
      } catch (e) {
        if (e instanceof HubError) throw e;
        continue;
      }
      told.push(c);
    }
    return told;
  }

  /** Send this agent's card to one contact now. */
  async sendCard(name: string): Promise<HistoryEntry> {
    const [pubkey, contact] = await contacts.find(this.st, name);
    if (contact.status !== "approved") throw new Error(`${contact.alias} is ${contact.status}`);
    const myName = await this.alias(), about = await this.card();
    const conn = await this.connect();
    try {
      await conn.sendMessage(pubkey, contacts.cardText(myName, about));
    } finally {
      conn.close();
    }
    return (await locked(this.st, "state", async () => {
      await contacts.peerKnows(this.st, pubkey, myName, about);
      return record(this.st, {
        type: "card", direction: "out", peer: contact.npub, alias: contact.alias, name: myName, about,
        text: `card sent to ${contact.alias}`,
      });
    }))!;
  }

  /** Refuse to run an existing identity under another alias unless renaming: one agent, one home. */
  async checkHome(alias: string, rename = false): Promise<void> {
    const stored = (await this.st.get<Record<string, string>>("settings", {})).alias;
    if (!rename && stored && stored !== alias && (await this.hasIdentity())) {
      throw new IdentityError(`this directory belongs to ${stored}; use another MYOUS_HOME, or pass --rename if this is the same agent`);
    }
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

  /**
   * Send a file (protocol section 6): encrypt it with a fresh key, upload the
   * ciphertext to the hub, and send the key in a kind-15 file message.
   * `extraTags` adds worker semantics, e.g. [["w", "put", id, path]].
   */
  async sendFile(name: string, path: string, extraTags: string[][] = [], mime = "application/octet-stream"): Promise<HistoryEntry> {
    const [pubkey, contact] = await contacts.find(this.st, name);
    if (contact.status !== "approved") throw new Error(`${contact.alias} is ${contact.status}`);
    const fileName = sanitizeName(basename(path));
    if (!fileName) throw new Error(`not a usable file name: ${path}`);
    const enc = await encryptFile(new Uint8Array(await fs.readFile(path)));
    const info: Omit<FileInfo, "url" | "w"> = {
      name: fileName, mime, size: enc.ciphertext.length, x: enc.x, ox: enc.ox,
      key: Buffer.from(enc.key).toString("hex"), nonce: Buffer.from(enc.nonce).toString("hex"),
    };
    const desc = await (await this.blobs()).upload(enc.ciphertext, enc.x);
    const conn = await this.connect();
    try {
      await conn.sendFileMessage(pubkey, desc.url, [...fileTags(info), ...extraTags]);
    } finally {
      conn.close();
    }
    const w = extraTags.find((t) => t[0] === "w");
    return (await locked(this.st, "state", () => record(this.st, {
      type: "file", direction: "out", peer: contact.npub, alias: contact.alias, text: `file: ${fileName} (${enc.ciphertext.length} bytes)`,
      ...info, url: desc.url, ...(w ? { w: w.slice(1) } : {}),
    })))!;
  }

  /**
   * Download and decrypt a received file into `dir` (default: files/ in the
   * data directory). Verifies the blob against the message before writing.
   * Never overwrites: an existing name gets a numeric suffix.
   */
  async fetch(entry: HistoryEntry, dir?: string): Promise<string> {
    if (entry.type !== "file" || !entry.x || !entry.ox || !entry.key || !entry.nonce || !entry.name) {
      throw new Error("not a file entry");
    }
    const ciphertext = await (await this.blobs()).get(entry.x);
    const plaintext = await decryptFile(ciphertext, Buffer.from(entry.key, "hex"), Buffer.from(entry.nonce, "hex"), entry.x, entry.ox);
    const target = dir ?? this.filesDir();
    await fs.mkdir(target, { recursive: true, mode: 0o700 });
    const path = await freeName(target, sanitizeName(entry.name)!);
    await fs.writeFile(path, plaintext, { mode: 0o600 });
    return path;
  }

  /** Where fetched files go unless the caller says otherwise. */
  filesDir(): string {
    const home = (this.st as { home?: string }).home;
    return join(home ?? process.env.MYOUS_HOME ?? join(process.env.HOME ?? ".", ".myous"), "files");
  }

  private async blobs(): Promise<Blobs> {
    const cfg = await this.hub.config();
    return new Blobs(await this.key(), cfg.blob_api ?? this.hub.url + "/blob");
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
          text: `myous ${latest} is available (this client is v${VERSION}). Consider upgrading to exactly that version, ` +
            `from the registry or from verified source, as in ${url}/skill.md. What changed: ${url}/changelog.md`,
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
    await this.syncCards();
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
        try {
          await this.syncCards();
        } catch {
          // next round
        }
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
      // Worker replies (protocol section 7) are consumed by the command that
      // waits for them, by id, so they don't show up as new items.
      const entries = (await this.st.readHistory()).filter((e) =>
        e.seq > last && e.direction !== "out" && e.type !== "result" && e.type !== "ack" && !(e.type === "file" && e.w?.[0] === "file"));
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
    let blobApi: string | undefined;
    for (const [sender, rawText, rawSentAt, ms, part, kind, tags] of messages) {
      const contact = await contacts.approved(this.st, sender);
      if (!contact) continue; // not paired, or blocked: drop silently
      if (kind === KIND_FILE) {
        blobApi ??= (await this.hub.config()).blob_api ?? this.hub.url + "/blob";
        const info = parseFileTags(tags, rawText, blobApi);
        if (!info) continue; // malformed, or a blob that isn't on our hub
        stored.push(await record(this.st, {
          type: "file", direction: "in", peer: contact.npub, alias: contact.alias, sent_at: rawSentAt,
          text: `file: ${info.name} (${info.size} bytes)`, ...info,
        }));
        continue;
      }
      let text = rawText, sentAt = rawSentAt;
      if (part) {
        changed = true;
        const done = parts.add(buf, sender, part, rawText, rawSentAt, ms, now);
        if (!done) continue; // waiting for the other parts
        [text, sentAt] = done;
      }
      const base = { direction: "in" as const, peer: contact.npub, alias: contact.alias, sent_at: sentAt };
      const card = contacts.parseCard(text);
      if (card) {
        // A card (protocol section 4) is kept on the contact; the entry says what changed.
        const [, line] = await contacts.receiveCard(this.st, sender, card, now);
        stored.push(await record(this.st, { ...base, type: "card", name: card.name, about: card.about, text: line }));
        continue;
      }
      stored.push(await record(this.st, { ...base, type: "message", text, ...workerReply(text) }));
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

/**
 * A worker's reply (protocol 7.1) is a kind-14 message whose content is a
 * JSON object with "myous" "result" or "ack" and the request's id; such a
 * message is recorded under that type, with its fields, for the command
 * waiting on it. Anything else is an ordinary message.
 */
function workerReply(text: string): Partial<HistoryEntry> {
  if (!text.startsWith("{")) return {};
  let obj: any;
  try {
    obj = JSON.parse(text);
  } catch {
    return {};
  }
  if (!obj || typeof obj !== "object" || typeof obj.id !== "string" || !/^[0-9a-f]{32}$/.test(obj.id)) return {};
  if (obj.myous === "result") {
    return { type: "result", id: obj.id, exit: Number(obj.exit), stdout: String(obj.stdout ?? ""), stderr: String(obj.stderr ?? ""), truncated: Boolean(obj.truncated) };
  }
  if (obj.myous === "ack") {
    const fields: Partial<HistoryEntry> = { type: "ack", id: obj.id, ok: Boolean(obj.ok) };
    if (typeof obj.path === "string") fields.path = obj.path;
    if (typeof obj.size === "number") fields.size = obj.size;
    if (typeof obj.sha256 === "string") fields.sha256 = obj.sha256;
    if (typeof obj.error === "string") fields.error = obj.error;
    return fields;
  }
  return {};
}

/** `name` in `dir`, or `name` with a numeric suffix if it's taken. */
async function freeName(dir: string, name: string): Promise<string> {
  const dot = name.lastIndexOf(".");
  const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  for (let i = 0; ; i++) {
    const candidate = join(dir, i ? `${stem}-${i}${ext}` : name);
    try {
      await fs.access(candidate);
    } catch {
      return candidate;
    }
  }
}

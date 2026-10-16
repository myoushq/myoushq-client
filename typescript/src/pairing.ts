// Pairing two agents through the hub's mailbox. See protocol.md, "Pairing".
//
// A code looks like 4821-K7F3QX: the nameplate (4821) names a mailbox on
// the hub; the secret (K7F3QX) never leaves the two agents. Both sides run
// SPAKE2 on the full code through the mailbox, getting a key the hub can't
// learn, then send each other their identity encrypted with it. A wrong
// code or a tampering hub makes decryption fail and the pairing aborts.

import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { randomBytes } from "@noble/hashes/utils.js";
import { nip19 } from "nostr-tools";
import * as contacts from "./contacts.js";
import type { Contact, ContactContext } from "./contacts.js";
import { record } from "./history.js";
import { Hub, HubError, now } from "./hub.js";
import { pakeFinish, pakeStart, type Role } from "./pake.js";
import { locked, type Storage } from "./storage.js";

export const VERSION = 1;
const SECRET_LEN = 6;
// Crockford base32: no I, L, O, U, so codes survive being read aloud.
const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

export class PairingError extends Error {}

/** Claiming retries when the answer is lost, after retryDelayMs, then twice that. Tests shorten it. */
export const CLAIM = { attempts: 3, retryDelayMs: 1000 };

/** A mailbox message as posted: base64 of its JSON. */
function body(msg: unknown): string {
  return Buffer.from(JSON.stringify(msg)).toString("base64");
}

/** Whether a hub status turned the request down for good (gone, taken, bad), not "not now" (429) or a proxy's 5xx. */
function refused(status: number): boolean {
  return status >= 400 && status < 500 && status !== 429;
}

/** An error's message with its cause's ("fetch failed" alone says little). */
function describe(e: unknown): string {
  const err = e as { message?: string; cause?: { message?: string; code?: string } } | undefined;
  const cause = err?.cause?.code ?? err?.cause?.message;
  return [err?.message ?? String(e), cause].filter(Boolean).join(": ");
}

// --- codes -----------------------------------------------------------------

export function makeSecret(): string {
  // 256 is a multiple of 32, so taking each byte mod 32 is unbiased.
  return Array.from(randomBytes(SECRET_LEN), (b) => ALPHABET[b % 32]).join("");
}

export function formatCode(nameplate: string, secret: string): string {
  return `${nameplate}-${secret}`;
}

export function makeLink(linkBase: string, nameplate: string, secret: string): string {
  return `${linkBase}${nameplate}#${secret}`;
}

/** Accept a pairing link, or a code like "4821-K7F3QX" or "4821 k7f 3qx". */
export function parseCode(text: string, linkBase: string): [string, string] {
  text = text.trim();
  if (text.includes("://")) {
    const url = new URL(text);
    const expected = new URL(linkBase).host;
    if (url.host !== expected) throw new PairingError(`this link is for ${url.host}, but this agent uses ${expected}`);
    const m = /^\/p\/(\d+)$/.exec(url.pathname);
    if (!m || url.hash.length < 2) throw new PairingError("that doesn't look like a complete pairing link");
    return [m[1], normalizeSecret(url.hash.slice(1))];
  }
  const m = /^(\d+)[\s-]+([0-9A-Za-z\s-]+)$/.exec(text);
  if (!m) throw new PairingError("pairing codes look like 4821-K7F3QX");
  return [m[1], normalizeSecret(m[2])];
}

export function normalizeSecret(raw: string): string {
  const s = raw.replace(/[\s-]/g, "").toUpperCase().replace(/[IL]/g, "1").replace(/O/g, "0");
  if (s.length !== SECRET_LEN || [...s].some((c) => !ALPHABET.includes(c))) {
    throw new PairingError("the secret part of the code is not valid");
  }
  return s;
}

// --- crypto ----------------------------------------------------------------

const utf8 = new TextEncoder();

export function derive(key: Uint8Array, info: string, length = 32): Uint8Array {
  return hkdf(sha256, key, undefined, utf8.encode(`myous pairing v${VERSION} ${info}`), length);
}

/** Six digits both owners can compare by eye, if they want to. */
export function verifyCode(key: Uint8Array): string {
  const n = derive(key, "verify", 8).reduce((acc, b) => (acc << 8n) | BigInt(b), 0n);
  return (n % 1_000_000n).toString().padStart(6, "0");
}

export interface SealedMessage {
  t: "payload";
  v: number;
  n: string;
  c: string;
}

/** Encrypt a payload's JSON text for the peer. */
export function sealText(key: Uint8Array, role: Role, nameplate: string, plaintext: string, nonce = randomBytes(12)): SealedMessage {
  const c = chacha20poly1305(derive(key, `from ${role}`), nonce, utf8.encode(nameplate)).encrypt(utf8.encode(plaintext));
  return { t: "payload", v: VERSION, n: b64(nonce), c: b64(c) };
}

export interface Payload {
  v: number;
  pubkey: string;
  alias: string;
}

export function unseal(key: Uint8Array, peerRole: Role, nameplate: string, msg: SealedMessage): Payload {
  try {
    const plain = chacha20poly1305(derive(key, `from ${peerRole}`), unb64(msg.n), utf8.encode(nameplate)).decrypt(unb64(msg.c));
    const payload = JSON.parse(new TextDecoder().decode(plain)) as Payload;
    nip19.npubEncode(payload.pubkey); // throws on a malformed key
    if (!/^[0-9a-f]{64}$/.test(payload.pubkey)) throw new Error("bad key");
    return payload;
  } catch {
    throw new PairingError("the code didn't match (or the exchange was tampered with)");
  }
}

function b64(b: Uint8Array): string {
  return Buffer.from(b).toString("base64");
}

function unb64(s: string): Uint8Array {
  return new Uint8Array(Buffer.from(s, "base64"));
}

// --- the exchange ----------------------------------------------------------
// A pairing spans a few round trips, so its state is a "pending/<nameplate>"
// document, finished by whichever call gets there first: accept (which
// waits a while) or any later advance().

export interface Pending {
  role: Role;
  nameplate: string;
  secret: string;
  token: string;
  expires_at: number;
  seed: string; // recreates our SPAKE2 instance
  after: number;
  /** "posting": our PAKE message (`message`, the base64 body) is saved but not confirmed posted. */
  stage: "posting" | "wait_pake" | "wait_payload" | "done" | "failed" | "elsewhere";
  message?: string;
  key?: string;
  /** Relationship context to record on the contact once paired. */
  context?: ContactContext;
  contact?: Contact;
  verify?: string;
  error?: string;
  /** The handshake so far, for the owner to read when it fails. */
  trace?: TraceStep[];
}

/**
 * One step of a pairing's handshake, kept in the pairing's own record (and
 * in "pairing-trace" when it fails) so the owner can see what happened; the
 * hub logs nothing. It never leaves this machine.
 */
export interface TraceStep {
  at: number;
  step: "claim" | "post" | "post (repeat)" | "wait" | "done" | "failed";
  host?: string;
  error?: string;
  /** A resumed claim: when the hub gave it. */
  claimed_at?: number;
}

const TRACE_LEN = 20; // steps kept per pairing

/** The last failed pairing's nameplate and steps, oldest first, if it failed at or after `since` (Unix seconds). */
export async function pairingTrace(st: Storage, since: number): Promise<{ nameplate: string; trace: TraceStep[] } | null> {
  const t = await st.get<{ nameplate: string; trace: TraceStep[] } | null>("pairing-trace", null);
  const last = t?.trace?.at(-1);
  return t && last && last.at >= since ? t : null;
}

export interface Invite extends Pending {
  code: string;
  link: string;
}

export class Pairing {
  constructor(private st: Storage, private hub: Hub, private pubkey: string, private alias: string) {}

  async pending(): Promise<Pending[]> {
    const found = await Promise.all((await this.st.names("pending/")).map((n) => this.st.get<Pending | null>(n, null)));
    return found.filter((p): p is Pending => p !== null);
  }

  /** Open a mailbox and post our half of the PAKE. Returns the code and link to share. */
  async invite(context?: ContactContext): Promise<Invite> {
    const box = await this.hub.request("POST", "/api/pair", {});
    const nameplate: string = box.nameplate;
    const secret = makeSecret();
    const seed = randomBytes(32);
    await this.post(nameplate, box.token, { t: "pake", v: VERSION, m: b64(pakeStart("a", formatCode(nameplate, secret), seed)) });
    const p: Pending = {
      role: "a", nameplate, secret, token: box.token, expires_at: box.expires_at,
      seed: b64(seed), after: 0, stage: "wait_pake", ...(context ? { context } : {}),
    };
    await this.save(p);
    const linkBase = (await this.hub.config()).pair_link_base;
    return { ...p, code: formatCode(nameplate, secret), link: makeLink(linkBase, nameplate, secret) };
  }

  /** Join someone else's invite; finishes now if the other side answers within `wait` seconds. */
  async accept(code: string, wait = 60, context?: ContactContext): Promise<Pending> {
    const [nameplate, secret] = parseCode(code, (await this.hub.config()).pair_link_base);
    const mine = await this.st.get<Pending | null>(`pending/${nameplate}`, null);
    if (mine && mine.role === "b" && mine.secret === secret) {
      // Accepted before (e.g. the connection dropped); carry on with it.
      await this.st.delete(`claiming/${nameplate}`);
      return this.advance(mine, wait);
    }
    const claiming = await this.st.get<{ claim?: unknown; trace?: TraceStep[] } | null>(`claiming/${nameplate}`, null);
    const trace = claiming?.trace ?? [];
    let claim;
    try {
      claim = await this.claim(nameplate, claiming?.claim, trace);
    } catch (e) {
      await this.st.put("pairing-trace", { nameplate, trace });
      throw e;
    }
    // Saved before it is posted ("posting"): if the answer to the post is
    // lost, advance() posts exactly this message again, never a new one.
    const seed = randomBytes(32);
    const p: Pending = {
      role: "b", nameplate, secret, token: claim.token, expires_at: claim.expires_at,
      seed: b64(seed), after: 0, stage: "posting", ...(context ? { context } : {}), trace,
      message: body({ t: "pake", v: VERSION, m: b64(pakeStart("b", formatCode(nameplate, secret), seed)) }),
    };
    await this.save(p);
    await this.st.delete(`claiming/${nameplate}`);
    return this.advance(p, wait);
  }

  /**
   * Claim the invite's second side, retrying when the answer is lost (the
   * connection drops, or a proxy answers 5xx).
   *
   * The claim carries a random id, saved in "claiming/<nameplate>" before
   * the first attempt: the hub answers a repeat of the same id with the
   * same token, so neither a retry here nor running the same accept again
   * (after a crash, or when every attempt failed) burns the code. The
   * record goes once the pairing is saved or the hub refuses the code; it
   * stays while the hub can't be reached. `saved` is its id, if any;
   * `trace` gets a step per attempt and is saved with the id.
   */
  private async claim(nameplate: string, saved: unknown, trace: TraceStep[]): Promise<{ token: string; expires_at: number }> {
    const name = `claiming/${nameplate}`;
    const id = typeof saved === "string" && /^[0-9a-f]{32}$/.test(saved) ? saved : Buffer.from(randomBytes(16)).toString("hex");
    let lost: unknown;
    for (let attempt = 0; attempt < CLAIM.attempts; attempt++) {
      await this.st.put(name, { nameplate, claim: id, trace });
      try {
        const answer = await this.hub.request("POST", `/api/pair/${nameplate}/claim`, { claim: id });
        this.trace(trace, "claim", undefined, answer.repeat ? answer.claimed_at : undefined);
        return answer;
      } catch (e) {
        this.trace(trace, "claim", e);
        if (e instanceof HubError && e.status < 500) {
          // Turned down; a 429 keeps the record for later.
          if (refused(e.status)) await this.st.delete(name);
          throw new PairingError(e.reason);
        }
        if (e instanceof SyntaxError) throw e;
        lost = e; // the connection dropped, or a proxy couldn't get the hub's answer
      }
      if (attempt + 1 < CLAIM.attempts) await new Promise((r) => setTimeout(r, CLAIM.retryDelayMs * (attempt + 1)));
    }
    await this.st.put(name, { nameplate, claim: id, trace });
    throw new Error(`could not reach the hub to accept the code (${describe(lost)}); `
      + "accepting the same code again carries on where this stopped", { cause: lost });
  }

  /** Note a step of the handshake (see TraceStep). */
  private trace(trace: TraceStep[], step: TraceStep["step"], error?: unknown, claimedAt?: number): void {
    const s: TraceStep = { at: now(), step };
    if (step === "claim" || step === "post" || step === "post (repeat)" || step === "wait") s.host = new URL(this.hub.url).host;
    if (error !== undefined) s.error = typeof error === "string" ? error : describe(error);
    if (claimedAt !== undefined) s.claimed_at = claimedAt;
    trace.push(s);
    trace.splice(0, Math.max(0, trace.length - TRACE_LEN));
  }

  /** Move every pending pairing forward without waiting; returns the ones that finished. */
  async advanceAll(): Promise<Pending[]> {
    const finished: Pending[] = [];
    for (const p of await this.pending()) {
      try {
        const r = await this.advance(p, 0, false);
        if (r.stage === "done" || r.stage === "failed") finished.push(r);
      } catch (e) {
        if (!(e instanceof HubError) && !(e instanceof TypeError)) throw e; // hub unreachable: retry later
      }
    }
    return finished;
  }

  /** Move one pairing forward with whatever the peer has posted. */
  async advance(p: Pending, wait = 0, block = true): Promise<Pending> {
    const r = await locked(this.st, `pending/${p.nameplate}`, async () => {
      const current = await this.st.get<Pending | null>(`pending/${p.nameplate}`, null);
      if (!current) return { ...p, stage: "elsewhere" as const };
      return this.advanceLocked(current, wait);
    }, block);
    return r ?? p;
  }

  private async advanceLocked(p: Pending, wait: number): Promise<Pending> {
    const deadline = now() + wait;
    for (;;) {
      if (now() > p.expires_at) return this.fail(p, "pairing invite expired");
      if (p.stage === "posting") {
        // Our PAKE message, saved before the first try; the hub takes a
        // repeat of a side's last message without appending it.
        try {
          const answer = await this.hub.request("POST", `/api/pair/${p.nameplate}/messages`, { body: p.message }, p.token);
          this.trace((p.trace ??= []), answer?.repeat ? "post (repeat)" : "post");
          delete p.message;
          p.stage = "wait_pake";
          await this.save(p);
        } catch (e) {
          this.trace((p.trace ??= []), "post", e);
          if (e instanceof HubError && (e.status === 403 || e.status === 404)) return this.fail(p, "pairing invite expired or was closed");
          if (e instanceof HubError && e.status < 500) return this.fail(p, e.reason);
          if (e instanceof SyntaxError) throw e;
          await this.save(p); // unreachable: post it again while there's time, else next poll
          if (now() + 2 >= deadline) return p;
          await new Promise((r) => setTimeout(r, 2000));
          continue;
        }
      }
      let got;
      try {
        const w = Math.max(0, Math.min(25, deadline - now()));
        got = await this.hub.request("GET", `/api/pair/${p.nameplate}/messages?after=${p.after}&wait=${w}`, undefined, p.token);
      } catch (e) {
        if (e instanceof HubError && (e.status === 403 || e.status === 404)) {
          return this.fail(p, "pairing invite expired or was closed");
        }
        if (e instanceof HubError || e instanceof SyntaxError) throw e;
        // Network trouble (proxies drop long polls): retry while there's
        // time, else leave it pending for the next poll.
        this.trace((p.trace ??= []), "wait", e);
        if (now() + 2 >= deadline) return p;
        await new Promise((r) => setTimeout(r, 2000));
        continue;
      }
      for (const body of got.messages as string[]) {
        p.after += 1;
        try {
          await this.step(p, JSON.parse(Buffer.from(body, "base64").toString("utf8")));
        } catch (e) {
          if (e instanceof PairingError) return this.fail(p, e.message);
          throw e;
        }
        if (p.stage === "done") return p;
      }
      await this.save(p);
      if (now() >= deadline) return p;
    }
  }

  private async step(p: Pending, msg: any): Promise<void> {
    const peerRole: Role = p.role === "a" ? "b" : "a";
    if (p.stage === "wait_pake" && msg?.t === "pake") {
      let key: Uint8Array;
      try {
        key = pakeFinish(p.role, formatCode(p.nameplate, p.secret), unb64(p.seed), unb64(msg.m));
      } catch {
        throw new PairingError("bad message from the other side");
      }
      p.key = b64(key);
      p.stage = "wait_payload";
      await this.save(p);
      const mine = JSON.stringify({ alias: this.alias, pubkey: this.pubkey, v: VERSION });
      await this.post(p.nameplate, p.token, sealText(key, p.role, p.nameplate, mine));
    } else if (p.stage === "wait_payload" && msg?.t === "payload") {
      const key = unb64(p.key!);
      const payload = unseal(key, peerRole, p.nameplate, msg);
      if (payload.pubkey === this.pubkey) throw new PairingError("that's this agent's own invite");
      await locked(this.st, "state", async () => {
        let contact = await contacts.add(this.st, payload.pubkey, contacts.cleanName(String(payload.alias ?? "")));
        const fields = contacts.contextFields(p.context);
        if (Object.keys(fields).length) contact = await contacts.update(this.st, payload.pubkey, fields);
        // They learned our alias from the payload; a card follows if we have one.
        await contacts.peerKnows(this.st, payload.pubkey, this.alias, "");
        p.stage = "done";
        this.trace((p.trace ??= []), "done");
        p.contact = contact;
        p.verify = verifyCode(key);
        await this.st.delete(`pending/${p.nameplate}`);
        let text = `paired with ${contact.alias} (verification code ${p.verify})`;
        if (!contact.relationship) {
          text += `. Ask your owner how they know this contact and what you may share with it, ` +
            `then record it: myous context ${JSON.stringify(contact.alias)} --relationship ... --sharing "..."`;
        }
        await record(this.st, { type: "paired", peer: contact.npub, alias: contact.alias, text });
      });
      // Don't close the mailbox: the peer may not have read our payload yet.
    } else {
      throw new PairingError("unexpected message from the other side");
    }
  }

  private async fail(p: Pending, error: string): Promise<Pending> {
    this.trace((p.trace ??= []), "failed", error);
    await this.st.put("pairing-trace", { nameplate: p.nameplate, trace: p.trace });
    await this.st.delete(`pending/${p.nameplate}`);
    await this.hub.request("DELETE", `/api/pair/${p.nameplate}`, undefined, p.token).catch(() => {});
    await locked(this.st, "state", () =>
      record(this.st, { type: "pairing_failed", text: `pairing ${p.nameplate} failed: ${error}` }));
    return { ...p, stage: "failed", error };
  }

  private save(p: Pending): Promise<void> {
    return this.st.put(`pending/${p.nameplate}`, p);
  }

  private async post(nameplate: string, token: string, msg: unknown): Promise<void> {
    await this.hub.request("POST", `/api/pair/${nameplate}/messages`, { body: body(msg) }, token);
  }
}

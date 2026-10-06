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
import type { Contact } from "./contacts.js";
import { record } from "./history.js";
import { Hub, HubError, now } from "./hub.js";
import { pakeFinish, pakeStart, type Role } from "./pake.js";
import { locked, type Storage } from "./storage.js";

export const VERSION = 1;
const SECRET_LEN = 6;
// Crockford base32: no I, L, O, U, so codes survive being read aloud.
const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

export class PairingError extends Error {}

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
  stage: "wait_pake" | "wait_payload" | "done" | "failed" | "elsewhere";
  key?: string;
  contact?: Contact;
  verify?: string;
  error?: string;
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
  async invite(): Promise<Invite> {
    const box = await this.hub.request("POST", "/api/pair", {});
    const nameplate: string = box.nameplate;
    const secret = makeSecret();
    const seed = randomBytes(32);
    await this.post(nameplate, box.token, { t: "pake", v: VERSION, m: b64(pakeStart("a", formatCode(nameplate, secret), seed)) });
    const p: Pending = {
      role: "a", nameplate, secret, token: box.token, expires_at: box.expires_at,
      seed: b64(seed), after: 0, stage: "wait_pake",
    };
    await this.save(p);
    const linkBase = (await this.hub.config()).pair_link_base;
    return { ...p, code: formatCode(nameplate, secret), link: makeLink(linkBase, nameplate, secret) };
  }

  /** Join someone else's invite; finishes now if the other side answers within `wait` seconds. */
  async accept(code: string, wait = 60): Promise<Pending> {
    const [nameplate, secret] = parseCode(code, (await this.hub.config()).pair_link_base);
    const mine = await this.st.get<Pending | null>(`pending/${nameplate}`, null);
    if (mine && mine.role === "b" && mine.secret === secret) {
      // Accepted before (e.g. the connection dropped); carry on with it.
      return this.advance(mine, wait);
    }
    let claim;
    try {
      claim = await this.hub.request("POST", `/api/pair/${nameplate}/claim`, {});
    } catch (e) {
      if (e instanceof HubError) throw new PairingError(e.reason);
      throw e;
    }
    const seed = randomBytes(32);
    await this.post(nameplate, claim.token, { t: "pake", v: VERSION, m: b64(pakeStart("b", formatCode(nameplate, secret), seed)) });
    const p: Pending = {
      role: "b", nameplate, secret, token: claim.token, expires_at: claim.expires_at,
      seed: b64(seed), after: 0, stage: "wait_pake",
    };
    await this.save(p);
    return this.advance(p, wait);
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
        const contact = await contacts.add(this.st, payload.pubkey, String(payload.alias ?? "").slice(0, 64));
        p.stage = "done";
        p.contact = contact;
        p.verify = verifyCode(key);
        await this.st.delete(`pending/${p.nameplate}`);
        await record(this.st, {
          type: "paired", peer: contact.npub, alias: contact.alias,
          text: `paired with ${contact.alias} (verification code ${p.verify})`,
        });
      });
      // Don't close the mailbox: the peer may not have read our payload yet.
    } else {
      throw new PairingError("unexpected message from the other side");
    }
  }

  private async fail(p: Pending, error: string): Promise<Pending> {
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
    const body = Buffer.from(JSON.stringify(msg)).toString("base64");
    await this.hub.request("POST", `/api/pair/${nameplate}/messages`, { body }, token);
  }
}

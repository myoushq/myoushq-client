// Files: a file travels as an encrypted blob on the hub plus a file message
// that carries the key (protocol.md section 6). The hub stores bytes it
// can't read and learns neither the recipient nor the file name.

import { createHash, randomBytes, webcrypto } from "node:crypto";
import { finalizeEvent } from "nostr-tools/pure";
import { HubError } from "./hub.js";
import { proxyFor, requestRawVia } from "./proxy.js";

export const KIND_BLOB_AUTH = 24242;
/** How long a blob authorization is good for; the hub allows up to 10 minutes. */
const AUTH_TTL = 5 * 60;

export interface EncryptedFile {
  key: Uint8Array;
  nonce: Uint8Array;
  ciphertext: Uint8Array;
  /** SHA-256 of the ciphertext, hex: the blob's name on the hub. */
  x: string;
  /** SHA-256 of the plaintext, hex. */
  ox: string;
}

/** What a file message says about its blob; `key` and `nonce` are hex. */
export interface FileInfo {
  name: string;
  mime: string;
  size: number;
  x: string;
  ox: string;
  url: string;
  key: string;
  nonce: string;
  /** Worker semantics (protocol section 7), e.g. ["put", id, path] or ["file", id]. */
  w?: string[];
}

/** A copy backed by a plain ArrayBuffer, which WebCrypto's types insist on. */
function plain(b: Uint8Array): Uint8Array<ArrayBuffer> {
  return new Uint8Array(b);
}

export function sha256hex(data: Uint8Array): string {
  return createHash("sha256").update(data).digest("hex");
}

/** AES-256-GCM with a fresh key and nonce per file (or the given ones, for tests). */
export async function encryptFile(plaintext: Uint8Array, key = randomBytes(32), nonce = randomBytes(12)): Promise<EncryptedFile> {
  if (key.length !== 32 || nonce.length !== 12) throw new Error("file key must be 32 bytes and the nonce 12");
  const k = await webcrypto.subtle.importKey("raw", plain(key), "AES-GCM", false, ["encrypt"]);
  const ciphertext = new Uint8Array(await webcrypto.subtle.encrypt({ name: "AES-GCM", iv: plain(nonce) }, k, plain(plaintext)));
  return { key: new Uint8Array(key), nonce: new Uint8Array(nonce), ciphertext, x: sha256hex(ciphertext), ox: sha256hex(plaintext) };
}

/** Decrypt, checking the ciphertext hash first and the plaintext hash after. */
export async function decryptFile(ciphertext: Uint8Array, key: Uint8Array, nonce: Uint8Array, x?: string, ox?: string): Promise<Uint8Array> {
  if (x !== undefined && sha256hex(ciphertext) !== x) throw new Error("the blob's contents don't match the message (x)");
  const k = await webcrypto.subtle.importKey("raw", plain(key), "AES-GCM", false, ["decrypt"]);
  let plaintext: Uint8Array;
  try {
    plaintext = new Uint8Array(await webcrypto.subtle.decrypt({ name: "AES-GCM", iv: plain(nonce) }, k, plain(ciphertext)));
  } catch {
    throw new Error("the file can't be decrypted: wrong key or tampered blob");
  }
  if (ox !== undefined && sha256hex(plaintext) !== ox) throw new Error("the decrypted file doesn't match the message (ox)");
  return plaintext;
}

/** A file name only: the last path component; null for empty, "." and "..". */
export function sanitizeName(name: string): string | null {
  const last = name.split(/[\\/]/).pop() ?? "";
  if (!last || last === "." || last === ".." || /[\p{Cc}]/u.test(last)) return null;
  return last.slice(0, 255);
}

/** The tags of a kind-15 file message (protocol 6.3), after "p" and "ms". */
export function fileTags(info: Omit<FileInfo, "url" | "w">): string[][] {
  return [
    ["file-type", info.mime],
    ["encryption-algorithm", "aes-gcm"],
    ["decryption-key", info.key],
    ["decryption-nonce", info.nonce],
    ["x", info.x],
    ["ox", info.ox],
    ["size", String(info.size)],
    ["name", info.name],
  ];
}

/** Reads a received file message; null if it's malformed or points off the hub. */
export function parseFileTags(tags: string[][], content: string, blobApi: string): FileInfo | null {
  const tag = (name: string) => tags.find((t) => t[0] === name)?.[1];
  const hex = (v: string | undefined, bytes: number) => (v && new RegExp(`^[0-9a-f]{${bytes * 2}}$`).test(v) ? v : undefined);
  const key = hex(tag("decryption-key"), 32), nonce = hex(tag("decryption-nonce"), 12);
  const x = hex(tag("x"), 32), ox = hex(tag("ox"), 32);
  const name = sanitizeName(tag("name") ?? "");
  const size = Number(tag("size"));
  if (tag("encryption-algorithm") !== "aes-gcm" || !key || !nonce || !x || !ox || !name || !Number.isInteger(size) || size < 0) return null;
  const base = blobApi.replace(/\/+$/, "") + "/";
  if (content !== base + x) return null;
  const w = tags.find((t) => t[0] === "w");
  return { name, mime: tag("file-type") || "application/octet-stream", size, x, ox, url: content, key, nonce, ...(w ? { w: w.slice(1) } : {}) };
}

/** The Authorization header for a blob call: a signed BUD-11 event, base64url. */
export function blobAuth(secretKey: Uint8Array, action: "upload" | "get" | "delete", x: string, now = Math.floor(Date.now() / 1000)): string {
  const event = finalizeEvent({
    kind: KIND_BLOB_AUTH, content: `myous ${action}`, created_at: now,
    tags: [["t", action], ["x", x], ["expiration", String(now + AUTH_TTL)]],
  }, secretKey);
  return "Nostr " + Buffer.from(JSON.stringify(event)).toString("base64url");
}

export interface BlobDescriptor {
  url: string;
  sha256: string;
  size: number;
  type: string;
  uploaded: number;
  expires: number;
}

/** The hub's blob store (protocol 6.2), authorized with this agent's key. */
export class Blobs {
  constructor(private secretKey: Uint8Array, readonly blobApi: string) {}

  async upload(ciphertext: Uint8Array, x = sha256hex(ciphertext)): Promise<BlobDescriptor> {
    const resp = await this.call("PUT", "/upload", blobAuth(this.secretKey, "upload", x), ciphertext);
    return JSON.parse(resp.toString("utf8")) as BlobDescriptor;
  }

  async get(x: string): Promise<Uint8Array> {
    return new Uint8Array(await this.call("GET", "/" + x, blobAuth(this.secretKey, "get", x)));
  }

  async delete(x: string): Promise<void> {
    await this.call("DELETE", "/" + x, blobAuth(this.secretKey, "delete", x));
  }

  private async call(method: string, endpoint: string, auth: string, body?: Uint8Array): Promise<Buffer> {
    const url = this.blobApi.replace(/\/+$/, "") + endpoint;
    const headers: Record<string, string> = { Authorization: auth, Accept: "application/octet-stream, application/json" };
    if (body) {
      headers["Content-Type"] = "application/octet-stream";
      headers["Content-Length"] = String(body.length);
    }
    const proxy = proxyFor(url);
    const resp = proxy
      ? await requestRawVia(proxy, url, { method, headers, body: body && Buffer.from(body), timeoutMs: 300_000 })
      : await fetch(url, { method, headers, body: body && plain(body), signal: AbortSignal.timeout(300_000) })
          .then(async (r) => ({ status: r.status, statusText: r.statusText, body: Buffer.from(await r.arrayBuffer()) }));
    if (resp.status < 200 || resp.status > 299) {
      let reason = resp.statusText;
      try {
        reason = JSON.parse(resp.body.toString("utf8")).error ?? reason;
      } catch {}
      throw new HubError(resp.status, reason);
    }
    return resp.body;
  }
}

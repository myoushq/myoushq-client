// Long messages: splitting text into parts and putting it back together.
// See PROTOCOL.md, "Long messages".
//
// One message holds about 28 KB of text. Longer text goes out as up to 16
// parts, each a normal message tagged ["part", id, index, total]; the
// receiver buffers them and delivers one message when all have arrived.

import { randomBytes } from "node:crypto";

export const PART_BYTES = 24_000; // JSON-escaped size of one part's text
export const MAX_PARTS = 16;
export const MAX_BYTES = 262_144; // UTF-8 size of a whole message
export const MAX_UNFINISHED = 4; // per sender
export const UNFINISHED_TTL = 3600; // seconds after the first part arrived

const encoder = new TextEncoder();
const utf8Length = (s: string) => encoder.encode(s).length;

function escapedSize(c: string): number {
  if ('"\\\b\f\n\r\t'.includes(c)) return 2;
  if ("<>&  ".includes(c) || c.codePointAt(0)! < 0x20) return 6;
  return utf8Length(c);
}

/** The parts to send `text` in (one, if it fits). Throws if it's too long to send at all. */
export function split(text: string): string[] {
  const size = utf8Length(text);
  if (size > MAX_BYTES) {
    throw new Error(`message is ${size} bytes; the limit is ${MAX_BYTES}. Shorten it or send it in several messages`);
  }
  const parts: string[] = [];
  let current = "";
  let currentSize = 0;
  for (const c of text) { // by code point
    const n = escapedSize(c);
    if (current && currentSize + n > PART_BYTES) {
      parts.push(current);
      current = "";
      currentSize = 0;
    }
    current += c;
    currentSize += n;
  }
  parts.push(current);
  if (parts.length > MAX_PARTS) throw new Error(`message needs ${parts.length} parts; the limit is ${MAX_PARTS}. Shorten it`);
  return parts;
}

export function newId(): string {
  return randomBytes(16).toString("hex");
}

export interface Part {
  id: string;
  index: number;
  total: number;
}

/** Reads a ["part", id, index, total] tag; null if malformed. */
export function parseTag(t: string[]): Part | null {
  if (t.length < 4 || !/^[0-9a-f]{1,64}$/.test(t[1]) || !/^\d+$/.test(t[2]) || !/^\d+$/.test(t[3])) return null;
  const index = Number(t[2]), total = Number(t[3]);
  if (total < 2 || total > MAX_PARTS || index < 1 || index > total) return null;
  return { id: t[1], index, total };
}

/** A long message waiting for parts, in the "partials" document. */
export interface Unfinished {
  sender: string;
  total: number;
  parts: Record<string, string>;
  first: number;
  sent_at?: number;
  ms?: number;
}

export type Buffer = Record<string, Unfinished>;

/** Buffers one part. Returns [text, sentAt, ms] once the message is complete. */
export function add(buf: Buffer, sender: string, part: Part, text: string, sentAt: number, ms: number, now: number): [string, number, number] | null {
  const key = `${sender}:${part.id}`;
  if (!buf[key]) {
    const mine = Object.keys(buf).filter((k) => buf[k].sender === sender).sort((a, b) => buf[a].first - buf[b].first);
    for (const old of mine.slice(0, Math.max(0, mine.length - MAX_UNFINISHED + 1))) delete buf[old];
    buf[key] = { sender, total: part.total, parts: {}, first: now };
  }
  const u = buf[key];
  if (u.total !== part.total || String(part.index) in u.parts) return null;
  if (Object.values(u.parts).reduce((n, t) => n + utf8Length(t), 0) + utf8Length(text) > MAX_BYTES) {
    delete buf[key];
    return null;
  }
  u.parts[String(part.index)] = text;
  if (part.index === 1) {
    u.sent_at = sentAt;
    u.ms = ms;
  }
  if (Object.keys(u.parts).length < u.total) return null;
  delete buf[key];
  let joined = "";
  for (let i = 1; i <= u.total; i++) joined += u.parts[String(i)];
  return [joined, u.sent_at ?? sentAt, u.ms ?? ms];
}

/** Removes messages unfinished for too long; returns [sender, text, sentAt]
 * for each, with markers where parts are missing. */
export function expire(buf: Buffer, now: number): [string, string, number][] {
  const out: [string, string, number][] = [];
  for (const [key, u] of Object.entries(buf)) {
    if (now - u.first <= UNFINISHED_TTL) continue;
    delete buf[key];
    let text = "";
    for (let i = 1; i <= u.total; i++) text += u.parts[String(i)] ?? `[part ${i} of ${u.total} missing]`;
    out.push([u.sender, text, u.sent_at ?? u.first]);
  }
  return out;
}

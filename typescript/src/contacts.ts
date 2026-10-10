// Paired peers. Only `approved` contacts can reach this agent.
// Statuses: approved, blocked (`pending` is reserved for future contact requests).
//
// Cards (protocol.md, section 4): `card` is the latest {name, about, at} the
// contact sent about itself; `peer_knows` is {name, about} as this agent
// last told the contact (the alias at pairing, then each card sent), so a
// change is sent once. The contact's `alias` is this agent's own label for
// it and never follows a card.
//
// Callers hold the "state" lock around changes.

import { nip19 } from "nostr-tools";
import type { Storage } from "./storage.js";

export interface Contact {
  alias: string;
  npub: string;
  status: "approved" | "blocked";
  paired_at: number;
  /** How the owner knows this contact (one of RELATIONSHIPS), kept only here. */
  relationship?: string;
  /** The owner's guidance on what may be shared with this contact. */
  sharing?: string;
  /** Who made the pairing when it wasn't the agent itself ("owner": through the myous desktop app). */
  added_by?: string;
  /** What the contact last said about itself (its card), and when. */
  card?: { name: string; about: string; at: number };
  /** What this agent last told the contact about itself. */
  peer_knows?: { name: string; about: string };
}

export const RELATIONSHIPS = ["family", "friend", "colleague", "business", "service", "other"];
const MAX_SHARING = 500;
export const MAX_NAME = 64; // an alias, as in the pairing payload
export const MAX_ABOUT = 500; // a card's self-description

/** Relationship context to record on a contact; missing fields are left as they are. */
export interface ContactContext {
  relationship?: string;
  sharing?: string;
  added_by?: string;
}

/** Validated fields to store (only what's given). Throws on bad input. */
export function contextFields(cc: ContactContext = {}): Partial<Contact> {
  const fields: Partial<Contact> = {};
  if (cc.added_by?.trim()) fields.added_by = cc.added_by.trim();
  if (cc.relationship !== undefined) {
    if (!RELATIONSHIPS.includes(cc.relationship)) throw new Error(`relationship must be one of: ${RELATIONSHIPS.join(", ")}`);
    fields.relationship = cc.relationship;
  }
  if (cc.sharing !== undefined) {
    if ([...cc.sharing].length > MAX_SHARING) throw new Error(`sharing guidance is limited to ${MAX_SHARING} characters`);
    if (cc.sharing.trim()) fields.sharing = cc.sharing.trim();
  }
  return fields;
}

/** The current relationship context of the contact with this npub (the
 * owner's), and what the contact says about itself (its card's about). */
export async function contextOf(st: Storage, npub: string): Promise<{ relationship: string | null; sharing: string | null; about: string | null }> {
  const c = Object.values(await load(st)).find((c) => c.npub === npub);
  return { relationship: c?.relationship ?? null, sharing: c?.sharing ?? null, about: c?.card?.about || null };
}

// --- cards (protocol.md, section 4) ------------------------------------------

export interface Card {
  name: string;
  about: string;
}

/** The {name, about} of a card message, or null when the text is not a
 * valid card (then it stays an ordinary message). */
export function parseCard(text: string): Card | null {
  if (!text.startsWith("{")) return null;
  let obj: any;
  try {
    obj = JSON.parse(text);
  } catch {
    return null;
  }
  if (!obj || typeof obj !== "object" || Array.isArray(obj) || obj.myous !== "card") return null;
  const name = obj.name, about = obj.about ?? "";
  if (typeof name !== "string" || typeof about !== "string") return null;
  const card = { name: name.trim(), about: about.trim() };
  // Lengths in code points, as Python's len() counts them.
  const nameLen = [...card.name].length;
  if (nameLen < 1 || nameLen > MAX_NAME || [...card.about].length > MAX_ABOUT) return null;
  return card;
}

/** The JSON of this agent's card. */
export function cardText(name: string, about: string): string {
  return JSON.stringify({ myous: "card", name, about });
}

/**
 * Store a contact's card; returns the contact and a line for the history
 * saying what changed: a new or changed description, a new name
 * (announced, never applied: the alias is ours), or both.
 */
export async function receiveCard(st: Storage, pubkey: string, card: Card, at: number): Promise<[Contact, string]> {
  const contacts = await load(st);
  const c = contacts[pubkey];
  const old = c.card;
  const knownName = old?.name || c.alias;
  const bits: string[] = [];
  if (card.name !== knownName) {
    bits.push(`now calls itself "${card.name}"; you call it "${c.alias}" ` +
      `(keep that, or follow it: myous rename "${c.alias}" "${card.name}")`);
  }
  if (card.about !== (old?.about || "")) bits.push(card.about ? `describes itself: ${card.about}` : "cleared its description");
  if (!bits.length) bits.push("sent its card again, unchanged");
  c.card = { name: card.name, about: card.about, at };
  await st.put("contacts", contacts);
  return [c, `${c.alias} ` + bits.join("; ")];
}

/** Record what this agent has told a contact about itself. */
export async function peerKnows(st: Storage, pubkey: string, name: string, about: string): Promise<void> {
  const contacts = await load(st);
  if (!contacts[pubkey]) return;
  contacts[pubkey].peer_knows = { name, about };
  await st.put("contacts", contacts);
}

export type Contacts = Record<string, Contact>;

export async function load(st: Storage): Promise<Contacts> {
  return st.get<Contacts>("contacts", {});
}

/** Pin a peer as approved. Re-pairing with a known key keeps its alias. */
export async function add(st: Storage, pubkey: string, alias: string): Promise<Contact> {
  const contacts = await load(st);
  if (contacts[pubkey]) {
    contacts[pubkey].status = "approved";
  } else {
    contacts[pubkey] = {
      alias: uniqueAlias(contacts, alias || "peer"),
      npub: nip19.npubEncode(pubkey),
      status: "approved",
      paired_at: Math.floor(Date.now() / 1000),
    };
  }
  await st.put("contacts", contacts);
  return contacts[pubkey];
}

/** Look up a contact by alias, npub or hex key. */
export async function find(st: Storage, name: string): Promise<[string, Contact]> {
  const entries = Object.entries(await load(st));
  const exact = entries.find(([k, c]) => name === c.alias || name === c.npub || name === k);
  const found = exact ?? entries.find(([, c]) => c.alias.toLowerCase() === name.toLowerCase());
  if (!found) throw new Error(`no contact named ${JSON.stringify(name)}`);
  return found;
}

export async function update(st: Storage, name: string, fields: Partial<Contact>): Promise<Contact> {
  const [pubkey] = await find(st, name);
  const contacts = await load(st);
  if (fields.alias && Object.entries(contacts).some(([k, c]) => k !== pubkey && c.alias === fields.alias)) {
    throw new Error(`alias ${JSON.stringify(fields.alias)} is already used`);
  }
  Object.assign(contacts[pubkey], fields);
  await st.put("contacts", contacts);
  return contacts[pubkey];
}

export async function approved(st: Storage, pubkey: string): Promise<Contact | undefined> {
  const c = (await load(st))[pubkey];
  return c?.status === "approved" ? c : undefined;
}

function uniqueAlias(contacts: Contacts, alias: string): string {
  const taken = new Set(Object.values(contacts).map((c) => c.alias));
  let candidate = alias;
  for (let n = 2; taken.has(candidate); n++) candidate = `${alias}-${n}`;
  return candidate;
}

// Paired peers. Only `approved` contacts can reach this agent.
// Statuses: approved, blocked (`pending` is reserved for future contact requests).
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
}

export const RELATIONSHIPS = ["family", "friend", "colleague", "business", "service", "other"];
const MAX_SHARING = 500;

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

/** The current relationship context of the contact with this npub. */
export async function contextOf(st: Storage, npub: string): Promise<{ relationship: string | null; sharing: string | null }> {
  const c = Object.values(await load(st)).find((c) => c.npub === npub);
  return { relationship: c?.relationship ?? null, sharing: c?.sharing ?? null };
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

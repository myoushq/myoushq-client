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

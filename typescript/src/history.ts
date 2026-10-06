// Message history and the "state" document (next seq, read position,
// handled gift wraps). Callers hold the "state" lock around record().

import type { HistoryEntry, Storage } from "./storage.js";

export interface State {
  next_seq?: number;
  read_seq?: number;
  seen?: Record<string, number>;
  registered?: boolean;
  last_poll?: number;
  /** The newest release already announced in the inbox. */
  announced_release?: string;
  /** Ids of hub notices already passed on. */
  seen_notices?: string[];
}

export async function record(st: Storage, entry: Omit<HistoryEntry, "seq" | "at">): Promise<HistoryEntry> {
  const state = await st.get<State>("state", {});
  const full: HistoryEntry = { ...entry, seq: state.next_seq ?? 1, at: Math.floor(Date.now() / 1000) };
  await st.appendHistory(full);
  state.next_seq = full.seq + 1;
  await st.put("state", state);
  return full;
}

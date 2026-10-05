// The hub's HTTPS API: its config and the pairing mailbox.

import type { Storage } from "./storage.js";

export const DEFAULT_HUB = "https://myoushq.com";
const CONFIG_MAX_AGE = 6 * 3600;

export interface HubConfig {
  version: number;
  relays: string[];
  pair_api: string;
  pair_link_base: string;
  pow_difficulty: number;
  url?: string;
  fetched_at?: number;
}

export class HubError extends Error {
  constructor(readonly status: number, readonly reason: string) {
    super(`hub error ${status}: ${reason}`);
  }
}

export class Hub {
  constructor(private st: Storage, readonly url: string) {}

  static async open(st: Storage, url?: string): Promise<Hub> {
    const settings = await st.get<Record<string, string>>("settings", {});
    return new Hub(st, (url ?? settings.hub ?? DEFAULT_HUB).replace(/\/+$/, ""));
  }

  /** Relay list and other settings, cached and refreshed every few hours. */
  async config(refresh = false): Promise<HubConfig> {
    const cached = await this.st.get<HubConfig | null>("hub", null);
    const ours = cached && cached.url === this.url;
    if (ours && !refresh && now() - (cached.fetched_at ?? 0) < CONFIG_MAX_AGE) return cached;
    try {
      const cfg = (await this.request("GET", "/config.json")) as HubConfig;
      const stored = { ...cfg, url: this.url, fetched_at: now() };
      await this.st.put("hub", stored);
      return stored;
    } catch (e) {
      if (ours) return cached; // hub unreachable: keep using what we had
      throw e;
    }
  }

  async request(method: string, endpoint: string, body?: unknown, token?: string): Promise<any> {
    const headers: Record<string, string> = { Accept: "application/json" };
    if (body !== undefined) headers["Content-Type"] = "application/json";
    if (token) headers.Authorization = "Bearer " + token;
    const resp = await fetch(this.url + endpoint, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(40_000),
    });
    const text = await resp.text();
    if (!resp.ok) {
      let reason = resp.statusText;
      try {
        reason = JSON.parse(text).error ?? reason;
      } catch {}
      throw new HubError(resp.status, reason);
    }
    return text ? JSON.parse(text) : null;
  }
}

export function now(): number {
  return Math.floor(Date.now() / 1000);
}

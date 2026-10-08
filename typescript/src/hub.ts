// The hub's HTTPS API: its config and the pairing mailbox.

import { proxyFor, requestVia } from "./proxy.js";
import type { Storage } from "./storage.js";

export const DEFAULT_HUB = "https://myoushq.com";
const CONFIG_MAX_AGE = 6 * 3600;

/** An announcement from the hub, passed on to the agent once. */
export interface Notice {
  id: string;
  text: string;
  url?: string;
  expires?: number;
  min_version?: string;
  max_version?: string;
}

export interface HubConfig {
  version: number;
  relays: string[];
  pair_api: string;
  pair_link_base: string;
  pow_difficulty: number;
  /** The blob store for files (protocol section 6); older hubs omit it. */
  blob_api?: string;
  /** Newest client release the hub announces, e.g. "v0.2.0". */
  latest_release?: string;
  /** Notices for the agent (see Agent.checkNotices). */
  notices?: Notice[];
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
    const url = this.url + endpoint;
    const payload = body === undefined ? undefined : JSON.stringify(body);
    const proxy = proxyFor(url);
    const resp = proxy
      ? await requestVia(proxy, url, { method, headers, body: payload, timeoutMs: 40_000 })
      : await fetch(url, { method, headers, body: payload, signal: AbortSignal.timeout(40_000) })
          .then(async (r) => ({ status: r.status, statusText: r.statusText, text: await r.text() }));
    const text = resp.text;
    if (resp.status < 200 || resp.status > 299) {
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

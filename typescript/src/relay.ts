// Nostr side: registering, publishing and fetching encrypted messages.

import { finalizeEvent, generateSecretKey, getPublicKey, verifyEvent, type Event, type EventTemplate } from "nostr-tools/pure";
import { AbstractRelay } from "nostr-tools/abstract-relay";
import { Relay } from "nostr-tools/relay";
import { minePow } from "nostr-tools/nip13";
import * as nip44 from "nostr-tools/nip44";
import { createRumor, createSeal, unwrapEvent } from "nostr-tools/nip59";
import { proxiedWebSocket, proxyFor } from "./proxy.js";

export const KIND_PROFILE = 0;
export const KIND_CHAT = 14;
export const KIND_GIFT_WRAP = 1059;
export const KIND_INBOX_RELAYS = 10050;
export const MESSAGE_TTL = 86400;

export class RelayError extends Error {}

/** A set of authenticated relay connections. Close it when done. */
export class Connection {
  private constructor(private secretKey: Uint8Array, readonly relays: Relay[]) {}

  static async open(secretKey: Uint8Array, urls: string[]): Promise<Connection> {
    const relays = await Promise.all(urls.map((url) => connectAuthed(secretKey, url)));
    return new Connection(secretKey, relays);
  }

  close(): void {
    for (const r of this.relays) r.close();
  }

  get pubkey(): string {
    return getPublicKey(this.secretKey);
  }

  /** Publish to the given relays (default: all); fails if none accept. */
  async publish(event: Event, relays: Relay[] = this.relays): Promise<void> {
    const results = await Promise.allSettled(relays.map((r) => r.publish(event)));
    if (!results.some((r) => r.status === "fulfilled")) {
      const reasons = results.map((r, i) => `${relays[i].url}: ${(r as PromiseRejectedResult).reason?.message ?? r}`);
      throw new RelayError(`no relay accepted the event (${reasons.join("; ")})`);
    }
  }

  /** Publish our profile. The first one, with proof of work, registers us. */
  async register(alias: string, difficulty: number): Promise<void> {
    const template = {
      kind: KIND_PROFILE,
      content: JSON.stringify({ name: alias, about: "myoushq agent" }),
      tags: [] as string[][],
      created_at: Math.floor(Date.now() / 1000),
      pubkey: this.pubkey,
    };
    const mined = difficulty > 0 ? minePow(template, difficulty) : template;
    await this.publish(finalizeEvent(mined, this.secretKey));
  }

  async publishInboxRelays(urls: string[]): Promise<void> {
    const event = finalizeEvent({
      kind: KIND_INBOX_RELAYS, content: "", created_at: Math.floor(Date.now() / 1000),
      tags: urls.map((u) => ["relay", u]),
    }, this.secretKey);
    await this.publish(event);
  }

  async inboxRelays(pubkey: string): Promise<string[]> {
    const events = await fetchAll(this.relays[0], { kinds: [KIND_INBOX_RELAYS], authors: [pubkey], limit: 1 });
    const newest = events.sort((a, b) => b.created_at - a.created_at)[0];
    return newest ? newest.tags.filter((t) => t[0] === "relay" && t[1]).map((t) => t[1]) : [];
  }

  /** Send a NIP-17 private message to the peer's inbox relays. */
  async sendMessage(recipient: string, text: string): Promise<string> {
    const known = new Set(this.relays.map((r) => r.url));
    const inbox = (await this.inboxRelays(recipient)).map(normalizeUrl).filter((u) => known.has(u));
    const targets = inbox.length ? this.relays.filter((r) => inbox.includes(r.url)) : this.relays;
    // Nostr timestamps are whole seconds; the encrypted "ms" tag keeps
    // messages sent within the same second in order.
    const rumor = createRumor({
      kind: KIND_CHAT, content: text, tags: [["p", recipient], ["ms", String(Date.now())]],
    }, this.secretKey);
    const wrap = giftWrap(createSeal(rumor, this.secretKey, recipient), recipient);
    await this.publish(wrap, targets);
    return wrap.id;
  }

  /** Everything the relays still hold for us. No `since`: wrap timestamps are randomized. */
  async fetchWraps(): Promise<Event[]> {
    const lists = await Promise.all(this.relays.map((r) =>
      fetchAll(r, { kinds: [KIND_GIFT_WRAP], "#p": [this.pubkey], limit: 500 })));
    return lists.flat();
  }

  /** Call onWrap for each gift wrap, stored ones first, until the connection closes. */
  streamWraps(onWrap: (wrap: Event) => void): Promise<string> {
    return new Promise((resolve) => {
      for (const r of this.relays) {
        r.subscribe([{ kinds: [KIND_GIFT_WRAP], "#p": [this.pubkey] }], {
          onevent: onWrap,
          onclose: (reason) => resolve(reason || "closed"),
        });
        const previous = r.onclose;
        r.onclose = () => {
          previous?.();
          resolve("connection closed");
        };
      }
    });
  }
}

/**
 * A NIP-59 gift wrap with the expiration taken from the real current time.
 * nostr-tools' createWrap randomizes created_at (as it should) but has no
 * way to add the expiration tag, so the wrap is built here.
 */
function giftWrap(seal: Event, recipient: string): Event {
  const ephemeral = generateSecretKey();
  const now = Math.floor(Date.now() / 1000);
  const template: EventTemplate = {
    kind: KIND_GIFT_WRAP,
    content: nip44.encrypt(JSON.stringify(seal), nip44.getConversationKey(ephemeral, recipient)),
    created_at: now - Math.floor(Math.random() * 2 * 86400),
    tags: [["p", recipient], ["expiration", String(now + MESSAGE_TTL)]],
  };
  return finalizeEvent(template, ephemeral);
}

/** Returns [sender, text, sent_at, ms] for a valid chat message, else null. */
export function unwrap(secretKey: Uint8Array, wrap: Event): [string, string, number, number] | null {
  let rumor;
  try {
    // Checks the seal's signature and that the rumor's author is the seal's signer.
    rumor = unwrapEvent(wrap, secretKey);
  } catch {
    return null;
  }
  if (rumor.kind !== KIND_CHAT) return null;
  const msTag = rumor.tags.find((t) => t[0] === "ms" && /^\d+$/.test(t[1] ?? ""));
  return [rumor.pubkey, rumor.content, rumor.created_at, msTag ? Number(msTag[1]) : rumor.created_at * 1000];
}

async function connectAuthed(secretKey: Uint8Array, url: string): Promise<Relay> {
  const proxy = proxyFor(url);
  const relay = proxy
    ? (new AbstractRelay(url, { verifyEvent, websocketImplementation: await proxiedWebSocket(proxy, url.startsWith("wss:")) }) as Relay)
    : new Relay(url);
  const sign = async (t: EventTemplate) => finalizeEvent(t, secretKey);
  relay.onauth = sign;
  await relay.connect();
  // The relay challenges on connect; answer before doing anything else.
  for (let i = 0; i < 100 && !(relay as any).challenge; i++) await sleep(50);
  if (!(relay as any).challenge) {
    relay.close();
    throw new RelayError(`${url} didn't send an auth challenge`);
  }
  await relay.auth(sign);
  return relay;
}

function fetchAll(relay: Relay, filter: Record<string, unknown>): Promise<Event[]> {
  return new Promise((resolve, reject) => {
    const events: Event[] = [];
    const sub = relay.subscribe([filter as any], {
      onevent: (e) => events.push(e),
      oneose: () => {
        sub.close();
        resolve(events);
      },
      onclose: (reason) => (reason && reason !== "closed by caller" ? reject(new RelayError(reason)) : resolve(events)),
      eoseTimeout: 15_000,
    });
  });
}

function normalizeUrl(u: string): string {
  return u.replace(/\/+$/, "");
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

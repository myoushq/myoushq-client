// Outbound proxy support, for agents whose network only lets them out
// through an HTTP proxy (common in sandboxes and offices).
//
// The standard variables choose the proxy: HTTPS_PROXY for wss:// and
// https://, HTTP_PROXY for ws:// and http://, ALL_PROXY as a fallback, and
// NO_PROXY for exceptions (lowercase names work too). Node's fetch and
// WebSocket ignore them, so when a proxy applies, hub requests go through
// node:http(s) and relay connections through the `ws` package, both over an
// HTTP CONNECT tunnel. TLS to the hub and relay stays end to end: the proxy
// sees host names, not the traffic.

import http from "node:http";
import https from "node:https";
import net from "node:net";
import tls from "node:tls";

/** The proxy to use for `target`, from the environment, if any. */
export function proxyFor(target: string): URL | undefined {
  const u = new URL(target);
  if (bypassed(u.hostname)) return undefined;
  const secure = u.protocol === "https:" || u.protocol === "wss:";
  const names = secure ? ["HTTPS_PROXY", "https_proxy"] : ["HTTP_PROXY", "http_proxy"];
  const value = [...names, "ALL_PROXY", "all_proxy"].map((n) => process.env[n]).find((v) => v);
  if (!value) return undefined;
  const proxy = new URL(value.includes("://") ? value : "http://" + value);
  if (proxy.protocol !== "http:") throw new Error(`unsupported proxy ${proxy.protocol}//; use http://`);
  return proxy;
}

function bypassed(host: string): boolean {
  const list = process.env.NO_PROXY ?? process.env.no_proxy ?? "";
  host = host.toLowerCase().replace(/^\[|\]$/g, "");
  return list.split(",").map((e) => e.trim().replace(/^\./, "").toLowerCase()).filter(Boolean)
    .some((e) => e === "*" || host === e || host.endsWith("." + e));
}

function authHeader(proxy: URL): Record<string, string> {
  if (!proxy.username) return {};
  const creds = `${decodeURIComponent(proxy.username)}:${decodeURIComponent(proxy.password)}`;
  return { "Proxy-Authorization": "Basic " + Buffer.from(creds).toString("base64") };
}

/** A TCP connection to host:port through the proxy (HTTP CONNECT). */
export function tunnel(proxy: URL, host: string, port: number): Promise<net.Socket> {
  return new Promise((resolve, reject) => {
    const target = host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
    const req = http.request({
      host: proxy.hostname, port: proxy.port || 80, method: "CONNECT", path: target, agent: false,
      headers: { Host: target, ...authHeader(proxy) },
    });
    req.once("connect", (res, socket, head) => {
      if (res.statusCode !== 200) {
        socket.destroy();
        reject(new Error(`proxy refused CONNECT: ${res.statusCode} ${res.statusMessage}`));
        return;
      }
      if (head.length) socket.unshift(head);
      resolve(socket);
    });
    req.once("error", reject);
    req.setTimeout(30_000, () => req.destroy(new Error("proxy timed out")));
    req.end();
  });
}

/** An agent whose connections go through the proxy. */
export function tunnelAgent(proxy: URL, secure: boolean): http.Agent {
  const Base = secure ? https.Agent : http.Agent;
  const agent = new Base({ keepAlive: false });
  (agent as any).createConnection = (opts: any, done: (err: Error | null, s?: net.Socket) => void) => {
    const port = Number(opts.port) || (secure ? 443 : 80);
    tunnel(proxy, opts.host, port).then(
      (socket) => done(null, secure ? tls.connect({ ...opts, socket, servername: opts.servername ?? opts.host }) : socket),
      (err) => done(err),
    );
  };
  return agent;
}

/** A WebSocket class for nostr-tools that connects through the proxy. */
export async function proxiedWebSocket(proxy: URL, secure: boolean): Promise<typeof WebSocket> {
  let WS: any;
  try {
    WS = (await import("ws")).default;
  } catch {
    throw new Error("relay connections through a proxy need the optional `ws` package (npm ci installs it)");
  }
  const agent = tunnelAgent(proxy, secure);
  return class extends WS {
    constructor(url: string, protocols?: string | string[]) {
      super(url, protocols, { agent });
    }
  } as any;
}

/** A plain HTTP(S) request through the proxy; returns status and body as text. */
export async function requestVia(proxy: URL, url: string, init: { method: string; headers: Record<string, string>; body?: string; timeoutMs: number }): Promise<{ status: number; statusText: string; text: string }> {
  const { status, statusText, body } = await requestRawVia(proxy, url, init);
  return { status, statusText, text: body.toString() };
}

/** The same, with the body as bytes (blobs aren't text). */
export function requestRawVia(proxy: URL, url: string, init: { method: string; headers: Record<string, string>; body?: string | Buffer; timeoutMs: number }): Promise<{ status: number; statusText: string; body: Buffer }> {
  const u = new URL(url);
  const secure = u.protocol === "https:";
  return new Promise((resolve, reject) => {
    // https: tunnel with CONNECT. http: ask the proxy for the absolute URL.
    const opts: http.RequestOptions = secure
      ? { host: u.hostname, port: u.port || 443, path: u.pathname + u.search, agent: tunnelAgent(proxy, true) }
      : { host: proxy.hostname, port: proxy.port || 80, path: url, agent: false, headers: { Host: u.host, ...authHeader(proxy) } };
    const req = (secure ? https : http).request({ ...opts, method: init.method, headers: { ...opts.headers, ...init.headers } }, (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, statusText: res.statusMessage ?? "", body: Buffer.concat(chunks) }));
      res.on("error", reject);
    });
    req.on("error", reject);
    req.setTimeout(init.timeoutMs, () => req.destroy(new Error("request timed out")));
    if (init.body !== undefined) req.write(init.body);
    req.end();
  });
}

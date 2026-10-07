#!/usr/bin/env node
// myous command line: the library plus file storage in $MYOUS_HOME
// (default ~/.myous). Covers the shared CLI contract in clients/README.md.

import { writeFile } from "node:fs/promises";
import { parseArgs } from "node:util";
import { Agent, IdentityError } from "./agent.js";
import type { Pending } from "./pairing.js";
import { FileStorage, type HistoryEntry } from "./storage.js";

const USAGE = `usage: myous <command> [options]

  init --alias NAME [--hub URL]     create the identity (once) and register
  invite [--json]                   start a pairing: link, code and QR image
  accept CODE_OR_LINK [--wait S]    join a pairing
  send NAME TEXT... | send NAME -   send a message to a contact (- reads it from stdin)
  poll [--json]                     advance pairings and fetch messages once
  listen                            stay connected and receive live
  inbox [--json] [--peek] [--local] fetch, then new messages and other items
  history [--with NAME] [--json]    past messages
  contacts [--json]                 paired contacts
  block NAME | unblock NAME         drop / accept a contact's messages
  status                            identity, hub and contacts`;

async function main(): Promise<void> {
  const [command, ...rest] = process.argv.slice(2);
  const { values: opts, positionals: args } = parseArgs({
    args: rest,
    allowPositionals: true,
    options: {
      alias: { type: "string" }, hub: { type: "string" }, json: { type: "boolean" },
      wait: { type: "string" }, peek: { type: "boolean" }, local: { type: "boolean" }, with: { type: "string" },
      "qr-out": { type: "string" },
    },
  });
  if (!command || command === "--help" || command === "-h") {
    console.log(USAGE);
    return;
  }
  const st = new FileStorage();
  const agent = await Agent.open(st, opts.hub);

  switch (command) {
    case "init": {
      const settings = await st.get<Record<string, string>>("settings", {});
      const alias = opts.alias ?? settings.alias;
      if (!alias) fail("give this agent a friendly name: myous init --alias NAME");
      const created = !(await agent.hasIdentity());
      if (created) await agent.createIdentity();
      if (!(await agent.isRegistered())) console.log(`registering with ${agent.hub.url} (proof of work, a few seconds)...`);
      await agent.register(alias);
      console.log(`${created ? "created" : "kept existing"} identity ${await agent.npub()}`);
      console.log(`data directory: ${st.home} (keep it; the key file must never be lost)`);
      break;
    }
    case "invite": {
      const inv = await agent.invite();
      let svg: string | null = opts["qr-out"] ?? st.path(`invite-${inv.nameplate}.svg`);
      try {
        // Optional dependency. SVG works wherever images do and needs no image library.
        const QRCode = (await import("qrcode")).default;
        await writeFile(svg, await QRCode.toString(inv.link, { type: "svg", errorCorrectionLevel: "M" }));
      } catch {
        svg = null;
      }
      if (opts.json) {
        console.log(JSON.stringify({ code: inv.code, link: inv.link, nameplate: inv.nameplate, expires_at: inv.expires_at, qr: svg }));
      } else {
        console.log(`pairing link: ${inv.link}\npairing code: ${inv.code}\nQR image:     ${svg ?? "(not available; install the optional qrcode package)"}`);
        console.log("valid for 15 minutes, for one person; it finishes the next time this agent polls or listens");
      }
      break;
    }
    case "accept": {
      if (!args[0]) fail("give the pairing link or code");
      report(await agent.accept(args[0], Number(opts.wait ?? 60)));
      break;
    }
    case "send": {
      if (args.length < 2) fail("usage: myous send NAME TEXT... (or - to read it from stdin)");
      const text = args.length === 2 && args[1] === "-" ? await readStdin() : args.slice(1).join(" ");
      if (!text.trim()) fail("nothing to send");
      const entry = await agent.send(args[0], text);
      console.log(`sent to ${entry.alias}`);
      break;
    }
    case "poll": {
      const entries = await agent.poll();
      console.log(opts.json ? JSON.stringify(entries, null, 2) : `${entries.length} new item(s); read them with \`myous inbox\``);
      break;
    }
    case "listen": {
      for (;;) {
        const reason = await agent.listen((entries) => entries.forEach((e) => console.log(`${e.type}: ${e.alias ?? ""}`)));
        console.error(`relay connection ended (${reason}); reconnecting in 5s`);
        await new Promise((r) => setTimeout(r, 5000));
      }
    }
    case "inbox": {
      if (!opts.local) {
        try {
          await agent.poll();
        } catch (e) {
          console.error(`warning: couldn't fetch new items (${(e as Error).message ?? e}); showing what's stored`);
        }
      }
      const entries = await agent.unread(!opts.peek);
      if (opts.json) console.log(JSON.stringify(entries, null, 2));
      else if (!entries.length) console.log("no new messages");
      else printEntries(entries);
      break;
    }
    case "history": {
      const entries = await agent.history(opts.with);
      if (opts.json) console.log(JSON.stringify(entries, null, 2));
      else printEntries(entries);
      break;
    }
    case "contacts": {
      const all = await agent.contacts();
      if (opts.json) console.log(JSON.stringify(all, null, 2));
      else for (const c of Object.values(all)) console.log(`${c.alias.padEnd(20)} ${c.status.padEnd(9)} ${c.npub}`);
      break;
    }
    case "block":
      console.log(`blocked ${(await agent.block(args[0])).alias}`);
      break;
    case "unblock":
      console.log(`unblocked ${(await agent.unblock(args[0])).alias}`);
      break;
    case "status": {
      const has = await agent.hasIdentity();
      console.log(JSON.stringify({
        data_dir: st.home, hub: agent.hub.url, identity: has ? await agent.npub() : null,
        alias: await agent.alias(), registered: await agent.isRegistered(),
        contacts: Object.keys(await agent.contacts()).length,
        pending_pairings: has ? (await (await agent.pairing()).pending()).map(describePending) : [],
        unread: (await agent.unread(false)).length,
      }, null, 2));
      break;
    }
    default:
      fail(`unknown command ${command}\n\n${USAGE}`);
  }
}

/** What a pairing in progress is waiting for. */
function describePending(p: Pending): string {
  const waiting = p.stage !== "wait_pake" ? "waiting for the other agent's details"
    : p.role === "a" ? "waiting for the other agent to join" : "waiting for the inviting agent to answer";
  const left = Math.max(0, Math.floor((p.expires_at - Date.now() / 1000) / 60));
  return `${p.nameplate}: ${waiting}, expires in ${left} min`;
}

function report(r: Pending): void {
  if (r.stage === "done") {
    console.log(`paired with ${r.contact!.alias} (${r.contact!.npub})`);
    console.log(`verification code: ${r.verify} (both owners should see the same number)`);
  } else if (r.stage === "failed") {
    fail(`pairing failed: ${r.error}`);
  } else if (r.stage === "elsewhere") {
    console.log("the pairing was completed by another run; see `myous inbox`");
  } else {
    console.log("the other agent hasn't answered yet; it finishes the next time this agent polls or listens");
  }
}

function printEntries(entries: HistoryEntry[]): void {
  for (const e of entries) {
    const when = new Date((e.sent_at ?? e.at) * 1000).toISOString().slice(0, 16).replace("T", " ");
    if (e.type !== "message") console.log(`[${when}] (${e.type}) ${e.text}`);
    else if (e.direction === "out") console.log(`[${when}] me -> ${e.alias}: ${e.text}`);
    else console.log(`[${when}] ${e.alias}: ${e.text}`);
  }
}

function fail(message: string): never {
  console.error(message);
  process.exit(1);
}

main().catch((e) => {
  fail(e instanceof IdentityError ? e.message : `error: ${e?.message ?? e}`);
});

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of process.stdin) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString("utf8");
}

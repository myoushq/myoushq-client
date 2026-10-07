"""myous command line: the library plus file storage in $MYOUS_HOME
(default ~/.myous). Run `myous --help` for the commands."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time

from myous import qr, vm
from myous.agent import Agent, IdentityError
from myous.hub import DEFAULT_HUB, HubError
from myous.pairing import PairingError
from myous.relay import RelayError
from myous.storage import FileStorage


def cmd_init(agent: Agent, st: FileStorage, args) -> None:
    alias = args.alias or st.get("settings", {}).get("alias")
    if not alias:
        sys.exit("give this agent a friendly name: myous init --alias NAME")
    created = not agent.has_identity()
    if created:
        agent.create_identity()
    if not agent.is_registered():
        print(f"registering with {agent.hub.url} (proof of work, a few seconds)...", flush=True)
    asyncio.run(agent.register(alias))
    print(("created" if created else "kept existing") + f" identity {agent.keys.public_key().to_bech32()}")
    print(f"alias: {alias}")
    print(f"data directory: {st.home} (keep it; the key file must never be lost)")


def cmd_invite(agent: Agent, st: FileStorage, args) -> None:
    inv = agent.invite(args.relationship, args.sharing)
    svg = args.qr_out or str(st.path(f"invite-{inv['nameplate']}.svg"))
    try:
        qr.write_svg(inv["link"], svg)
    except ImportError:
        svg = None  # QR support not installed: pip install 'myous[qr]'

    if args.json:
        print(json.dumps({"code": inv["code"], "link": inv["link"], "nameplate": inv["nameplate"],
                          "expires_at": inv["expires_at"], "qr": svg}))
        return
    minutes = max(1, int((inv["expires_at"] - time.time()) // 60))
    print(f"pairing link: {inv['link']}")
    print(f"pairing code: {inv['code']}")
    print(f"QR image:     {svg or '(not available; install myous[qr])'}")
    if args.qr_text and svg:
        print(qr.as_text(inv["link"]))
    print(f"valid for {minutes} minutes, for one person")
    if not args.wait:
        print("it finishes the next time this agent polls or listens; the result appears in `myous inbox`")
        return
    print("waiting for the other side...", flush=True)
    while True:
        r = agent.pairing.advance(inv, wait=25)
        if r["stage"] in ("done", "failed", "elsewhere"):
            _report_pairing(r)
            return


def cmd_accept(agent: Agent, st: FileStorage, args) -> None:
    code = args.code
    if os.path.isfile(code):
        code = qr.read_image(code)
    r = agent.accept(code, wait=args.wait, relationship=args.relationship, sharing=args.sharing)
    if r["stage"] not in ("done", "failed", "elsewhere"):
        print("the other agent hasn't answered yet; it finishes the next time this agent polls "
              "or listens (result in `myous inbox`)")
        return
    _report_pairing(r)


def _report_pairing(r: dict) -> None:
    if r["stage"] == "done":
        c = r["contact"]
        print(f"paired with {c['alias']} ({c['npub']})")
        print(f"verification code: {r['verify']} (both owners should see the same number)")
    elif r["stage"] == "elsewhere":
        print("the pairing was completed by another run; see `myous inbox`")
    else:
        sys.exit(f"pairing failed: {r['error']}")


def cmd_send(agent: Agent, st: FileStorage, args) -> None:
    text = sys.stdin.read() if args.text == ["-"] else " ".join(args.text)
    if not text.strip():
        sys.exit("nothing to send")
    entry = asyncio.run(agent.send(args.to, text))
    print(f"sent to {entry['alias']}")


def context_line(e: dict) -> str | None:
    """How the owner knows the sender and what may be shared, for an incoming message."""
    if e.get("type") != "message" or e.get("direction") != "in" or "relationship" not in e:
        return None
    if not e.get("relationship") and not e.get("sharing"):
        return (f"    (relationship not set: until your owner tells you, share nothing personal; record it with "
                f"myous context \"{e['alias']}\" --relationship ... --sharing \"...\")")
    parts = [e.get("relationship") or "relationship not set"]
    if e.get("sharing"):
        parts.append(f"may share: {e['sharing']}")
    return f"    ({'; '.join(parts)})"


def _print_entries(entries: list[dict]) -> None:
    for e in entries:
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(e.get("sent_at", e["at"])))
        if e["type"] != "message":
            print(f"[{when}] ({e['type']}) {e['text']}")
        elif e["direction"] == "out":
            print(f"[{when}] me -> {e['alias']}: {e['text']}")
        else:
            print(f"[{when}] {e['alias']}: {e['text']}")
            line = context_line(e)
            if line:
                print(line)


def cmd_inbox(agent: Agent, st: FileStorage, args) -> None:
    if not args.local:
        try:
            asyncio.run(agent.poll())
        except (HubError, RelayError, OSError) as e:
            print(f"warning: couldn't fetch new items ({e}); showing what's stored", file=sys.stderr)
    entries = agent.unread(mark_read=not args.peek)
    if args.json:
        print(json.dumps(entries, indent=2))
    elif not entries:
        print("no new messages")
    else:
        _print_entries(entries)


def cmd_history(agent: Agent, st: FileStorage, args) -> None:
    entries = agent.history(args.with_, args.limit)
    if args.json:
        print(json.dumps(entries, indent=2))
    else:
        _print_entries(entries)


def cmd_contacts(agent: Agent, st: FileStorage, args) -> None:
    all_contacts = agent.contacts()
    if args.json:
        print(json.dumps(all_contacts, indent=2))
        return
    if not all_contacts:
        print("no contacts yet; pair with `myous invite` or `myous accept`")
    for c in all_contacts.values():
        print(f"{c['alias']:<20} {c['status']:<9} {c.get('relationship') or '-':<10} {c['npub']}")


def cmd_context(agent: Agent, st: FileStorage, args) -> None:
    c = agent.set_context(args.name, args.relationship, args.sharing)
    if args.json:
        print(json.dumps({"alias": c["alias"], "relationship": c.get("relationship"), "sharing": c.get("sharing")}))
        return
    print(f"{c['alias']}: relationship {c.get('relationship') or '(not set)'}; "
          f"may share: {c.get('sharing') or '(not set: share nothing personal)'}")


def cmd_block(agent: Agent, st: FileStorage, args) -> None:
    print(f"blocked {agent.block(args.name)['alias']}; their messages will be dropped")


def cmd_unblock(agent: Agent, st: FileStorage, args) -> None:
    print(f"unblocked {agent.unblock(args.name)['alias']}")


def cmd_rename(agent: Agent, st: FileStorage, args) -> None:
    print(f"renamed to {agent.rename(args.name, args.new_alias)['alias']}")


def cmd_poll(agent: Agent, st: FileStorage, args) -> None:
    entries = asyncio.run(agent.poll())
    vm.run_hook(st, entries)
    if args.json:
        print(json.dumps(entries, indent=2))
    elif not args.quiet:
        print(f"{len(entries)} new item(s); read them with `myous inbox`")


def cmd_listen(agent: Agent, st: FileStorage, args) -> None:
    asyncio.run(vm.run_listener(agent, st))


def cmd_ensure(agent: Agent, st: FileStorage, args) -> None:
    """Health check, safe to run as often as you like."""
    if not agent.is_registered():
        sys.exit("not registered with the hub yet; run `myous init --alias NAME`")
    vm.trim_logs(st)
    notes = []
    healthy = vm.listener_healthy(st)
    if healthy:
        notes.append("listener running")
    elif not args.no_listener:
        vm.start_listener(st)
        notes.append("listener started")
    last_poll = st.get("state", {}).get("last_poll", 0)
    if not healthy or time.time() - last_poll > vm.SAFETY_POLL_EVERY:
        entries = asyncio.run(agent.poll())
        vm.run_hook(st, entries)
        notes.append(f"polled, {len(entries)} new item(s)")
    if not args.quiet:
        print("; ".join(notes))


def cmd_cron(agent: Agent, st: FileStorage, args) -> None:
    if args.action == "install":
        print(vm.install_cron(st))
    elif args.action == "remove":
        print(vm.remove_cron())
    else:
        print(vm.show_cron())


def cmd_hook(agent: Agent, st: FileStorage, args) -> None:
    settings = st.get("settings", {})
    if args.action == "set":
        if not args.command:
            sys.exit("give the command to run, e.g. myous hook set 'my-wake-command'")
        settings["on_message"] = args.command
    elif args.action == "clear":
        settings.pop("on_message", None)
    if args.action != "show":
        st.put("settings", settings)
    print(f"on new items, run: {settings.get('on_message') or '(nothing)'}")


def cmd_status(agent: Agent, st: FileStorage, args) -> None:
    info = {
        "data_dir": str(st.home),
        "hub": agent.hub.url,
        "identity": agent.keys.public_key().to_bech32() if agent.has_identity() else None,
        "alias": st.get("settings", {}).get("alias"),
        "registered": agent.is_registered(),
        "contacts": len(agent.contacts()),
        "pending_pairings": _pending_summary(agent) if agent.has_identity() else [],
        "listener_running": vm.listener_healthy(st),
        "hook": st.get("settings", {}).get("on_message"),
        "unread": len(agent.unread(mark_read=False)),
    }
    if args.json:
        print(json.dumps(info, indent=2))
    else:
        for k, v in info.items():
            print(f"{k:<17} {v}")


def _pending_summary(agent: Agent) -> list[str]:
    """What each pairing in progress is waiting for."""
    out = []
    for p in agent.pairing.pending():
        left = max(0, int((p["expires_at"] - time.time()) // 60))
        if p["stage"] == "wait_pake":
            waiting = "waiting for the other agent to join" if p["role"] == "a" else "waiting for the inviting agent to answer"
        else:
            waiting = "waiting for the other agent's details"
        out.append(f"{p['nameplate']}: {waiting}, expires in {left} min")
    return out


def _context_options(p: argparse.ArgumentParser) -> None:
    from myous.contacts import RELATIONSHIPS
    p.add_argument("--relationship", choices=RELATIONSHIPS, help="how your owner knows this contact")
    p.add_argument("--sharing", metavar="TEXT", help="your owner's guidance on what you may share with this contact")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="myous", description="Encrypted messaging between paired AI agents.")
    sub = parser.add_subparsers(dest="command", required=True)

    def add(name, func, help_text):
        p = sub.add_parser(name, help=help_text)
        p.set_defaults(func=func)
        return p

    p = add("init", cmd_init, "create this agent's identity (once) and register with the hub")
    p.add_argument("--alias", help="friendly name shown to peers")
    p.add_argument("--hub", help=f"hub URL (default {DEFAULT_HUB})")

    p = add("invite", cmd_invite, "start a pairing: get a QR code, link and code to share")
    p.add_argument("--qr-out", metavar="SVG", help="where to write the QR image")
    p.add_argument("--qr-text", action="store_true", help="also print the QR code as text")
    p.add_argument("--wait", action="store_true", help="stay until the other side joins")
    p.add_argument("--json", action="store_true", help="print code, link and QR path as JSON")
    _context_options(p)

    p = add("accept", cmd_accept, "join a pairing from a link, code, or photo of the QR code")
    p.add_argument("code", help="pairing link, code like 4821-K7F3QX, or path to a QR image")
    p.add_argument("--wait", type=float, default=60, help="seconds to wait for the other side (default 60)")
    _context_options(p)

    p = add("send", cmd_send, "send a message to a paired contact")
    p.add_argument("to", help="contact alias or npub")
    p.add_argument("text", nargs="+", help="message text, or - to read it from stdin")

    p = add("inbox", cmd_inbox, "fetch, then show new messages and other items, and mark them read")
    p.add_argument("--peek", action="store_true", help="don't mark them read")
    p.add_argument("--local", action="store_true", help="don't fetch; show only what's already stored")
    p.add_argument("--json", action="store_true")

    p = add("history", cmd_history, "show past messages, both directions")
    p.add_argument("--with", dest="with_", metavar="CONTACT")
    p.add_argument("--limit", type=int, default=50)
    p.add_argument("--json", action="store_true")

    p = add("contacts", cmd_contacts, "list paired contacts")
    p.add_argument("--json", action="store_true")

    p = add("context", cmd_context, "show or set how your owner knows a contact and what you may share with it")
    p.add_argument("name")
    _context_options(p)
    p.add_argument("--json", action="store_true")

    add("block", cmd_block, "drop all messages from a contact").add_argument("name")
    add("unblock", cmd_unblock, "accept messages from a contact again").add_argument("name")
    p = add("rename", cmd_rename, "change a contact's alias")
    p.add_argument("name")
    p.add_argument("new_alias")

    p = add("poll", cmd_poll, "advance pairings and fetch waiting messages, once")
    p.add_argument("--quiet", action="store_true")
    p.add_argument("--json", action="store_true", help="print the new items as JSON")

    add("listen", cmd_listen, "stay connected and receive messages live")

    p = add("ensure", cmd_ensure, "restart the listener if it died; poll if it's down (safe to repeat)")
    p.add_argument("--quiet", action="store_true")
    p.add_argument("--no-listener", action="store_true", help="never start the listener")

    p = add("cron", cmd_cron, "optional: a crontab entry that runs `ensure` every minute")
    p.add_argument("action", choices=["install", "remove", "show"])

    p = add("hook", cmd_hook, "optional: run a command whenever new messages or pairing results arrive")
    p.add_argument("action", choices=["set", "clear", "show"])
    p.add_argument("command", nargs="?", help="shell command; gets MYOUS_NEW (count) in its environment")

    p = add("status", cmd_status, "show identity, hub, contacts and listener state")
    p.add_argument("--json", action="store_true")
    return parser


def main() -> None:
    args = build_parser().parse_args()
    st = FileStorage()
    agent = Agent(st, hub_url=getattr(args, "hub", None))
    try:
        args.func(agent, st, args)
    except IdentityError as e:
        sys.exit(str(e))
    except (HubError, RelayError, PairingError, KeyError, ValueError) as e:
        sys.exit(f"error: {e.args[0] if isinstance(e, KeyError) else e}")
    except OSError as e:
        sys.exit(f"error: network problem ({e}); try again. Behind a proxy? Set HTTPS_PROXY.")


if __name__ == "__main__":
    main()

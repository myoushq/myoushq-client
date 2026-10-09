"""myous command line: the library plus file storage in $MYOUS_HOME
(default ~/.myous). Run `myous --help` for the commands."""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time

from myous import __version__, contacts, qr, vm
from myous.agent import Agent, IdentityError, WorkerError
from myous.files import BlobError
from myous.hub import DEFAULT_HUB, HubError
from myous.pairing import PairingError
from myous.relay import RelayError
from myous.storage import FileStorage


def cmd_init(agent: Agent, st: FileStorage, args) -> None:
    alias = args.alias or st.get("settings", {}).get("alias")
    if not alias:
        sys.exit("give this agent a friendly name: myous init --alias NAME")
    created = not agent.has_identity()
    stored = st.get("settings", {}).get("alias")
    if not created and stored and stored != alias and not args.rename:
        sys.exit(f"this directory belongs to {stored}; use another MYOUS_HOME, or pass --rename if this is the same agent")
    if created:
        agent.create_identity()
    if not agent.is_registered():
        print(f"registering with {agent.hub.url} (proof of work, a few seconds)...", flush=True)
    asyncio.run(agent.register(alias))
    print(("created" if created else "kept existing") + f" identity {agent.keys.public_key().to_bech32()}")
    print(f"alias: {alias}")
    print(f"data directory: {st.home} (keep it; the key file must never be lost)")


def cmd_invite(agent: Agent, st: FileStorage, args) -> None:
    inv = agent.invite(args.relationship, args.sharing, args.added_by)
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
    r = agent.accept(code, wait=args.wait, relationship=args.relationship, sharing=args.sharing, added_by=args.added_by)
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
        if e["type"] == "file":
            who = f"me -> {e['alias']}" if e.get("direction") == "out" else f"from {e['alias']}"
            print(f"[{when}] file {who}: {e['name']} ({e.get('size')} bytes, id {e['seq']}; `myous fetch {e['seq']}`)")
        elif e["type"] != "message":
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
        print(json.dumps([_public(e) for e in entries], indent=2))
    elif not entries:
        print("no new messages")
    else:
        _print_entries(entries)


def _public(e: dict) -> dict:
    """An entry without the file key: that stays in the history only."""
    return {k: v for k, v in e.items() if k not in ("key", "nonce")}


def cmd_history(agent: Agent, st: FileStorage, args) -> None:
    entries = agent.history(args.with_, args.limit)
    if args.json:
        print(json.dumps([_public(e) for e in entries], indent=2))
    else:
        _print_entries(entries)


def cmd_send_file(agent: Agent, st: FileStorage, args) -> None:
    if not os.path.isfile(args.path):
        sys.exit(f"no such file: {args.path}")
    entry = asyncio.run(agent.send_file(args.to, args.path, mime=args.mime))
    print(f"sent {entry['name']} ({entry['size']} bytes) to {entry['alias']}; the blob expires in a day")


def cmd_fetch(agent: Agent, st: FileStorage, args) -> None:
    which = None if args.latest or args.id is None else args.id
    path = agent.fetch(which, args.to)
    print(path)


def _remote(spec: str, agent: Agent) -> tuple[str, str] | None:
    """(contact, path) if `spec` is CONTACT:PATH naming a contact, else None."""
    name, sep, path = spec.partition(":")
    if not sep or not name or "/" in name or os.path.exists(spec):
        return None
    try:
        contacts.find(agent.st, name)
    except KeyError:
        return None
    return name, path


def cmd_cp(agent: Agent, st: FileStorage, args) -> None:
    src, dst = _remote(args.src, agent), _remote(args.dst, agent)
    if bool(src) == bool(dst):
        sys.exit("exactly one side must be CONTACT:PATH (a paired contact), e.g. "
                 "myous cp report.pdf worker:in.pdf  or  myous cp worker:out.png out.png")
    if dst:
        if not os.path.isfile(args.src):
            sys.exit(f"no such file: {args.src}")
        ack = asyncio.run(agent.put(dst[0], args.src, dst[1] or os.path.basename(args.src), timeout=args.timeout))
        print(f"written on {dst[0]}: {ack['path']} ({ack['size']} bytes)")
    else:
        to = args.dst if args.dst not in (".", "") else "./"
        path = asyncio.run(agent.get(src[0], src[1], to, timeout=args.timeout))
        print(path)


def cmd_exec(agent: Agent, st: FileStorage, args) -> None:
    cmd = " ".join(args.cmd[1:] if args.cmd[:1] == ["--"] else args.cmd)
    if not cmd.strip():
        sys.exit("give the command to run: myous exec CONTACT -- CMD...")
    try:
        result = asyncio.run(agent.exec(args.contact, cmd, timeout=args.timeout))
    except TimeoutError as e:
        sys.stderr.write(f"error: {e}\n")
        sys.exit(124)
    sys.stdout.write(result.get("stdout", ""))
    sys.stdout.flush()
    sys.stderr.write(result.get("stderr", ""))
    sys.stderr.flush()
    sys.exit(result.get("exit", 1) if result.get("exit", 1) >= 0 else 1)


def cmd_worker(agent: Agent, st: FileStorage, args) -> None:
    from myous.worker import ABOUT, Worker
    alias = args.alias or st.get("settings", {}).get("alias")
    if not agent.has_identity():
        if not alias:
            sys.exit("give this worker a name the first time: myous worker --alias NAME")
        agent.create_identity()
    if not agent.is_registered() or st.get("settings", {}).get("about") != ABOUT or args.alias:
        print(f"registering with {agent.hub.url}...", flush=True)
        asyncio.run(agent.register(alias, about=ABOUT))
    print(f"worker {agent.alias} ({agent.keys.public_key().to_bech32()}), work directory {args.work}", flush=True)
    worker = Worker(agent, st, args.work, review_cmd=args.review, pause_file=args.pause_file,
                    allow_absolute=args.allow_absolute, notes=args.note)
    try:
        asyncio.run(worker.run())
    except KeyboardInterrupt:
        pass


def cmd_contacts(agent: Agent, st: FileStorage, args) -> None:
    all_contacts = agent.contacts()
    if args.json:
        print(json.dumps(all_contacts, indent=2))
        return
    if not all_contacts:
        print("no contacts yet; pair with `myous invite` or `myous accept`")
    for c in all_contacts.values():
        print(f"{c['alias']:<20} {c['status']:<9} {c.get('relationship') or '-':<10} {c.get('added_by') or '-':<7} {c['npub']}")


def cmd_context(agent: Agent, st: FileStorage, args) -> None:
    c = agent.set_context(args.name, args.relationship, args.sharing, args.added_by)
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


def cmd_watcher(agent: Agent, st: FileStorage, args) -> None:
    from myous.muse import watcher
    sys.exit(watcher.run(agent, st, args.takeover, args.seconds, args.retry))


def cmd_check(agent: Agent, st: FileStorage, args) -> None:
    from myous.muse import check
    sys.exit(check.run(agent, st))


def cmd_hook_script(agent: Agent, st: FileStorage, args) -> None:
    from myous.muse import hook_script
    print(hook_script())


def cmd_status(agent: Agent, st: FileStorage, args) -> None:
    all_contacts = agent.contacts()
    info = {
        "data_dir": str(st.home),
        "client": "python",
        "version": __version__,
        "hub": agent.hub.url,
        "identity": agent.keys.public_key().to_bech32() if agent.has_identity() else None,
        "alias": st.get("settings", {}).get("alias"),
        "registered": agent.is_registered(),
        "contacts": len(all_contacts),
        "contact_list": [{k: c[k] for k in ("alias", "npub", "status", "paired_at", "relationship", "sharing", "added_by")
                          if c.get(k) is not None} for c in all_contacts.values()],
        "pending_pairings": _pending_summary(agent) if agent.has_identity() else [],
        "listener_running": vm.listener_healthy(st),
        "hook": st.get("settings", {}).get("on_message"),
        "unread": len(agent.unread(mark_read=False)),
        "last_used": st.last_used(),
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
    p.add_argument("--added-by", metavar="WHO", help="who made this pairing, when not you: 'owner' (through the myous desktop app)")


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
    p.add_argument("--rename", action="store_true", help="rename this agent (the directory belongs to another alias)")

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

    p = add("send-file", cmd_send_file, "send a file to a paired contact (encrypted; the hub keeps the blob a day)")
    p.add_argument("to", help="contact alias or npub")
    p.add_argument("path")
    p.add_argument("--mime", help="the file's MIME type (default application/octet-stream)")

    p = add("fetch", cmd_fetch, "download and decrypt a received file (default: the latest)")
    p.add_argument("id", nargs="?", help="the file entry's id, as shown by inbox")
    p.add_argument("--latest", action="store_true")
    p.add_argument("--to", help="directory (default $MYOUS_HOME/files) or exact file path")

    p = add("cp", cmd_cp, "copy a file to or from a worker, like scp: CONTACT:PATH on one side")
    p.add_argument("src")
    p.add_argument("dst")
    p.add_argument("--timeout", type=float, default=300, help="seconds to wait for the worker (default 300)")

    p = add("exec", cmd_exec, "run a command on a worker, like ssh: myous exec CONTACT -- CMD...")
    p.add_argument("contact")
    p.add_argument("cmd", nargs=argparse.REMAINDER)
    p.add_argument("--timeout", type=float, default=120, help="seconds the command may run (default 120, max 600)")

    p = add("worker", cmd_worker, "be a worker: run commands and move files for paired contacts")
    p.add_argument("--alias", help="the worker's name (needed the first time)")
    p.add_argument("--work", default=os.environ.get("MYOUS_WORK", "work"), help="work directory (default ./work or $MYOUS_WORK)")
    p.add_argument("--review", metavar="CMD", help="review hook: a command that reads the request as JSON and exits 0 to allow")
    p.add_argument("--pause-file", help="refuse requests while this file exists (default $MYOUS_HOME/worker.paused)")
    p.add_argument("--allow-absolute", action="store_true", help="allow absolute paths in cp requests")
    p.add_argument("--note", action="append", default=[], help="a line to add to the worker's help text (repeatable)")
    p.add_argument("--hub", help=f"hub URL (default {DEFAULT_HUB})")

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

    # For Meta's Muse (docs/muse.md): a one-shot watcher, the scheduled
    # check, and the hook script that runs the watcher every minute.
    p = add("watcher", cmd_watcher, "Muse: wait for new items once, then exit so the caller wakes you")
    from myous.muse.watcher import add_arguments
    add_arguments(p)
    add("check", cmd_check, "Muse: poll once and say what needs doing (scheduled backstop)")
    add("hook-script", cmd_hook_script, "Muse: print the path of the hook script to register")
    return parser


def main() -> None:
    args = build_parser().parse_args()
    st = FileStorage()
    agent = Agent(st, hub_url=getattr(args, "hub", None))
    try:
        args.func(agent, st, args)
    except IdentityError as e:
        sys.exit(str(e))
    except (HubError, BlobError, RelayError, PairingError, WorkerError, TimeoutError, KeyError, ValueError) as e:
        sys.exit(f"error: {e.args[0] if isinstance(e, KeyError) else e}")
    except OSError as e:
        sys.exit(f"error: network problem ({e}); try again. Behind a proxy? Set HTTPS_PROXY.")


if __name__ == "__main__":
    main()

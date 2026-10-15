# myoushq worker

A worker is an agent that runs what your other agents ask: commands,
file transfers, and scripts that drive a browser you've logged into. Pair
it with your agent (Muse, Claude Code, ...) and that agent can reach your
machine through myoushq, end-to-end encrypted, with no inbound ports.
Protocol: [protocol.md](https://myoushq.com/protocol.md), section 7.
Install guide for an agent setting it up: [docs/worker.md](../docs/worker.md).

What's here:

| File | Role |
|---|---|
| `Dockerfile`, `compose.yml` | the container: client, Chromium, virtual display, VNC view |
| `entrypoint.sh` | starts the display, the browser, then `myous worker` |
| `browser.py` | keeps a Chromium with a persistent profile running, reachable on port 9222 |
| `forward.py` | with the browser on the host (`MYOUS_BROWSER=host`), relays port 9222 in the container to it |
| `review.py` | the review hook: honours the pause file, then the review level in `review.json` (`trust`: everything runs; `changes`: reads run, the rest asks the owner; `all`: everything asks) by writing a question to `approvals/` and waiting for the answer the desktop app writes |
| `desktop/` | myous for Mac, Linux and Windows, the owner's app (see `desktop/README.md`) |
| `mac/` | the earlier Objective-C myous for Mac, superseded by `desktop/` and no longer released (`mac/README.md`); its `notarize.sh`, `make-dmg.sh` and `compose-image.yml` are still used |
| `build-worker.sh` | builds the Go worker the myous apps carry (`myous-worker`) for running without a container |

## Docker mode (recommended)

```sh
docker compose up -d --build     # in this directory; the first build downloads Playwright's image (large)
docker compose logs -f           # watch it start; Ctrl-C leaves it running
```

Then:

- **Log into sites** the worker should use: open
  http://localhost:PORT/?autoconnect=1&reconnect=1&resize=remote, where
  PORT is what `docker compose port worker 6080` prints (Docker picks a
  free one at each start; the apps' "Open browser" finds it for you).
  That address connects on its own and fills the tab; the bare
  http://localhost:PORT shows noVNC's Connect button first. Logins
  persist across restarts (the `browser-profile` volume).
- **Pair it** with your agent: the pairing code is in
  `~/.myous-worker/worker.json` (`invite.code`) and in the logs. Tell
  your other agent to accept it, e.g. "accept 4821-K7F3QX; it's my
  desktop worker, run commands there for me". A new code appears whenever
  the worker has no contacts yet.
- **Check it's running:** `docker compose ps`, or `cat ~/.myous-worker/worker.json`.
- **Pause** (refuse every request until you say otherwise):
  `touch ~/.myous-worker/worker.paused`; remove the file to resume.
- **See what it did:** `~/.myous-worker/worker.log` (every request,
  allowed, refused or expired) and `docker compose logs`.
- **Another alias** than "Desktop worker": `MYOUS_ALIAS="Max's Mac" docker compose up -d`.

Where things are:

| What | Where |
|---|---|
| identity, contacts, status (`worker.json`), log, pause file | `~/.myous-worker` on the host, `/home/worker/.myous` inside |
| browser profile (logins) | Docker volume `browser-profile` |
| work directory (files sent to the worker, outputs) | Docker volume `work`, `/work` inside |
| ports | noVNC (the browser view) on a free port Docker picks at each start, 127.0.0.1 only (`docker compose port worker 6080`); 9222 (Chromium CDP) inside the container only |

Commands run inside the container as user `worker`, in `/work`. Nothing
from the host is mounted except `~/.myous-worker`.

## How it works underneath

These are the details the apps rely on; an agent setting up a worker
doesn't need them (`docs/worker.md` is its guide).

**The browser view and the clipboard.** `entrypoint.sh` runs TigerVNC's
`Xvnc` (it accepts the viewer's size, so `resize=remote` fills the tab at
native resolution), matchbox to keep Chromium maximized, and noVNC through
websockify on port 6080, with noVNC's client as the index page. It adds
two scripts to the page: `title.js` (the tab's title, "myous - <paired
agent>") and `clip.js`, which translates the host's shortcuts for the
worker's Linux Chromium (Cmd on a Mac, Ctrl on Windows and Linux): Cmd+V
or Ctrl+V pastes the host's clipboard into the worker's browser, Cmd+C or
Ctrl+C copies from it to the host (text only; files go through
`myous cp`), and the same goes for Cmd+A, Cmd+Z and the other editing
shortcuts. Shortcuts the host browser keeps for itself (new tab, close
tab, quit) stay with it.

**The browser on the host.** `MYOUS_BROWSER=host` makes the entrypoint
skip the container's browser and run `forward.py`, which relays
`127.0.0.1:9222` inside the container to the port named in
`~/.myous-worker/browser.json` (`{"host": "host.docker.internal",
"port": N}`), written by whatever runs the browser on the host. myous
for Mac does this when Set up's "Where should its browser run?" (or
Settings) says on this Mac: a real Chrome, Edge, Brave or Chromium with
the worker's own profile, sandboxed to its folder (`desktop/README.md`,
"On a Mac"). Scripts connect to `http://localhost:9222` either way, and
there is no browser view: the owner logs in in that browser's own
window. Either way the browser is started as a plain process, never
through Playwright: a Playwright-launched browser carries the automation
mark (`navigator.webdriver` is true on every page) for as long as
Playwright is attached, and sites that screen for bots refuse it even
while the owner drives it by hand. A script's `connect_over_cdp` does
not set the mark. `MYOUS_TZ` and `MYOUS_LANG` (e.g. `America/Chicago`,
`en-US`) give the container's browser the owner's clock and language.

**Command files.** Files dropped in `~/.myous-worker/commands/` are
picked up by the worker: `new-code` (a fresh pairing code while
unpaired), `unpair` (drop every contact and offer a code),
`stop-<request id>` (end the command running under that id; its result
says `stopped by the owner`) and `quit` (stop the worker as SIGTERM
does: the command running is ended, its agent told, and the worker
exits). The app uses all four: it writes `quit` whenever it stops a
worker without a container, on every system (on Windows the only way to
stop one gracefully). Request ids name these files, so the worker
refuses a request whose id isn't 1 to 64 characters from A-Z, a-z,
0-9, - and _ (protocol.md, 7.1).

**Stopping the worker.** On SIGTERM or SIGINT (`docker compose down`,
the app's Stop for a worker in a container) or a `quit` file, the worker
ends the command it is running with everything it started, answers that
request "stopped by the owner (the worker is shutting down)", gives the
requests waiting behind it the same answer without running them (their
records say `stopped`), and exits. For a worker without a container the
app writes the `quit` file on every system and, outside Windows, sends
SIGTERM too; it gives the worker ten seconds, then kills it.

**Questions to the owner.** At the `changes` and `all` review levels, a
request that needs the owner is written to
`~/.myous-worker/approvals/<id>.json`, and the worker waits up to 120
seconds for `<id>.answer` containing `allow` or `refuse`; the apps show
the question (as a notification and in the window) and write the answer.
No answer in time refuses the request with a reason the agent sees.

## Direct mode (adventurous)

Run the same worker on your machine, no Docker. Then **`exec` runs
commands on your computer, as you**, with whatever your account can
reach: only do this for agents you'd give a terminal to, and keep the
review hook strict.

```sh
python3 -m venv ~/.myous-worker/venv
~/.myous-worker/venv/bin/pip install --require-hashes -r python/requirements.lock
~/.myous-worker/venv/bin/pip install --no-deps ./python
~/.myous-worker/venv/bin/pip install playwright==1.63.0             # the version the container has
~/.myous-worker/venv/bin/playwright install chromium
MYOUS_HOME=~/.myous-worker ~/.myous-worker/venv/bin/python worker/browser.py &   # a Chromium window appears; log in there
MYOUS_HOME=~/.myous-worker ~/.myous-worker/venv/bin/myous worker --alias "Max's Mac" --work ~/myous-work --review worker/review.py
```

`--alias` is needed the first time only: that run creates the worker's
identity and registers it. Without an app, nothing shows you the
review's questions (levels `changes` and `all`, `review.json` in
`~/.myous-worker`): answer one by writing `allow` or `refuse` into
`~/.myous-worker/approvals/<id>.answer`, next to the request in
`<id>.json`, within 120 seconds; unanswered, it is refused. Or keep the
level at `trust` and pause the worker (`touch
~/.myous-worker/worker.paused`) whenever it shouldn't run anything.

On Windows, use `venv\Scripts\` instead of `venv/bin/`; commands run in
PowerShell. The myous apps run a worker without a container themselves,
with the Go worker below inside them (`desktop/README.md`, "Without
containers").

Without Python, the Go client's worker does the same job from one
binary (`go/README.md`): `MYOUS_HOME=~/.myous-worker myous worker --alias
"Max's Mac" --work ~/myous-work`. With no `--review` it applies
`review.py`'s rules itself (the pause file, `review.json`'s level and the
`approvals/` questions); `--review CMD` runs a hook of your own instead.
The browser still needs Python and Playwright (`browser.py`).

Scripts attach to the browser the same way as in the container
(`connect_over_cdp("http://localhost:9222")`). To keep it running after
you log out, wrap the last two commands in a launchd agent, or use the
app (`desktop/`).

## Security

- Only approved contacts' messages reach the worker at all; everything
  else is dropped before the hook sees it. The hook (`review.py`) is the
  control point for what those contacts may do: at the default level,
  `trust`, it allows everything and logs it; `changes` and `all` ask the
  owner.
- Your agent can be talked into things by messages from *its* other
  contacts, so the worker treats your own agent's requests as untrusted
  input (protocol.md section 8). Tighten `review.py` as soon as the worker
  does anything that matters. Keep the hook outside the work directory
  (in Docker it's `/opt/worker/review.py`); a requester could otherwise
  replace it with a `put`, and the worker refuses to start if it's there.
- The browser holds your logins. In Docker mode they stay in the
  container's volume; the noVNC page has no password because it's
  reachable only from this machine. Don't publish the browser view beyond
  127.0.0.1.
- Chromium runs with `--no-sandbox` inside the container (its own sandbox
  needs privileges the container doesn't get); the container is the
  sandbox, with all capabilities dropped and no privilege gain.
- Files arrive and leave end-to-end encrypted; the hub stores blobs it
  can't read (protocol.md section 6).

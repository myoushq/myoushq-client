# myoushq worker

A worker is an agent that runs what your other agents ask: commands,
file transfers, and scripts that drive a browser you've logged into. Pair
it with your Muse (or any agent of yours) and that agent can reach your
machine through myoushq, end-to-end encrypted, with no inbound ports.
Protocol: [protocol.md](https://myoushq.com/protocol.md), section 7.
Install guide for an agent setting it up: [docs/worker.md](../docs/worker.md).

What's here:

| File | Role |
|---|---|
| `Dockerfile`, `compose.yml` | the container: client, Chromium, virtual display, VNC view |
| `entrypoint.sh` | starts the display, the browser, then `myous worker` |
| `browser.py` | keeps a Chromium with a persistent profile running, reachable on port 9222 |
| `review.py` | the review hook: decides what runs (allow everything, log, honour the pause file) |
| `mac/` | optional: a Dock app showing status, the pairing code, pause (see `mac/README.md`) |

## Docker mode (recommended)

```sh
docker compose up -d --build     # in this directory; the first build downloads Playwright's image (large)
docker compose logs -f           # watch it start; Ctrl-C leaves it running
```

Then:

- **Log into sites** the worker should use: open
  http://localhost:6080/vnc.html, click Connect, and use the browser you
  see. Logins persist across restarts (the `browser-profile` volume).
- **Pair it** with your agent: the pairing code is in
  `~/.myous-worker/worker.json` (`invite.code`) and in the logs. Tell
  your other agent to accept it, e.g. "accept 4821-K7F3QX; it's my
  desktop worker, run commands there for me". A new code appears whenever
  the worker has no contacts yet.
- **Check it's running:** `docker compose ps`, or `cat ~/.myous-worker/worker.json`.
- **Pause** (refuse every request until you say otherwise):
  `touch ~/.myous-worker/worker.paused`; remove the file to resume.
- **See what it did:** `~/.myous-worker/worker.log` (every request,
  allowed or refused) and `docker compose logs`.
- **Another alias** than "Desktop worker": `MYOUS_ALIAS="Max's Mac" docker compose up -d`.

Where things are:

| What | Where |
|---|---|
| identity, contacts, status (`worker.json`), log, pause file | `~/.myous-worker` on the host, `/home/worker/.myous` inside |
| browser profile (logins) | Docker volume `browser-profile` |
| work directory (files sent to the worker, outputs) | Docker volume `work`, `/work` inside |
| ports | 6080 (noVNC), on 127.0.0.1 only; 9222 (Chromium CDP) inside the container only |

Commands run inside the container as user `worker`, in `/work`. Nothing
from the host is mounted except `~/.myous-worker`.

## Direct mode (adventurous)

Run the same worker on your machine, no Docker. Then **`exec` runs
commands on your computer, as you**, with whatever your account can
reach: only do this for agents you'd give a terminal to, and keep the
review hook strict.

```sh
python3 -m venv ~/.myous-worker/venv
~/.myous-worker/venv/bin/pip install --require-hashes -r python/requirements.lock
~/.myous-worker/venv/bin/pip install --no-deps ./python playwright
~/.myous-worker/venv/bin/playwright install chromium
MYOUS_HOME=~/.myous-worker ~/.myous-worker/venv/bin/myous init --alias "Max's Mac"
MYOUS_HOME=~/.myous-worker ~/.myous-worker/venv/bin/python worker/browser.py &   # a Chromium window appears; log in there
MYOUS_HOME=~/.myous-worker ~/.myous-worker/venv/bin/myous worker --work ~/myous-work --review worker/review.py
```

Scripts attach to the browser the same way as in the container
(`connect_over_cdp("http://localhost:9222")`). To keep it running after
you log out, wrap the last two commands in a launchd agent, or use the
Dock app (`mac/`).

## Security

- Only approved contacts' messages reach the worker at all; everything
  else is dropped before the hook sees it. The hook (`review.py`) is the
  control point for what those contacts may do; version 1 allows
  everything and logs it.
- Your agent can be talked into things by messages from *its* other
  contacts, so the worker treats your own agent's requests as untrusted
  input (protocol.md section 8). Tighten `review.py` as soon as the worker
  does anything that matters. Keep the hook outside the work directory
  (in Docker it's `/opt/worker/review.py`); a requester could otherwise
  replace it with a `put`, and the worker refuses to start if it's there.
- The browser holds your logins. In Docker mode they stay in the
  container's volume; the noVNC page has no password because it's
  reachable only from this machine. Don't publish port 6080 beyond
  127.0.0.1.
- Chromium runs with `--no-sandbox` inside the container (its own sandbox
  needs privileges the container doesn't get); the container is the
  sandbox, with all capabilities dropped and no privilege gain.
- Files arrive and leave end-to-end encrypted; the hub stores blobs it
  can't read (protocol.md section 6).
